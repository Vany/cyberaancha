//! aancha-server — CLI dispatch only (PROG.md layout). Everything real lives in modules.

mod answer;
mod auth;
mod backup;
mod config;
mod db;
mod http;
mod index;
mod kb;
mod queue;
mod raw;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "aancha-server", version, about = "Knowledge base server for cyberaancha")]
struct Cli {
    /// Path to secret-free TOML config
    #[arg(long, default_value = "aancha.toml", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server (HTTP + internal schedulers)
    Serve,
    /// Create a dated backup tarball immediately
    Backup,
    /// Drop the live DB and restore the newest backup (destructive)
    Restore {
        /// Restore the newest tarball
        #[arg(long)]
        latest: bool,
        /// Required confirmation — replaces current data
        #[arg(long)]
        yes: bool,
    },
    /// Set panel password for a role (owner | admin); reads stdin if piped
    SetPassword { role: String },
    /// Generate and store a bearer token (collector | preparer | mcp); prints it once
    GenToken { purpose: String },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,aancha_server=debug".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load(&cli.config)?;

    match cli.cmd {
        Cmd::Serve => serve(cfg, cli.config),
        Cmd::Backup => {
            let file = open_db(&cfg)?.with(|c| backup::create(c, &cfg, &cli.config))?;
            println!("{}", file.display());
            Ok(())
        }
        Cmd::Restore { latest, yes } => {
            if !latest {
                bail!("only --latest restore is supported");
            }
            if !yes {
                bail!("restore replaces the current database — re-run with --yes");
            }
            let from = backup::restore_latest(&cfg)?;
            println!("restored from {}", from.display());
            Ok(())
        }
        Cmd::SetPassword { role } => {
            let password = read_secret(&format!("New password for {role}: "))?;
            open_db(&cfg)?.with(|c| auth::set_password(c, &role, &password))?;
            println!("password for {role} set");
            Ok(())
        }
        Cmd::GenToken { purpose } => {
            let token = open_db(&cfg)?.with(|c| auth::gen_token(c, &purpose))?;
            println!("{token}");
            eprintln!("(stored hashed; this is the only time it is shown)");
            Ok(())
        }
    }
}

fn open_db(cfg: &config::Config) -> Result<db::Db> {
    db::Db::open(&cfg.server.data_dir)
}

/// Interactive: prompt without echo, confirm twice. Piped: read one line —
/// enables provisioning like `echo pw | aancha-server set-password owner`.
fn read_secret(prompt: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password(prompt)?;
        let second = rpassword::prompt_password("Repeat: ")?;
        if first != second {
            bail!("passwords do not match");
        }
        Ok(first)
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).context("reading password from stdin")?;
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

#[tokio::main]
async fn serve(cfg: config::Config, config_path: PathBuf) -> Result<()> {
    let db = open_db(&cfg)?;
    let cfg = Arc::new(cfg);
    tracing::debug!(
        writer_heap_mb = cfg.index.writer_heap_mb,
        pace_ms = cfg.harvest.pace_ms,
        "config loaded"
    );

    tokio::spawn(backup::daily_loop(db.clone(), cfg.clone(), config_path.clone()));

    // Open and warm the search index from the DB (derivable — safe to wipe/rebuild).
    let index = Arc::new(index::SearchIndex::open(&cfg.server.index_dir, cfg.index.writer_heap_mb)?);
    let docs = db.with(|c| kb::index_docs(c))?;
    let n = index.rebuild(&docs)?;
    tracing::info!(articles = n, "search index ready");

    let state = http::AppState {
        db,
        cfg: cfg.clone(),
        config_path: Arc::new(config_path),
        basic_cache: Default::default(),
        index,
    };
    let app = http::router(state);

    let listener = tokio::net::TcpListener::bind(&cfg.server.bind).await?;
    tracing::info!(bind = %cfg.server.bind, channel = %cfg.channel.handle, "aancha-server up");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    // SIGTERM is what docker sends; ctrl-c is for dev.
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("installing SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await.expect("installing ctrl-c handler");
}
