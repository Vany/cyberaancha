//! TOML config. Secret-free by design — secrets live in the DB `auth` table (SPEC §13).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub channel: Channel,
    #[serde(default)]
    pub owner: Owner,
    #[serde(default)]
    pub server: Server,
    #[serde(default)]
    pub harvest: Harvest,
    #[serde(default)]
    pub index: Index,
    #[serde(default)]
    pub backup: Backup,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    /// e.g. "@vanyserezhkin" (test) or "@AnchaBaranovaProf" (production)
    pub handle: String,
}

/// User-facing identity of the channel owner, so the product isn't hardcoded to
/// one person. Defaults describe Prof. Baranova; override per instance.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Owner {
    /// Display name (English/MCP), e.g. "Ancha Baranova".
    pub name: String,
    /// How the bot refers to the owner in answers — genitive: "Мнение <reference>:".
    #[serde(rename = "ref")]
    pub reference: String,
    /// The full attribution/disclaimer line appended to every bot answer.
    pub disclaimer: String,
    /// Short instance brand shown in the panel header/title. Empty ⇒ derived from
    /// the channel handle (see `Config::brand`).
    pub brand: String,
}

impl Default for Owner {
    fn default() -> Self {
        // Generic defaults — the code is not hardcoded to any one person. The
        // production instance overrides these in `[owner]` (see cyberaancha.toml.example).
        Self {
            name: String::new(),
            reference: "автора".into(),
            disclaimer: "Справочный материал, не медицинская рекомендация.".into(),
            brand: String::new(),
        }
    }
}

impl Config {
    /// Panel brand: explicit config, else derived from the channel handle.
    pub fn brand(&self) -> String {
        if !self.owner.brand.is_empty() {
            return self.owner.brand.clone();
        }
        self.channel.handle.trim_start_matches('@').to_string()
    }

    /// Owner display name (English/MCP): explicit config, else the brand.
    pub fn owner_display(&self) -> String {
        if self.owner.name.is_empty() {
            self.brand()
        } else {
            self.owner.name.clone()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Server {
    pub bind: String,
    pub public_url: String,
    pub data_dir: PathBuf,
    pub index_dir: PathBuf,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8087".into(),
            public_url: "https://aancha.serezhkin.com".into(),
            data_dir: "data".into(),
            index_dir: "index".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Harvest {
    /// One harvest wave covers this many days of publish dates (SPEC C5).
    pub window_days: u32,
    /// Collector pause between YouTube requests, before jitter.
    pub pace_ms: u64,
}

impl Default for Harvest {
    fn default() -> Self {
        Self { window_days: 7, pace_ms: 1500 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Index {
    /// tantivy writer heap — keep small, n1 has 457 MB total (SPEC C7).
    pub writer_heap_mb: usize,
}

impl Default for Index {
    fn default() -> Self {
        Self { writer_heap_mb: 96 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Backup {
    pub dir: PathBuf,
    pub hour_utc: u8,
    pub keep: u32,
}

impl Default for Backup {
    fn default() -> Self {
        Self { dir: "backups".into(), hour_utc: 3, keep: 3 }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&raw).with_context(|| format!("parsing config {}", path.display()))?;
        Ok(cfg)
    }
}
