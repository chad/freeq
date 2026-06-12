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
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

/// Words too common to anchor a memory recall on. Spoken questions are
/// mostly these — "what do you think about the weather" carries exactly
/// two content words — and FTS-ORing the rest matched essentially every
/// stored exchange.
const RECALL_STOPWORDS: &[&str] = &[
    "the", "and", "but", "for", "are", "was", "were", "you", "your", "yours", "our", "ours", "his",
    "her", "hers", "its", "their", "theirs", "this", "that", "these", "those", "with", "from",
    "have", "has", "had", "what", "whats", "when", "where", "which", "who", "whom", "why", "how",
    "can", "could", "would", "should", "will", "shall", "may", "might", "must", "did", "does",
    "doing", "done", "about", "tell", "please", "okay", "yeah", "yes", "not", "now", "then",
    "there", "here", "they", "them", "she", "him", "out", "into", "over", "under", "again", "just",
    "very", "really", "some", "any", "all", "one", "two", "get", "got", "let", "lets", "know",
    "think", "like", "want", "going", "say", "said", "see", "look", "right", "well", "also", "too",
    "been", "being", "because", "still", "more",
];

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
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating memory parent dir {}", parent.display()))?;
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
    pub fn record(&self, channel: &str, asker: &str, question: &str, answer: &str) -> Result<()> {
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
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' {
                    c
                } else {
                    ' '
                }
            })
            .collect();
        // Build an OR query over the CONTENT words, each quoted as a literal
        // term, so recall fires on ANY shared term and `ORDER BY rank` (bm25)
        // surfaces the most relevant — the intended "top-K relevant" behaviour.
        // A bare multi-word MATCH is implicit-AND, which required a past
        // exchange to contain EVERY word of the question and so almost never hit
        // for natural, paraphrased recall ("remind me what we discussed…").
        //
        // Content words only: OR over stopwords ("what", "the", "you")
        // matched essentially EVERY stored exchange, so unrelated past
        // sessions got prepended to every question's prompt and the bot
        // answered from days-old context.
        let terms: Vec<String> = sanitised
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .filter(|t| t.len() >= 3 && !RECALL_STOPWORDS.contains(&t.as_str()))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let q: String = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");

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
        // Post-filter: with ≥2 content terms in the question, demand a
        // record share at least 2 DISTINCT terms. One shared word
        // ("weather") dragging in every weather exchange ever recorded
        // is how test junk ended up in live answers.
        if terms.len() >= 2 {
            let recs: Vec<Recollection> = recs
                .into_iter()
                .filter(|r| {
                    let hay = format!("{} {}", r.question, r.answer).to_lowercase();
                    terms.iter().filter(|t| hay.contains(t.as_str())).count() >= 2
                })
                .collect();
            return Ok(recs);
        }
        Ok(recs)
    }

    /// Most recent exchanges with a specific person (by their nick/asker),
    /// newest first — "what we last talked about". Powers the memory-aware
    /// greeting: a returning visitor is met with continuity, not a cold open.
    /// Case-insensitive on asker; cross-channel (memory of the person, anywhere).
    pub fn recall_by_asker(&self, asker: &str, limit: usize) -> Result<Vec<Recollection>> {
        let conn = self.conn.lock().expect("memory conn poisoned");
        // `exchanges` is an FTS5 virtual table — COLLATE NOCASE isn't honored on
        // its columns, so lowercase both sides; ts is stored as text so cast it.
        let mut stmt = conn
            .prepare(
                "SELECT asker, question, answer, ts FROM exchanges \
                 WHERE lower(asker) = lower(?1) ORDER BY CAST(ts AS INTEGER) DESC LIMIT ?2",
            )
            .context("preparing recall_by_asker query")?;
        let rows = stmt
            .query_map(params![asker, limit as i64], |row| {
                Ok(Recollection {
                    asker: row.get(0)?,
                    question: row.get(1)?,
                    answer: row.get(2)?,
                    ts: row.get(3)?,
                })
            })
            .context("executing recall_by_asker query")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("decoding recall_by_asker rows")
    }

    /// Format a list of recollections as a prose block for injection
    /// into an LLM prompt. Returns `None` if the list is empty.
    pub fn format_for_prompt(recs: &[Recollection]) -> Option<String> {
        if recs.is_empty() {
            return None;
        }
        let mut out = String::from(
            "PAST EXCHANGES from previous sessions (possibly days old — these were NOT said in \
             this call; use only if genuinely relevant, never as current context):\n",
        );
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
    fn recall_by_asker_survives_a_new_session() {
        // The memory-aware greeting moat: a person's history must come back when
        // the being is reopened (a fresh process / a wake-from-sleep).
        let dir = tempdir().unwrap();
        let db = dir.path().join("test.db");
        {
            let m = Memory::open(&db).unwrap();
            m.record("#a", "chad", "i'm building a persona studio", "ship it")
                .unwrap();
            m.record("#b", "chad", "my band plays avant-blues", "respect")
                .unwrap();
            m.record("#a", "someone-else", "unrelated", "ok").unwrap();
        }
        // New session: reopen the DB from scratch.
        let m2 = Memory::open(&db).unwrap();
        let recs = m2.recall_by_asker("CHAD", 5).unwrap(); // case-insensitive
        assert_eq!(
            recs.len(),
            2,
            "both of chad's exchanges, none of someone-else's"
        );
        assert!(recs.iter().all(|r| r.asker.eq_ignore_ascii_case("chad")));
        let block = Memory::format_for_prompt(&recs).unwrap();
        assert!(block.contains("persona studio") && block.contains("avant-blues"));
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

    #[test]
    fn stopword_only_query_recalls_nothing() {
        // "what do you think about that" has zero content words — OR-ing
        // its stopwords used to match EVERY stored exchange and drag old
        // sessions into every prompt.
        let dir = tempdir().unwrap();
        let m = Memory::open(&dir.path().join("test.db")).unwrap();
        m.record(
            "#x",
            "chad",
            "what do you think about cats",
            "they are fine",
        )
        .unwrap();
        let hits = m
            .recall("what do you think about that", Some("#x"), 5)
            .unwrap();
        assert!(
            hits.is_empty(),
            "stopword-only queries must not recall: {hits:?}"
        );
    }

    #[test]
    fn multi_term_query_needs_two_shared_content_words() {
        let dir = tempdir().unwrap();
        let m = Memory::open(&dir.path().join("test.db")).unwrap();
        m.record("#x", "chad", "weather in berlin today", "rainy")
            .unwrap();
        m.record("#x", "chad", "weather on mars", "thin and cold")
            .unwrap();
        // Two content terms (weather, berlin): only the exchange sharing
        // BOTH comes back — single-word overlap ("weather") no longer
        // drags in every weather exchange ever recorded.
        let hits = m
            .recall("how is the weather in berlin", Some("#x"), 5)
            .unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].question.contains("berlin"));
        // A single content term still recalls on that one word.
        let hits = m.recall("tell me about mars", Some("#x"), 5).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].question.contains("mars"));
    }
}
