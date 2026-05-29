//! Conversation memory — per-bot SQLite store of past exchanges,
//! queryable by FTS5. Drives the "she remembers" feature: when a
//! human (or peer) addresses the bot, we retrieve the top-K relevant
//! past exchanges and inject them into the LLM context so the bot
//! can naturally reference past discussions ("last time you asked
//! about ghostly's voronoi, you ended up at Lloyd's relaxation —
//! does that still hold?").
//!
//! Layout:
//!   * `~/.freeq/bots/<name>/memory.db`
//!   * One FTS5 virtual table `exchanges(channel, asker, question,
//!     answer, ts)`. Channel is stored unindexed so we can filter
//!     scope without polluting FTS rankings.
//!
//! Threading: `rusqlite::Connection` is `Send + !Sync`. We wrap it in
//! a `Mutex` so the async paths (which call `record` / `recall` from
//! arbitrary tasks) can share one connection. The DB ops are short
//! (one statement each), so the lock is held briefly.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// A single past exchange in the bot's memory.
#[derive(Debug, Clone)]
pub struct Recollection {
    pub asker: String,
    pub question: String,
    pub answer: String,
    /// Unix epoch seconds.
    pub ts: i64,
}

pub struct Memory {
    conn: Mutex<Connection>,
}

impl Memory {
    /// Open (or create) the SQLite store at `path`. Initialises the
    /// FTS5 virtual table on first run.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating memory parent dir {}", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening memory DB at {}", path.display()))?;
        // FTS5 virtual table — channel + ts are unindexed (UNINDEXED
        // tells FTS5 not to tokenise them). Tokenizer is `porter` to
        // collapse plurals / tense and improve recall on natural-
        // language questions.
        conn.execute_batch(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS exchanges USING fts5(
                channel UNINDEXED,
                asker UNINDEXED,
                question,
                answer,
                ts UNINDEXED,
                tokenize = 'porter unicode61'
            );
            "#,
        )
        .context("creating exchanges FTS5 table")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Persist one (question, answer) exchange.
    pub fn record(
        &self,
        channel: &str,
        asker: &str,
        question: &str,
        answer: &str,
    ) -> Result<()> {
        let ts = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().expect("memory conn poisoned");
        conn.execute(
            "INSERT INTO exchanges (channel, asker, question, answer, ts) \
             VALUES (?, ?, ?, ?, ?)",
            params![channel, asker, question, answer, ts],
        )
        .context("inserting exchange")?;
        Ok(())
    }

    /// Top-K past exchanges relevant to `query`. Scope can be the
    /// current channel (most common) or `None` for cross-channel
    /// memory.
    pub fn recall(
        &self,
        query: &str,
        channel: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Recollection>> {
        // FTS5 MATCH chokes on punctuation in user input. Strip
        // anything that isn't alphanumeric or a quote; if the result
        // is empty, return no recollections rather than fail.
        let sanitised: String = query
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
            .collect();
        let q = sanitised.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().expect("memory conn poisoned");
        let limit_i = limit as i64;
        let row_to_recollection = |row: &rusqlite::Row| -> rusqlite::Result<Recollection> {
            Ok(Recollection {
                asker: row.get(0)?,
                question: row.get(1)?,
                answer: row.get(2)?,
                ts: row.get(3)?,
            })
        };
        let recs: Vec<Recollection> = match channel {
            Some(ch) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT asker, question, answer, ts FROM exchanges \
                         WHERE channel = ? AND exchanges MATCH ? \
                         ORDER BY rank LIMIT ?",
                    )
                    .context("preparing recall query")?;
                let rows = stmt
                    .query_map(params![ch, q, limit_i], row_to_recollection)
                    .context("executing recall query")?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .context("decoding recall rows")?
            }
            None => {
                let mut stmt = conn
                    .prepare(
                        "SELECT asker, question, answer, ts FROM exchanges \
                         WHERE exchanges MATCH ? \
                         ORDER BY rank LIMIT ?",
                    )
                    .context("preparing recall query")?;
                let rows = stmt
                    .query_map(params![q, limit_i], row_to_recollection)
                    .context("executing recall query")?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .context("decoding recall rows")?
            }
        };
        Ok(recs)
    }

    /// Format a list of recollections as a prose block for injection
    /// into an LLM prompt. Returns `None` if the list is empty.
    pub fn format_for_prompt(recs: &[Recollection]) -> Option<String> {
        if recs.is_empty() {
            return None;
        }
        let mut out = String::from("RELEVANT PAST EXCHANGES (use only if they actually relate; do not force a reference):\n");
        for r in recs {
            let when = chrono::DateTime::<chrono::Utc>::from_timestamp(r.ts, 0)
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".into());
            out.push_str(&format!(
                "- on {when}, {asker} asked: \"{q}\"  → you replied: \"{a}\"\n",
                when = when,
                asker = r.asker,
                q = r.question.replace('\n', " "),
                a = r.answer.replace('\n', " "),
            ));
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn record_and_recall_round_trip() {
        let dir = tempdir().unwrap();
        let m = Memory::open(&dir.path().join("test.db")).unwrap();
        m.record("#x", "chad", "what is voronoi", "a partition of the plane")
            .unwrap();
        m.record("#x", "chad", "today's weather", "sunny").unwrap();

        let hits = m.recall("voronoi", Some("#x"), 3).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].answer.contains("partition"));
    }

    #[test]
    fn channel_scoping() {
        let dir = tempdir().unwrap();
        let m = Memory::open(&dir.path().join("test.db")).unwrap();
        m.record("#a", "x", "topic", "answer-a").unwrap();
        m.record("#b", "x", "topic", "answer-b").unwrap();

        let a = m.recall("topic", Some("#a"), 5).unwrap();
        let cross = m.recall("topic", None, 5).unwrap();
        assert_eq!(a.len(), 1);
        assert!(a[0].answer.ends_with("a"));
        assert_eq!(cross.len(), 2);
    }

    #[test]
    fn empty_query_returns_empty() {
        let dir = tempdir().unwrap();
        let m = Memory::open(&dir.path().join("test.db")).unwrap();
        m.record("#x", "x", "q", "a").unwrap();
        assert!(m.recall("", Some("#x"), 5).unwrap().is_empty());
        // Punctuation-only also yields nothing rather than panic.
        assert!(m.recall("???", Some("#x"), 5).unwrap().is_empty());
    }
}
