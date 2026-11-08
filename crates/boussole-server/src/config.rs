use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

/// Default listening address of the standalone server.
pub const DEFAULT_BIND: &str = "0.0.0.0:7878";

/// Server settings. Build one with [`ServerConfig::default`], [`ServerConfig::from_env`] or
/// [`ServerConfig::for_host_mode`], then adjust public fields if needed. More fields may be
/// added in the future: prefer `ServerConfig { bind, ..ServerConfig::for_host_mode(bind) }`
/// to spelling out every field.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// Address to listen on; port 0 picks a free port (see `ServerHandle::local_addr`).
    pub bind: SocketAddr,
    /// Simultaneous WebSocket connections; further upgrades are refused with HTTP 503.
    pub max_connections: usize,
    /// Simultaneous rooms; a `create_room` beyond it is answered with `invalid_state`.
    pub max_rooms: usize,
    /// Messages per second a connection may send in the long run (token bucket refill).
    pub msg_rate_per_s: u32,
    /// Messages a connection may send in a burst (token bucket size).
    pub msg_burst: u32,
    /// A socket that has not completed `hello` after this long is closed.
    pub handshake_timeout: Duration,
    /// The server pings every connection this often.
    pub ping_interval: Duration,
    /// A connection that sent nothing (not even a pong) for this long is dropped, which the
    /// engine treats as a disconnection (60 s resume window).
    pub idle_timeout: Duration,
    /// **Test hook.** Game time runs this many times faster than the wall clock, so tests can
    /// play 60 s rounds, 8 s intermissions and the 60 s resume window in milliseconds.
    /// Leave at 1 in production; transport timeouts and rate limits are not scaled.
    #[doc(hidden)]
    pub time_scale: u32,
}

impl Default for ServerConfig {
    /// Standalone server: `0.0.0.0:7878`, 2000 connections, 500 rooms.
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.parse().expect("valid default address"),
            max_connections: 2000,
            max_rooms: 500,
            msg_rate_per_s: 10,
            msg_burst: 20,
            handshake_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(60),
            time_scale: 1,
        }
    }
}

impl ServerConfig {
    /// Small caps for a server embedded in a player's app: a few rooms' worth of
    /// connections (16 connections, 4 rooms). Use `127.0.0.1:0` with a tunnel, `0.0.0.0:<port>`
    /// for LAN play.
    pub fn for_host_mode(bind: SocketAddr) -> Self {
        Self {
            bind,
            max_connections: 16,
            max_rooms: 4,
            ..Self::default()
        }
    }

    /// Read `BOUSSOLE_BIND`, `BOUSSOLE_MAX_CONNECTIONS` and `BOUSSOLE_MAX_ROOMS` from the
    /// environment; unset variables keep their [`Default`] value, malformed ones are an error.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut cfg = Self::default();
        if let Some(v) = get("BOUSSOLE_BIND") {
            cfg.bind = parse("BOUSSOLE_BIND", &v, "a socket address like 0.0.0.0:7878")?;
        }
        if let Some(v) = get("BOUSSOLE_MAX_CONNECTIONS") {
            cfg.max_connections = parse_positive("BOUSSOLE_MAX_CONNECTIONS", &v)?;
        }
        if let Some(v) = get("BOUSSOLE_MAX_ROOMS") {
            cfg.max_rooms = parse_positive("BOUSSOLE_MAX_ROOMS", &v)?;
        }
        Ok(cfg)
    }
}

fn parse<T: std::str::FromStr>(
    var: &'static str,
    raw: &str,
    expected: &'static str,
) -> Result<T, ConfigError> {
    raw.trim().parse().map_err(|_| ConfigError {
        var,
        value: raw.to_owned(),
        expected,
    })
}

fn parse_positive(var: &'static str, raw: &str) -> Result<usize, ConfigError> {
    let n: usize = parse(var, raw, "a positive integer")?;
    if n == 0 {
        return Err(ConfigError {
            var,
            value: raw.to_owned(),
            expected: "a positive integer",
        });
    }
    Ok(n)
}

/// A configuration variable holds something unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub var: &'static str,
    pub value: String,
    pub expected: &'static str,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}={:?} is invalid: expected {}",
            self.var, self.value, self.expected
        )
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_match_the_documentation() {
        let d = ServerConfig::default();
        assert_eq!(d.bind, "0.0.0.0:7878".parse().unwrap());
        assert_eq!((d.max_connections, d.max_rooms), (2000, 500));
        assert_eq!(ServerConfig::from_lookup(env(&[])).unwrap(), d);
    }

    #[test]
    fn host_mode_has_small_caps_and_keeps_the_bind() {
        let c = ServerConfig::for_host_mode("127.0.0.1:0".parse().unwrap());
        assert_eq!(c.bind, "127.0.0.1:0".parse().unwrap());
        assert_eq!((c.max_connections, c.max_rooms), (16, 4));
        assert_eq!(c.time_scale, 1);
    }

    #[test]
    fn env_overrides_and_rejects_garbage() {
        let c = ServerConfig::from_lookup(env(&[
            ("BOUSSOLE_BIND", " 127.0.0.1:9000 "),
            ("BOUSSOLE_MAX_CONNECTIONS", "12"),
            ("BOUSSOLE_MAX_ROOMS", "3"),
        ]))
        .unwrap();
        assert_eq!(c.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!((c.max_connections, c.max_rooms), (12, 3));

        for (var, bad) in [
            ("BOUSSOLE_BIND", "nope"),
            ("BOUSSOLE_BIND", "7878"),
            ("BOUSSOLE_MAX_CONNECTIONS", "-1"),
            ("BOUSSOLE_MAX_CONNECTIONS", "0"),
            ("BOUSSOLE_MAX_ROOMS", "many"),
        ] {
            let err = ServerConfig::from_lookup(env(&[(var, bad)])).unwrap_err();
            assert_eq!(err.var, var);
            assert!(err.to_string().contains(bad));
        }
    }
}
