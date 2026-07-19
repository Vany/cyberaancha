//! Credentials: panel passwords (argon2) and bearer tokens (blake3-at-rest).
//! Role names are the two basic-auth usernames; token purposes name the edges.

use anyhow::{Context, Result, bail};
use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use rusqlite::{Connection, OptionalExtension};

pub const ROLES: &[&str] = &["owner", "admin"];
pub const TOKEN_PURPOSES: &[&str] = &["collector", "preparer", "mcp"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
}

impl Role {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

pub fn set_password(conn: &Connection, role: &str, password: &str) -> Result<()> {
    if !ROLES.contains(&role) {
        bail!("unknown role {role:?}, expected one of {ROLES:?}");
    }
    if password.len() < 8 {
        bail!("password too short (min 8 chars)");
    }
    // rand::random uses the OS-seeded CSPRNG; avoids argon2's optional rand_core plumbing.
    let salt_bytes: [u8; 16] = rand::random();
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| anyhow::anyhow!("encoding salt: {e}"))?;
    let phc = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))?
        .to_string();
    upsert(conn, role, "password", &phc)
}

pub fn verify_password(conn: &Connection, role: &str, password: &str) -> Result<bool> {
    let Some(phc) = stored_hash(conn, role, "password")? else {
        tracing::warn!(role, "no password configured — run `aancha-server set-password`");
        return Ok(false);
    };
    let parsed =
        PasswordHash::new(&phc).map_err(|e| anyhow::anyhow!("corrupt stored hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Generates, stores the blake3 of, and returns (once) a new bearer token.
pub fn gen_token(conn: &Connection, purpose: &str) -> Result<String> {
    if !TOKEN_PURPOSES.contains(&purpose) {
        bail!("unknown token purpose {purpose:?}, expected one of {TOKEN_PURPOSES:?}");
    }
    let raw: [u8; 32] = rand::random();
    let token = format!("aancha-{purpose}-{}", hex(&raw));
    upsert(conn, purpose, "token", blake3::hash(token.as_bytes()).to_hex().as_str())?;
    Ok(token)
}

#[allow(dead_code)] // consumed by the bearer middlewares from P2 (collector/preparer) and P6 (mcp)
pub fn verify_token(conn: &Connection, purpose: &str, token: &str) -> Result<bool> {
    let Some(stored) = stored_hash(conn, purpose, "token")? else {
        tracing::warn!(purpose, "no token configured — run `aancha-server gen-token`");
        return Ok(false);
    };
    let stored = blake3::Hash::from_hex(&stored).context("corrupt stored token hash")?;
    // blake3::Hash equality is constant-time.
    Ok(stored == blake3::hash(token.as_bytes()))
}

fn stored_hash(conn: &Connection, name: &str, kind: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT hash FROM auth WHERE name = ?1 AND kind = ?2",
            [name, kind],
            |r| r.get(0),
        )
        .optional()?)
}

fn upsert(conn: &Connection, name: &str, kind: &str, hash: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO auth (name, kind, hash, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET kind = excluded.kind, hash = excluded.hash,
                                         updated_at = excluded.updated_at",
        [name, kind, hash, &jiff::Timestamp::now().to_string()],
    )?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[test]
    fn password_and_token_roundtrip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        db.with(|c| {
            assert!(set_password(c, "nobody", "longenough").is_err());
            assert!(set_password(c, "owner", "short").is_err());
            set_password(c, "owner", "correct horse")?;
            assert!(verify_password(c, "owner", "correct horse")?);
            assert!(!verify_password(c, "owner", "wrong")?);
            assert!(!verify_password(c, "admin", "anything")?); // not configured

            let t = gen_token(c, "collector")?;
            assert!(t.starts_with("aancha-collector-"));
            assert!(verify_token(c, "collector", &t)?);
            assert!(!verify_token(c, "collector", "aancha-collector-forged")?);
            assert!(!verify_token(c, "mcp", &t)?); // not configured
            let t2 = gen_token(c, "collector")?; // rotation invalidates the old one
            assert!(!verify_token(c, "collector", &t)?);
            assert!(verify_token(c, "collector", &t2)?);
            Ok(())
        })
    }
}
