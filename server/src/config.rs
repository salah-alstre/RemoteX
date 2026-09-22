use anyhow::{bail, Context, Result};
use std::{env, path::PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub public_url: String,
    pub database_path: PathBuf,
    /// Pepper for hashing device secrets.
    pub server_secret: String,
    pub admin_token: String,
    pub tls: Option<(PathBuf, PathBuf)>,
    pub log_dir: PathBuf,
    pub max_sessions: usize,
    pub max_relay_mbps_per_session: u32,
    pub updates_dir: PathBuf,
}

fn var(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn num<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match var(name) {
        Some(v) => v.parse().map_err(|_| anyhow::anyhow!("{name} must be a number")),
        None => Ok(default),
    }
}

/// Parses `KEY=VALUE` lines. Values are taken literally (no escape processing) so Windows paths such as
/// `certs\server.crt` survive; matching surrounding quotes are removed.
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            let key = key.strip_prefix("export ").unwrap_or(key).trim();
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            (!key.is_empty()).then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

/// Plain HTTP is only acceptable on loopback (development) or when the operator explicitly opts in,
/// so a broken TLS configuration can never silently expose an unencrypted server.
pub fn plain_http_allowed(host: &str, tls_configured: bool, explicit_opt_in: bool) -> bool {
    tls_configured || explicit_opt_in || matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// Real environment variables always win over the file.
fn load_env_file(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (key, value) in parse_env_file(text.trim_start_matches('\u{feff}')) {
        if env::var_os(&key).is_none() {
            env::set_var(key, value);
        }
    }
}

impl Config {
    /// Loads `.env` from the working directory and next to the executable, then the process environment.
    pub fn from_env() -> Result<Self> {
        if let Some(dir) = env::current_exe().ok().as_deref().and_then(|e| e.parent()) {
            load_env_file(&dir.join(".env"));
        }
        load_env_file(std::path::Path::new(".env"));

        let host = var("SERVER_HOST").unwrap_or_else(|| "0.0.0.0".into());
        let port: u16 = num("SERVER_PORT", 8443)?;
        let server_secret = var("SERVER_SECRET").context("SERVER_SECRET is required")?;
        let admin_token = var("ADMIN_TOKEN").context("ADMIN_TOKEN is required")?;
        if server_secret.len() < 32 {
            bail!("SERVER_SECRET must be at least 32 characters");
        }
        if admin_token.len() < 20 {
            bail!("ADMIN_TOKEN must be at least 20 characters");
        }
        let tls = match (var("TLS_CERT_PATH"), var("TLS_KEY_PATH")) {
            (Some(c), Some(k)) => Some((PathBuf::from(c), PathBuf::from(k))),
            (None, None) => None,
            _ => bail!("TLS_CERT_PATH and TLS_KEY_PATH must be set together"),
        };
        if !plain_http_allowed(
            &host,
            tls.is_some(),
            var("ALLOW_PLAIN_HTTP").as_deref() == Some("1"),
        ) {
            bail!(
                "TLS_CERT_PATH and TLS_KEY_PATH are not set: refusing to serve plain HTTP on {host}. \
                 Configure TLS, bind SERVER_HOST=127.0.0.1 for local development, or set ALLOW_PLAIN_HTTP=1 behind a TLS-terminating proxy."
            );
        }
        let scheme = if tls.is_some() { "wss" } else { "ws" };
        Ok(Self {
            public_url: var("PUBLIC_SERVER_URL").unwrap_or_else(|| format!("{scheme}://{host}:{port}/ws")),
            host,
            port,
            database_path: var("DATABASE_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| "remotex.db".into()),
            server_secret,
            admin_token,
            tls,
            log_dir: var("LOG_DIR").map(PathBuf::from).unwrap_or_else(|| "logs".into()),
            max_sessions: num("MAX_SESSIONS", 500)?,
            max_relay_mbps_per_session: num("MAX_RELAY_MBPS_PER_SESSION", 80)?,
            updates_dir: var("UPDATES_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| "updates".into()),
        })
    }

    /// Test/dev configuration with permissive local defaults.
    pub fn for_tests(db: PathBuf) -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 0,
            public_url: String::new(),
            database_path: db,
            server_secret: "x".repeat(40),
            admin_token: "t".repeat(24),
            tls: None,
            log_dir: "logs".into(),
            max_sessions: 50,
            max_relay_mbps_per_session: 200,
            updates_dir: "updates".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_env_file;
    use std::collections::HashMap;

    #[test]
    fn windows_paths_and_crlf_survive_parsing() {
        let text = r"# comment
SERVER_PORT=8443
TLS_CERT_PATH=C:\RemoteX\certs\server.crt

DATABASE_PATH=data\remotex.db
"
        .replace('\n', "\r\n");
        let map: HashMap<_, _> = parse_env_file(&text).into_iter().collect();
        assert_eq!(map["SERVER_PORT"], "8443");
        assert_eq!(map["TLS_CERT_PATH"], r"C:\RemoteX\certs\server.crt");
        assert_eq!(map["DATABASE_PATH"], r"data\remotex.db");
    }

    #[test]
    fn plain_http_is_limited_to_loopback_unless_opted_in() {
        use super::plain_http_allowed;
        assert!(plain_http_allowed("0.0.0.0", true, false));
        assert!(plain_http_allowed("127.0.0.1", false, false));
        assert!(plain_http_allowed("0.0.0.0", false, true));
        assert!(!plain_http_allowed("0.0.0.0", false, false));
        assert!(!plain_http_allowed("45.88.9.191", false, false));
    }

    #[test]
    fn quotes_export_and_malformed_lines() {
        let map: HashMap<_, _> = parse_env_file("export A=\"x y\"\nB='z'\nC=\nnot a pair\n=novalue\n")
            .into_iter()
            .collect();
        assert_eq!(map["A"], "x y");
        assert_eq!(map["B"], "z");
        assert_eq!(map["C"], "");
        assert_eq!(map.len(), 3);
    }
}
