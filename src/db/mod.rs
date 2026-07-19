//! SQLite access. One connection behind a mutex: our load is tiny row traffic
//! (heavy reads live in tantivy), and a single writer is SQLite reality anyway.
//! Async code goes through `call` (spawn_blocking); sync CLI paths use `with`.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Registry of migrations, applied in order via PRAGMA user_version.
/// Adding a file without listing it here means it never runs — keep in sync.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("migrations/001_init.sql")),
    ("002_queue", include_str!("migrations/002_queue.sql")),
    ("003_comments_chat", include_str!("migrations/003_comments_chat.sql")),
];

pub const DB_FILE: &str = "aancha.db";

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    /// Open (creating dirs/file as needed), set pragmas, run pending migrations.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("creating data dir {}", data_dir.display()))?;
        let path = data_dir.join(DB_FILE);
        let conn = Connection::open(&path)
            .with_context(|| format!("opening database {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        migrate(&conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// Async boundary: run a closure on the connection in a blocking task.
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = db.lock().expect("db mutex poisoned");
            f(&mut conn)
        })
        .await
        .context("db task join")?
    }

    /// Sync access for CLI paths.
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.0.lock().expect("db mutex poisoned");
        f(&mut conn)
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let applied: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version <= applied {
            continue;
        }
        conn.execute_batch(&format!(
            "BEGIN;\n{sql}\nPRAGMA user_version = {version};\nCOMMIT;"
        ))
        .with_context(|| format!("applying migration {name}"))?;
        tracing::info!(migration = name, "applied");
    }
    Ok(())
}

/// meta: tiny key-value store for watermarks, clocks, statuses.
pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_migrates_and_meta_roundtrips() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        db.with(|c| {
            let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            assert_eq!(v, MIGRATIONS.len() as i64);
            assert_eq!(meta_get(c, "absent")?, None);
            meta_set(c, "k", "v1")?;
            meta_set(c, "k", "v2")?;
            assert_eq!(meta_get(c, "k")?.as_deref(), Some("v2"));
            Ok(())
        })
    }
}
