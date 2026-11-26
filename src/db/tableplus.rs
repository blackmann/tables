//! Import connections from TablePlus: an exported `.tableplusconnection` file
//! (RNCryptor v3, protected by the password chosen at export), or TablePlus's
//! own `Connections.plist`.
use super::Engine;
use super::config::{self, ConnectionConfig, Env, SslMode, Store};
use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::Value;
use std::path::Path;

/// Whether the file is an encrypted export, and so needs its password.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.len() > 66 && bytes[0] == 3 && bytes[1] == 1
}

/// Decrypt an RNCryptor v3 password-based message.
pub fn decrypt(data: &[u8], password: &str) -> Result<Vec<u8>> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    use hmac::{Hmac, Mac};
    if !is_encrypted(data) {
        bail!("This isn't an encrypted TablePlus export");
    }
    let (enc_salt, hmac_salt, iv) = (&data[2..10], &data[10..18], &data[18..34]);
    let (body, tag) = data.split_at(data.len() - 32);
    let key = |salt: &[u8]| pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 32>(password.as_bytes(), salt, 10_000);
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key(hmac_salt)).expect("any key length");
    mac.update(body);
    mac.verify_slice(tag).map_err(|_| anyhow!("Wrong password"))?;
    cbc::Decryptor::<aes::Aes256>::new_from_slices(&key(enc_salt), iv)
        .expect("fixed sizes")
        .decrypt_padded_vec_mut::<Pkcs7>(&body[34..])
        .map_err(|_| anyhow!("The file is damaged"))
}

/// Read decrypted contents (JSON, or a plist in either form) as a list of connections.
fn entries(bytes: &[u8]) -> Result<Vec<serde_json::Map<String, Value>>> {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => {
            use std::io::Write;
            use std::process::{Command, Stdio};
            // plutil reads XML and binary plists from stdin, so nothing lands on disk.
            let mut child = Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
            child.stdin.take().context("plutil")?.write_all(bytes)?;
            let out = child.wait_with_output()?;
            if !out.status.success() {
                bail!("Couldn't read the file: it isn't a TablePlus connections file");
            }
            serde_json::from_slice(&out.stdout)?
        }
    };
    // A bare list, or a list under some key of a wrapper object.
    let list = match value {
        Value::Array(a) => a,
        Value::Object(o) => o.into_iter().find_map(|(_, v)| if let Value::Array(a) = v { Some(a) } else { None }).unwrap_or_default(),
        _ => vec![],
    };
    let list: Vec<_> = list.into_iter().filter_map(|v| if let Value::Object(o) = v { Some(o) } else { None }).collect();
    if list.is_empty() {
        bail!("No connections found in the file");
    }
    Ok(list)
}

/// What an import did, for the summary.
#[derive(Default, Debug)]
pub struct Report {
    pub added: Vec<String>,
    pub duplicates: Vec<String>,
    pub ssh: Vec<String>,
    /// Name and engine of connections to engines Tables doesn't speak yet.
    pub unsupported: Vec<(String, String)>,
    pub with_passwords: usize,
}

enum Parsed {
    Ok(ConnectionConfig, Option<String>),
    Ssh(String),
    Unsupported(String, String),
}

fn parse(e: &serde_json::Map<String, Value>, n: usize) -> Parsed {
    let s = |k: &str| match e.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    let b = |k: &str| matches!(e.get(k), Some(Value::Bool(true)));
    let driver = s("Driver");
    let engine = match driver.as_str() {
        "PostgreSQL" => Engine::Postgres,
        "SQLite" => Engine::Sqlite,
        "ClickHouse" => Engine::ClickHouse,
        _ => {
            let name = s("ConnectionName");
            return Parsed::Unsupported(name, if driver.is_empty() { "unknown".into() } else { driver });
        }
    };
    let mut c = ConnectionConfig::new(engine);
    c.id = format!("{}-{n}", c.id);
    c.name = s("ConnectionName");
    if b("isOverSSH") {
        return Parsed::Ssh(c.name);
    }
    if b("isUseSocket") {
        return Parsed::Unsupported(c.name, format!("{driver} over a socket"));
    }
    c.env = match s("Enviroment").to_lowercase().as_str() {
        "production" => Env::Production,
        "staging" | "testing" => Env::Staging,
        _ => Env::Development,
    };
    match engine {
        Engine::Sqlite => c.path = s("DatabasePath"),
        _ => {
            c.host = s("DatabaseHost");
            if c.host.is_empty() {
                c.host = "localhost".into();
            }
            c.port = s("DatabasePort").parse().unwrap_or(engine.default_port());
            let user = s("DatabaseUser");
            if !user.is_empty() {
                c.user = user;
            }
            c.database = s("DatabaseName");
            // TablePlus's order: preferred, disabled, required, verify-ca, verify-full.
            c.ssl = match e.get("tLSMode").and_then(Value::as_i64).unwrap_or(0) {
                1 => SslMode::Disable,
                2..=4 => SslMode::Require,
                _ => SslMode::Prefer,
            };
            // Tables talks to ClickHouse over HTTP, not its native protocol.
            if engine == Engine::ClickHouse {
                match c.port {
                    9000 => c.port = 8123,
                    9440 => (c.port, c.ssl) = (8443, SslMode::Require),
                    _ => {}
                }
            }
        }
    }
    if c.name.is_empty() {
        c.name = match engine {
            Engine::Sqlite => c.database_name(),
            _ if !c.database.is_empty() => c.database.clone(),
            _ => c.host.clone(),
        };
    }
    let password = Some(s("DatabasePassword")).filter(|p| !p.is_empty());
    Parsed::Ok(c, password)
}

fn same(a: &ConnectionConfig, b: &ConnectionConfig) -> bool {
    a.engine == b.engine && a.path == b.path && a.host == b.host && a.port == b.port && a.database == b.database && a.user == b.user
}

/// Add the file's connections to the saved ones, skipping any already there.
/// `password` opens an encrypted export.
pub fn import(path: &Path, password: &str) -> Result<Report> {
    let bytes = std::fs::read(path).with_context(|| format!("Couldn't read {}", path.display()))?;
    let bytes = if is_encrypted(&bytes) { decrypt(&bytes, password)? } else { bytes };
    let list = entries(&bytes)?;
    // Load afresh: other windows may have saved since this one loaded.
    let mut store = Store::load();
    let mut r = Report::default();
    for (n, e) in list.iter().enumerate() {
        match parse(e, n) {
            Parsed::Ssh(name) => r.ssh.push(name),
            Parsed::Unsupported(name, engine) => r.unsupported.push((name, engine)),
            Parsed::Ok(c, _) if store.connections.iter().any(|x| same(x, &c)) => r.duplicates.push(c.name),
            Parsed::Ok(c, pass) => {
                if let Some(p) = pass
                    && config::set_password(&c.id, &p).is_ok()
                {
                    r.with_passwords += 1;
                }
                r.added.push(c.name.clone());
                store.connections.push(c);
            }
        }
    }
    if !r.added.is_empty() {
        store.save()?;
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    use hmac::{Hmac, Mac};

    fn encrypt(plain: &[u8], password: &str) -> Vec<u8> {
        let (enc_salt, hmac_salt, iv) = ([1u8; 8], [2u8; 8], [3u8; 16]);
        let key = |salt: &[u8]| pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 32>(password.as_bytes(), salt, 10_000);
        let mut out = vec![3, 1];
        out.extend(enc_salt);
        out.extend(hmac_salt);
        out.extend(iv);
        out.extend(cbc::Encryptor::<aes::Aes256>::new_from_slices(&key(&enc_salt), &iv).unwrap().encrypt_padded_vec_mut::<Pkcs7>(plain));
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key(&hmac_salt)).unwrap();
        mac.update(&out);
        out.extend(mac.finalize().into_bytes());
        out
    }

    #[test]
    fn decrypts_and_reads() {
        let json = br#"[
            {"ConnectionName":"app","Driver":"PostgreSQL","DatabaseHost":"db.example.com","DatabasePort":"6543","DatabaseUser":"me","DatabaseName":"app","DatabasePassword":"pw","Enviroment":"production","tLSMode":2,"isOverSSH":false},
            {"ConnectionName":"tunnel","Driver":"PostgreSQL","isOverSSH":true},
            {"ConnectionName":"docs","Driver":"Mongo"},
            {"ConnectionName":"","Driver":"SQLite","DatabasePath":"/tmp/x.sqlite"}
        ]"#;
        let file = encrypt(json, "secret");
        assert!(decrypt(&file, "nope").is_err());
        let list = entries(&decrypt(&file, "secret").unwrap()).unwrap();
        assert_eq!(list.len(), 4);
        let Parsed::Ok(c, p) = parse(&list[0], 0) else { panic!() };
        assert_eq!((c.host.as_str(), c.port, c.user.as_str(), c.env, c.ssl, p.as_deref()), ("db.example.com", 6543, "me", Env::Production, SslMode::Require, Some("pw")));
        assert!(matches!(parse(&list[1], 1), Parsed::Ssh(_)));
        assert!(matches!(parse(&list[2], 2), Parsed::Unsupported(_, ref e) if e == "Mongo"));
        let Parsed::Ok(c, _) = parse(&list[3], 3) else { panic!() };
        assert_eq!((c.engine, c.name.as_str()), (Engine::Sqlite, "x"));
    }

    #[test]
    fn reads_plists() {
        let xml = br#"<?xml version="1.0"?><plist version="1.0"><array><dict><key>ConnectionName</key><string>a</string><key>Driver</key><string>PostgreSQL</string></dict></array></plist>"#;
        assert_eq!(entries(xml).unwrap().len(), 1);
    }
}

