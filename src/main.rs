//! aancha-server — CLI dispatch only (PROG.md layout). Everything real lives in modules.

mod config;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    /// Drop everything and restore from a backup (destructive)
    Restore {
        /// Restore the newest tarball
        #[arg(long)]
        latest: bool,
        /// Required confirmation — this wipes current data
        #[arg(long)]
        yes: bool,
    },
    /// Set panel password for a role (owner | admin)
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
        Cmd::Serve => serve(cfg),
        // Fail loudly until implemented (P1 tasks in TODO.md) — never a silent no-op.
        Cmd::Backup => bail!("backup: not implemented yet (P1)"),
        Cmd::Restore { .. } => bail!("restore: not implemented yet (P1)"),
        Cmd::SetPassword { .. } => bail!("set-password: not implemented yet (P1)"),
        Cmd::GenToken { .. } => bail!("gen-token: not implemented yet (P1)"),
    }
}

#[tokio::main]
async fn serve(cfg: config::Config) -> Result<()> {
    let app = axum::Router::new().route("/healthz", axum::routing::get(healthz));

    let listener = tokio::net::TcpListener::bind(&cfg.server.bind).await?;
    tracing::info!(bind = %cfg.server.bind, channel = %cfg.channel.handle, "aancha-server up");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
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
