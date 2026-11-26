//! Saved connections and per-connection app state, kept as JSON in Application
//! Support. Passwords never touch the file: they live in the login keychain.
use super::{Driver, Engine};
use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Env {
    #[default]
    Development,
    Staging,
    Production,
}

#[allow(dead_code)]
impl Env {
    pub const ALL: [Env; 3] = [Env::Development, Env::Staging, Env::Production];
    pub fn label(self) -> &'static str {
        match self {
            Env::Development => "development",
            Env::Staging => "staging",
            Env::Production => "production",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    /// Encrypt, but accept any certificate (self-signed, cloud proxies).
    RequireNoVerify,
}

#[allow(dead_code)]
impl SslMode {
    pub const ALL: [SslMode; 4] = [SslMode::Disable, SslMode::Prefer, SslMode::Require, SslMode::RequireNoVerify];
    pub fn label(self) -> &'static str {
        match self {
            SslMode::Disable => "Disable",
            SslMode::Prefer => "Prefer",
            SslMode::Require => "Require",
            SslMode::RequireNoVerify => "Require, don't verify",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    #[serde(default)]
    pub env: Env,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    /// The database file, for SQLite.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub ssl: SslMode,
}

impl ConnectionConfig {
    pub fn new(engine: Engine) -> Self {
        let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        ConnectionConfig {
            id,
            name: String::new(),
            engine,
            env: Env::Development,
            host: "localhost".into(),
            port: 5432,
            user: std::env::var("USER").unwrap_or_default(),
            database: String::new(),
            path: String::new(),
            ssl: SslMode::Prefer,
        }
    }
    /// Where the connection points, for the status bar.
    pub fn address(&self) -> String {
        match self.engine {
            Engine::Sqlite => self.path.replace(&std::env::var("HOME").unwrap_or_default(), "~"),
            Engine::Postgres => format!("{}:{}", self.host, self.port),
        }
    }
    pub fn database_name(&self) -> String {
        match self.engine {
            Engine::Sqlite => std::path::Path::new(&self.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            Engine::Postgres => self.database.clone(),
        }
    }
    /// Connect, returning the driver and, where the engine has one, a way to
    /// cancel its running query from another thread.
    pub fn open(&self, password: Option<String>) -> Result<(Box<dyn Driver>, Option<Box<dyn Fn() + Send + Sync>>)> {
        match self.engine {
            Engine::Sqlite => Ok((Box::new(super::sqlite::Sqlite::open(&self.path)?), None)),
            Engine::Postgres => {
                let (d, c) = super::postgres::Postgres::connect(self, password)?.into_parts();
                Ok((d, Some(c)))
            }
        }
    }
}

/// A query kept for later, per connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConnectionState {
    #[serde(default)]
    pub pinned: BTreeSet<String>,
    #[serde(default)]
    pub saved: Vec<SavedQuery>,
    #[serde(default)]
    pub history: Vec<String>,
    #[serde(default)]
    pub schema: Option<String>,
    /// When it was last connected to, as a Unix timestamp.
    #[serde(default)]
    pub last_used: Option<i64>,
}

/// App preferences.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    /// The interface font; the system's when unset.
    #[serde(default)]
    pub ui_font: Option<String>,
    #[serde(default)]
    pub ui_size: Option<f32>,
    /// The font for code, SQL and data; the default when unset.
    #[serde(default)]
    pub mono_font: Option<String>,
    #[serde(default)]
    pub mono_size: Option<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
    #[serde(default)]
    pub state: BTreeMap<String, ConnectionState>,
    #[serde(default)]
    pub last: Option<String>,
}

fn dir() -> PathBuf {
    if let Ok(d) = std::env::var("TABLES_HOME") {
        return PathBuf::from(d);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("Library/Application Support/Tables")
}

impl Store {
    pub fn load() -> Store {
        std::fs::read(dir().join("store.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        let d = dir();
        std::fs::create_dir_all(&d)?;
        let tmp = d.join("store.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, d.join("store.json"))?;
        Ok(())
    }
    pub fn state(&mut self, id: &str) -> &mut ConnectionState {
        self.state.entry(id.to_string()).or_default()
    }
    pub fn remember(&mut self, id: &str, sql: &str) {
        let h = &mut self.state(id).history;
        h.retain(|s| s != sql);
        h.insert(0, sql.to_string());
        h.truncate(50);
    }
}

const SERVICE: &str = "Tables";

pub fn password(id: &str) -> Option<String> {
    security_framework::passwords::get_generic_password(SERVICE, id).ok().and_then(|b| String::from_utf8(b).ok())
}

pub fn set_password(id: &str, password: &str) -> Result<()> {
    if password.is_empty() {
        let _ = security_framework::passwords::delete_generic_password(SERVICE, id);
        return Ok(());
    }
    security_framework::passwords::set_generic_password(SERVICE, id, password.as_bytes()).map_err(|e| anyhow!("Keychain: {e}"))
}

pub fn forget_password(id: &str) {
    let _ = security_framework::passwords::delete_generic_password(SERVICE, id);
}

/// Read a `postgres://user:pass@host:port/db?sslmode=…` URL into a config.
pub fn from_url(url: &str) -> Result<(ConnectionConfig, Option<String>)> {
    let url = url.trim();
    if let Some(path) = url.strip_prefix("sqlite://").or_else(|| url.strip_prefix("file:")) {
        let mut c = ConnectionConfig::new(Engine::Sqlite);
        c.path = path.to_string();
        c.name = c.database_name();
        return Ok((c, None));
    }
    let rest = url.strip_prefix("postgres://").or_else(|| url.strip_prefix("postgresql://")).context("Expected a postgres:// or sqlite:// URL")?;
    let mut c = ConnectionConfig::new(Engine::Postgres);
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (auth, hostpart) = rest.rsplit_once('@').map(|(a, h)| (Some(a), h)).unwrap_or((None, rest));
    let (hostport, db) = hostpart.split_once('/').unwrap_or((hostpart, ""));
    let (host, port) = hostport.rsplit_once(':').map(|(h, p)| (h, p.parse().unwrap_or(5432))).unwrap_or((hostport, 5432));
    c.host = host.to_string();
    c.port = port;
    c.database = db.to_string();
    let mut pass = None;
    if let Some(a) = auth {
        let (u, p) = a.split_once(':').map(|(u, p)| (u, Some(p))).unwrap_or((a, None));
        c.user = decode(u);
        pass = p.map(decode);
    }
    for kv in query.split('&') {
        if let Some(("sslmode", v)) = kv.split_once('=') {
            c.ssl = match v {
                "disable" => SslMode::Disable,
                "require" | "verify-ca" | "verify-full" => SslMode::Require,
                _ => SslMode::Prefer,
            };
        }
    }
    c.name = if c.database.is_empty() { c.host.clone() } else { c.database.clone() };
    Ok((c, pass))
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_urls() {
        let (c, p) = from_url("postgres://me:p%40ss@db.example.com:6543/app?sslmode=require").unwrap();
        assert_eq!((c.user.as_str(), c.host.as_str(), c.port, c.database.as_str(), c.ssl), ("me", "db.example.com", 6543, "app", SslMode::Require));
        assert_eq!(p.as_deref(), Some("p@ss"));
    }
}
