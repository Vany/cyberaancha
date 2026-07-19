//! Backups: dated tar.gz of a consistent SQLite snapshot (+ config copy for
//! reference). Created by the internal daily scheduler, the `backup` CLI, or
//! POST /api/backups. The tantivy index is derivable and never backed up.
//! Restore is a pure-filesystem operation on a stopped server.

use crate::config::Config;
use crate::db::{self, Db};
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use jiff::civil::Time;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use rusqlite::Connection;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

const PREFIX: &str = "aancha-";
const SUFFIX: &str = ".tar.gz";

/// Snapshot + tar + prune. Also records last_backup_{at,status,file} in meta.
pub fn create(conn: &Connection, cfg: &Config, config_path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(&cfg.backup.dir)?;
    let stamp = Timestamp::now().strftime("%Y%m%d-%H%M%S").to_string();
    let target = unique_path(&cfg.backup.dir, &stamp);
    let snapshot = cfg.backup.dir.join(format!(".snapshot-{stamp}.db"));

    // VACUUM INTO gives a consistent, compact copy without long write locks.
    let result = (|| -> Result<()> {
        conn.execute("VACUUM INTO ?1", [snapshot.to_str().context("non-utf8 path")?])?;
        let tar_gz = File::create(&target)?;
        let mut builder =
            tar::Builder::new(GzEncoder::new(tar_gz, Compression::default()));
        builder.append_path_with_name(&snapshot, db::DB_FILE)?;
        if config_path.exists() {
            // Reference copy only; restore never overwrites the live config.
            builder.append_path_with_name(config_path, "aancha.toml.reference")?;
        }
        builder.into_inner()?.finish()?;
        Ok(())
    })();
    let _ = fs::remove_file(&snapshot);

    let now = Timestamp::now().to_string();
    match &result {
        Ok(()) => {
            db::meta_set(conn, "last_backup_at", &now)?;
            db::meta_set(conn, "last_backup_status", "ok")?;
            db::meta_set(conn, "last_backup_file", &target.display().to_string())?;
        }
        Err(e) => {
            let _ = fs::remove_file(&target);
            db::meta_set(conn, "last_backup_at", &now)?;
            db::meta_set(conn, "last_backup_status", &format!("failed: {e:#}"))?;
        }
    }
    result?;

    prune(&cfg.backup.dir, cfg.backup.keep as usize)?;
    tracing::info!(file = %target.display(), "backup created");
    Ok(target)
}

pub fn list(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = vec![];
    if dir.exists() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
                found.push(path);
            }
        }
    }
    found.sort(); // names embed sortable timestamps
    Ok(found)
}

fn prune(dir: &Path, keep: usize) -> Result<()> {
    let found = list(dir)?;
    for old in found.iter().rev().skip(keep.max(1)) {
        fs::remove_file(old).with_context(|| format!("pruning {}", old.display()))?;
        tracing::info!(file = %old.display(), "backup pruned");
    }
    Ok(())
}

/// Timestamps are second-granular; on-demand backups may collide — disambiguate.
fn unique_path(dir: &Path, stamp: &str) -> PathBuf {
    let base = dir.join(format!("{PREFIX}{stamp}{SUFFIX}"));
    if !base.exists() {
        return base;
    }
    (2..)
        .map(|n| dir.join(format!("{PREFIX}{stamp}-{n}{SUFFIX}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// Destructive: replace the live DB with the newest tarball's snapshot.
/// Guarded by the caller's --yes; refuses while the server is listening.
/// The old DB is kept aside as *.pre-restore-<stamp>, WAL leftovers removed.
pub fn restore_latest(cfg: &Config) -> Result<PathBuf> {
    if std::net::TcpListener::bind(&cfg.server.bind).is_err() {
        bail!(
            "something is listening on {} — stop the server before restore",
            cfg.server.bind
        );
    }
    let newest = list(&cfg.backup.dir)?
        .pop()
        .with_context(|| format!("no backups found in {}", cfg.backup.dir.display()))?;

    let db_path = cfg.server.data_dir.join(db::DB_FILE);
    let incoming = db_path.with_extension("db.restoring");
    let mut extracted = false;
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(&newest)?));
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new(db::DB_FILE) {
            fs::create_dir_all(&cfg.server.data_dir)?;
            entry.unpack(&incoming)?;
            extracted = true;
        }
    }
    if !extracted {
        bail!("{} does not contain {}", newest.display(), db::DB_FILE);
    }

    if db_path.exists() {
        let stamp = Timestamp::now().strftime("%Y%m%d-%H%M%S").to_string();
        fs::rename(&db_path, db_path.with_extension(format!("db.pre-restore-{stamp}")))?;
    }
    // Stale WAL/SHM would corrupt the restored snapshot's view.
    for ext in ["db-wal", "db-shm"] {
        let _ = fs::remove_file(db_path.with_extension(ext));
    }
    fs::rename(&incoming, &db_path)?;
    Ok(newest)
}

/// Daily scheduler: fires at backup.hour_utc. Failures are logged and recorded
/// in meta (surfaced by /api/state) but never kill the server.
pub async fn daily_loop(db: Db, cfg: std::sync::Arc<Config>, config_path: PathBuf) {
    loop {
        let target_time = Time::midnight() + (cfg.backup.hour_utc as i32).hours();
        let now = Timestamp::now().to_zoned(TimeZone::UTC);
        let mut next = now.with().time(target_time).build().unwrap_or_else(|_| now.clone());
        if next <= now {
            next = next.checked_add(1.day()).expect("time overflow");
        }
        let wait = next.duration_since(&now).unsigned_abs();
        tracing::debug!(at = %next, "next daily backup scheduled");
        tokio::time::sleep(wait).await;

        let (cfg2, path2) = (cfg.clone(), config_path.clone());
        match db.call(move |c| create(c, &cfg2, &path2)).await {
            Ok(file) => tracing::info!(file = %file.display(), "daily backup done"),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "daily backup FAILED"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(root: &Path) -> Config {
        let toml = format!(
            r#"
            [channel]
            handle = "@test"
            [server]
            bind = "127.0.0.1:0"
            data_dir = "{root}/data"
            index_dir = "{root}/index"
            [backup]
            dir = "{root}/backups"
            keep = 2
            "#,
            root = root.display()
        );
        toml::from_str(&toml).unwrap()
    }

    #[test]
    fn backup_prune_restore_roundtrip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cfg = test_cfg(dir.path());
        let db = Db::open(&cfg.server.data_dir)?;
        let config_path = dir.path().join("absent.toml");

        db.with(|c| {
            db::meta_set(c, "marker", "v1")?;
            create(c, &cfg, &config_path)?;
            db::meta_set(c, "marker", "v2")?; // not in the backup
            assert_eq!(db::meta_get(c, "last_backup_status")?.as_deref(), Some("ok"));
            Ok(())
        })?;
        drop(db);

        let restored_from = restore_latest(&cfg)?;
        assert!(restored_from.to_string_lossy().ends_with(SUFFIX));
        let db = Db::open(&cfg.server.data_dir)?;
        db.with(|c| {
            assert_eq!(db::meta_get(c, "marker")?.as_deref(), Some("v1"));
            Ok(())
        })?;
        // Safety copy of the pre-restore DB exists.
        let safety: Vec<_> = fs::read_dir(&cfg.server.data_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("pre-restore"))
            .collect();
        assert_eq!(safety.len(), 1);

        // keep=2 prunes down to two tarballs.
        db.with(|c| {
            create(c, &cfg, &config_path)?;
            create(c, &cfg, &config_path)?;
            Ok(())
        })?;
        assert_eq!(list(&cfg.backup.dir)?.len(), 2);
        Ok(())
    }
}
