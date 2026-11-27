//! Local SQLite database (`%APPDATA%\Saywell\saywell.db`): dictation history and the personal
//! dictionary. Nothing in it ever leaves the PC.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

const SCHEMA_VERSION: i32 = 1;

pub struct Store {
    db: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    /// Unix time in milliseconds.
    pub ts: i64,
    /// Program the text went into ("chrome", "Code"), if known.
    pub app: String,
    /// Whisper's text before cleanup.
    pub raw: String,
    /// The text that was inserted.
    pub text: String,
    pub audio_ms: i64,
    pub latency_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Term {
    pub id: i64,
    pub term: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Rule {
    pub id: i64,
    pub find: String,
    pub replace: String,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.pragma_update(None, "journal_mode", "WAL")?;
        let version: i32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS history (
                     id INTEGER PRIMARY KEY,
                     ts INTEGER NOT NULL,
                     app TEXT NOT NULL DEFAULT '',
                     raw TEXT NOT NULL,
                     text TEXT NOT NULL,
                     audio_ms INTEGER NOT NULL DEFAULT 0,
                     latency_ms INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS history_ts ON history(ts);
                 CREATE TABLE IF NOT EXISTS dict_terms (
                     id INTEGER PRIMARY KEY,
                     term TEXT NOT NULL UNIQUE COLLATE NOCASE
                 );
                 CREATE TABLE IF NOT EXISTS dict_rules (
                     id INTEGER PRIMARY KEY,
                     find TEXT NOT NULL UNIQUE COLLATE NOCASE,
                     replace TEXT NOT NULL
                 );",
            )?;
        }
        db.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self { db: Mutex::new(db) })
    }

    // ---- history ----

    pub fn add_history(&self, app: &str, raw: &str, text: &str, audio_ms: i64, latency_ms: i64) -> Result<i64> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO history (ts, app, raw, text, audio_ms, latency_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now_ms(), app, raw, text, audio_ms, latency_ms],
        )?;
        Ok(db.last_insert_rowid())
    }

    /// Newest first. `query` matches the text or the app name (case-insensitive).
    pub fn list_history(&self, query: &str, limit: i64, offset: i64) -> Result<Vec<HistoryEntry>> {
        let db = self.db.lock().unwrap();
        let pattern = format!("%{}%", query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = db.prepare(
            "SELECT id, ts, app, raw, text, audio_ms, latency_ms FROM history
             WHERE ?1 = '' OR text LIKE ?2 ESCAPE '\\' OR app LIKE ?2 ESCAPE '\\'
             ORDER BY ts DESC, id DESC LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt.query_map(params![query, pattern, limit, offset], |r| {
            Ok(HistoryEntry {
                id: r.get(0)?,
                ts: r.get(1)?,
                app: r.get(2)?,
                raw: r.get(3)?,
                text: r.get(4)?,
                audio_ms: r.get(5)?,
                latency_ms: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn history_text(&self, id: i64) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row("SELECT text FROM history WHERE id = ?1", [id], |r| r.get(0)).optional()?)
    }

    pub fn delete_history(&self, id: i64) -> Result<()> {
        self.db.lock().unwrap().execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear_history(&self) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute("DELETE FROM history", [])?;
        db.execute_batch("VACUUM")?; // actually remove the text from the file
        Ok(())
    }

    /// Delete entries older than `days` (0 keeps everything). Returns how many were removed.
    pub fn prune_history(&self, days: u32) -> Result<usize> {
        if days == 0 {
            return Ok(0);
        }
        let cutoff = now_ms() - days as i64 * 86_400_000;
        Ok(self.db.lock().unwrap().execute("DELETE FROM history WHERE ts < ?1", [cutoff])?)
    }

    // ---- dictionary ----

    pub fn terms(&self) -> Result<Vec<Term>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT id, term FROM dict_terms ORDER BY term COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok(Term { id: r.get(0)?, term: r.get(1)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn add_term(&self, term: &str) -> Result<()> {
        let term = term.trim();
        if !term.is_empty() {
            self.db.lock().unwrap().execute("INSERT OR IGNORE INTO dict_terms (term) VALUES (?1)", [term])?;
        }
        Ok(())
    }

    pub fn remove_term(&self, id: i64) -> Result<()> {
        self.db.lock().unwrap().execute("DELETE FROM dict_terms WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn rules(&self) -> Result<Vec<Rule>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT id, find, replace FROM dict_rules ORDER BY find COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok(Rule { id: r.get(0)?, find: r.get(1)?, replace: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Add or update the rule for `find`.
    pub fn set_rule(&self, find: &str, replace: &str) -> Result<()> {
        let find = find.trim();
        if !find.is_empty() {
            self.db.lock().unwrap().execute(
                "INSERT INTO dict_rules (find, replace) VALUES (?1, ?2)
                 ON CONFLICT(find) DO UPDATE SET replace = excluded.replace",
                params![find, replace],
            )?;
        }
        Ok(())
    }

    pub fn remove_rule(&self, id: i64) -> Result<()> {
        self.db.lock().unwrap().execute("DELETE FROM dict_rules WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_add_search_delete_prune() {
        let s = Store::open_in_memory().unwrap();
        let a = s.add_history("chrome", "um hello world", "Hello world.", 1200, 400).unwrap();
        let b = s.add_history("Code", "fix the 50% bug", "Fix the 50% bug.", 900, 350).unwrap();
        assert_eq!(s.list_history("", 10, 0).unwrap().iter().map(|e| e.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(s.list_history("hello", 10, 0).unwrap()[0].id, a);
        assert_eq!(s.list_history("CODE", 10, 0).unwrap()[0].id, b, "app name, case-insensitive");
        assert_eq!(s.list_history("50%", 10, 0).unwrap().len(), 1, "% is literal");
        assert_eq!(s.list_history("5_%", 10, 0).unwrap().len(), 0, "_ is literal");
        assert_eq!(s.history_text(a).unwrap().as_deref(), Some("Hello world."));
        s.delete_history(a).unwrap();
        assert_eq!(s.history_text(a).unwrap(), None);
        // Backdate b and prune.
        s.db.lock().unwrap().execute("UPDATE history SET ts = ts - 40 * 86400000", []).unwrap();
        assert_eq!(s.prune_history(0).unwrap(), 0, "0 days keeps everything");
        assert_eq!(s.prune_history(30).unwrap(), 1);
        s.add_history("x", "r", "t", 0, 0).unwrap();
        s.clear_history().unwrap();
        assert!(s.list_history("", 10, 0).unwrap().is_empty());
    }

    #[test]
    fn dictionary_terms_and_rules() {
        let s = Store::open_in_memory().unwrap();
        s.add_term(" Kubernetes ").unwrap();
        s.add_term("kubernetes").unwrap(); // duplicate, different case
        s.add_term("").unwrap();
        s.add_term("Anneliese").unwrap();
        let terms: Vec<String> = s.terms().unwrap().into_iter().map(|t| t.term).collect();
        assert_eq!(terms, vec!["Anneliese", "Kubernetes"]);
        s.set_rule("my email", "me@example.com").unwrap();
        s.set_rule("My Email", "new@example.com").unwrap(); // updates, case-insensitive
        let rules = s.rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].replace, "new@example.com");
        s.remove_rule(rules[0].id).unwrap();
        assert!(s.rules().unwrap().is_empty());
    }
}
