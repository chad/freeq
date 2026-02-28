//! Persistent project memory backed by SQLite.
//!
//! Stores project specs, decisions, code artifacts, and conversation context
//! so agents can maintain state across sessions.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// A stored memory entry.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub project: String,
    pub kind: String,
    pub key: String,
    pub value: String,
    pub created_at: String,
}

/// Project memory store.
pub struct Memory {
    db: Mutex<Connection>,
}

impl Memory {
    /// Open or create a memory database.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path).context("Failed to open memory database")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS memory (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project TEXT NOT NULL,
                kind TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memory_project_kind
                ON memory(project, kind);
            CREATE INDEX IF NOT EXISTS idx_memory_project_key
                ON memory(project, key);",
        )?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Open an in-memory database (for testing).
    pub fn in_memory() -> Result<Self> {
        Self::open(Path::new(":memory:"))
    }

    /// Store a value. Overwrites if same project+kind+key exists.
    pub fn set(&self, project: &str, kind: &str, key: &str, value: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        db.execute(
            "INSERT OR REPLACE INTO memory (project, kind, key, value, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![project, kind, key, value, now],
        )?;
        Ok(())
    }

    /// Get a specific value.
    pub fn get(&self, project: &str, kind: &str, key: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT value FROM memory WHERE project = ?1 AND kind = ?2 AND key = ?3
             ORDER BY id DESC LIMIT 1",
        )?;
        let result = stmt
            .query_row(rusqlite::params![project, kind, key], |row| {
                row.get::<_, String>(0)
            })
            .ok();
        Ok(result)
    }

    /// Get all entries of a kind for a project.
    pub fn list(&self, project: &str, kind: &str) -> Result<Vec<Entry>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, project, kind, key, value, created_at
             FROM memory WHERE project = ?1 AND kind = ?2
             ORDER BY id ASC",
        )?;
        let entries = stmt
            .query_map(rusqlite::params![project, kind], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    kind: row.get(2)?,
                    key: row.get(3)?,
                    value: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Append to a log (doesn't overwrite — adds new entry).
    pub fn log(&self, project: &str, kind: &str, value: &str) -> Result<()> {
        let db = self.db.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let key = now.clone();
        db.execute(
            "INSERT INTO memory (project, kind, key, value, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![project, kind, key, value, now],
        )?;
        Ok(())
    }

    /// Get the full project context as a summary string (for LLM context).
    pub fn project_context(&self, project: &str) -> Result<String> {
        let mut parts = Vec::new();

        if let Some(spec) = self.get(project, "spec", "current")? {
            parts.push(format!("## Spec\n{spec}"));
        }
        if let Some(stack) = self.get(project, "decision", "stack")? {
            parts.push(format!("## Stack\n{stack}"));
        }
        if let Some(arch) = self.get(project, "decision", "architecture")? {
            parts.push(format!("## Architecture\n{arch}"));
        }

        let files = self.list(project, "file")?;
        if !files.is_empty() {
            let file_list: Vec<String> = files
                .iter()
                .map(|e| format!("### {}\n```\n{}\n```", e.key, e.value))
                .collect();
            parts.push(format!("## Files\n{}", file_list.join("\n\n")));
        }

        if let Some(url) = self.get(project, "deploy", "url")? {
            parts.push(format!("## Deployed\n{url}"));
        }

        Ok(parts.join("\n\n"))
    }
}
