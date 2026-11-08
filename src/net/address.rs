//! Turning what a player types or pastes into a WebSocket URL.
//!
//! Accepted: `ws://…` / `wss://…` URLs, `http(s)://…` URLs (what a browser shows for a tunnel),
//! and bare `host:port` / `host` (an IP address, `localhost`, a domain). The result always has a
//! path: `/ws` when none was given.

use std::fmt;

/// Where a server listens when the player gives a host without a port.
pub const DEFAULT_PORT: u16 = 7878;
/// Default address offered in the join form.
pub const DEFAULT_ADDRESS: &str = "ws://localhost:7878/ws";

/// Why an address could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    Empty,
    UnsupportedScheme(String),
    InvalidHost,
    InvalidPort,
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("Saisissez l’adresse du serveur."),
            Self::UnsupportedScheme(scheme) => write!(
                f,
                "Adresse non prise en charge (« {scheme}:// ») : utilisez ws://, wss:// ou https://."
            ),
            Self::InvalidHost => f.write_str("Adresse invalide : le nom de l’hôte est incorrect."),
            Self::InvalidPort => {
                f.write_str("Adresse invalide : le port doit être entre 1 et 65535.")
            }
        }
    }
}

impl std::error::Error for AddressError {}

/// `ws://host:port/ws` or `wss://host/ws` from whatever the player typed.
///
/// * `ws://` / `wss://` URLs are kept; a missing path becomes `/ws`.
/// * `http://` and `https://` become `ws://` and `wss://` (a tunnel URL pasted from a browser).
/// * `host:port` becomes `ws://host:port/ws`; a bare `host` takes port 7878.
/// * Hosts under `trycloudflare.com`, and port 443, are secure (`wss://`, no explicit port).
pub fn normalize_address(raw: &str) -> Result<String, AddressError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AddressError::Empty);
    }

    let (explicit_scheme, rest) = match raw.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, raw),
    };
    let secure_hint = match explicit_scheme.as_deref() {
        Some("ws" | "http") => Some(false),
        Some("wss" | "https") => Some(true),
        Some(other) => return Err(AddressError::UnsupportedScheme(other.to_owned())),
        None => None,
    };

    // authority[/path][?query][#fragment]
    let rest = rest.split('#').next().unwrap_or("");
    let (authority, path_and_query) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = split_host_port(authority)?;

    let tunnel = host.to_ascii_lowercase().ends_with(".trycloudflare.com");
    let secure = secure_hint.unwrap_or(tunnel || port == Some(443));

    let authority = match (port, secure) {
        (Some(443), true) | (None, true) => host.clone(),
        (Some(port), _) => format!("{host}:{port}"),
        (None, false) => format!("{host}:{DEFAULT_PORT}"),
    };
    let path = match path_and_query {
        "" | "/" => "/ws",
        other if other.starts_with('?') => "/ws",
        other => other,
    };
    Ok(format!(
        "{}://{authority}{path}",
        if secure { "wss" } else { "ws" }
    ))
}

fn split_host_port(authority: &str) -> Result<(String, Option<u16>), AddressError> {
    // Drop credentials such as `user@host`: never sent, never kept.
    let authority = authority.rsplit('@').next().unwrap_or("");
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        // [::1]:7878
        let (inside, after) = rest.split_once(']').ok_or(AddressError::InvalidHost)?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or(AddressError::InvalidHost)?),
        };
        if inside.is_empty() || !inside.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
            return Err(AddressError::InvalidHost);
        }
        (format!("[{inside}]"), port)
    } else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        let valid = !host.is_empty()
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-'])
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        if !valid {
            return Err(AddressError::InvalidHost);
        }
        (host.to_owned(), port)
    };
    let port = match port {
        None => None,
        Some(text) => match text.parse::<u16>() {
            Ok(0) | Err(_) => return Err(AddressError::InvalidPort),
            Ok(port) => Some(port),
        },
    };
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        normalize_address(input).unwrap_or_else(|e| panic!("{input:?} should parse: {e}"))
    }

    #[test]
    fn host_and_port_become_a_ws_url() {
        assert_eq!(ok("192.168.1.20:7878"), "ws://192.168.1.20:7878/ws");
        assert_eq!(ok("localhost:9000"), "ws://localhost:9000/ws");
        assert_eq!(
            ok("  mac-de-sam.local:7878  "),
            "ws://mac-de-sam.local:7878/ws"
        );
    }

    #[test]
    fn a_bare_host_takes_the_default_port() {
        assert_eq!(ok("192.168.1.20"), "ws://192.168.1.20:7878/ws");
        assert_eq!(ok("localhost"), "ws://localhost:7878/ws");
    }

    #[test]
    fn ws_and_wss_urls_are_kept() {
        assert_eq!(ok("ws://localhost:7878/ws"), DEFAULT_ADDRESS);
        assert_eq!(ok("wss://jeu.example.org/ws"), "wss://jeu.example.org/ws");
        assert_eq!(ok("WS://Host:1234/ws"), "ws://Host:1234/ws");
        // a custom path is the player's business
        assert_eq!(
            ok("wss://example.org/boussole/ws"),
            "wss://example.org/boussole/ws"
        );
    }

    #[test]
    fn a_missing_path_becomes_slash_ws() {
        assert_eq!(ok("ws://192.168.1.20:7878"), "ws://192.168.1.20:7878/ws");
        assert_eq!(ok("ws://192.168.1.20:7878/"), "ws://192.168.1.20:7878/ws");
        assert_eq!(ok("wss://jeu.example.org"), "wss://jeu.example.org/ws");
    }

    #[test]
    fn a_pasted_tunnel_url_becomes_wss_slash_ws() {
        assert_eq!(
            ok("https://abc-def-ghi.trycloudflare.com"),
            "wss://abc-def-ghi.trycloudflare.com/ws"
        );
        assert_eq!(
            ok("https://abc-def-ghi.trycloudflare.com/"),
            "wss://abc-def-ghi.trycloudflare.com/ws"
        );
        assert_eq!(
            ok("https://abc-def-ghi.trycloudflare.com/ws"),
            "wss://abc-def-ghi.trycloudflare.com/ws"
        );
        // without a scheme, a tunnel host is still secure
        assert_eq!(
            ok("abc-def-ghi.trycloudflare.com"),
            "wss://abc-def-ghi.trycloudflare.com/ws"
        );
        assert_eq!(
            ok("wss://abc-def-ghi.trycloudflare.com/ws"),
            "wss://abc-def-ghi.trycloudflare.com/ws"
        );
    }

    #[test]
    fn http_urls_map_to_ws() {
        assert_eq!(ok("http://192.168.1.20:7878"), "ws://192.168.1.20:7878/ws");
        assert_eq!(ok("https://jeu.example.org"), "wss://jeu.example.org/ws");
    }

    #[test]
    fn port_443_means_a_secure_connection() {
        assert_eq!(ok("jeu.example.org:443"), "wss://jeu.example.org/ws");
        assert_eq!(
            ok("wss://jeu.example.org:443/ws"),
            "wss://jeu.example.org/ws"
        );
        // an explicit ws:// on 443 stays plain
        assert_eq!(
            ok("ws://jeu.example.org:443/ws"),
            "ws://jeu.example.org:443/ws"
        );
    }

    #[test]
    fn ipv6_literals_and_stray_parts() {
        assert_eq!(ok("[::1]:7878"), "ws://[::1]:7878/ws");
        assert_eq!(ok("ws://[fe80::1]:9000/ws"), "ws://[fe80::1]:9000/ws");
        assert_eq!(ok("ws://host:7878/ws#top"), "ws://host:7878/ws");
        assert_eq!(ok("ws://host:7878?x=1"), "ws://host:7878/ws");
        // credentials are dropped, never kept in the URL
        assert_eq!(ok("ws://user:secret@host:7878/ws"), "ws://host:7878/ws");
    }

    #[test]
    fn nonsense_is_refused_with_a_french_message() {
        assert_eq!(normalize_address(""), Err(AddressError::Empty));
        assert_eq!(normalize_address("   "), Err(AddressError::Empty));
        assert_eq!(
            normalize_address("ftp://host"),
            Err(AddressError::UnsupportedScheme("ftp".into()))
        );
        assert_eq!(
            normalize_address("host:abc"),
            Err(AddressError::InvalidPort)
        );
        assert_eq!(normalize_address("host:0"), Err(AddressError::InvalidPort));
        assert_eq!(
            normalize_address("host:70000"),
            Err(AddressError::InvalidPort)
        );
        assert_eq!(
            normalize_address("ho st:7878"),
            Err(AddressError::InvalidHost)
        );
        assert_eq!(normalize_address("ws://"), Err(AddressError::InvalidHost));
        assert_eq!(normalize_address(":7878"), Err(AddressError::InvalidHost));
        assert_eq!(normalize_address("[::1"), Err(AddressError::InvalidHost));
        for error in [
            AddressError::Empty,
            AddressError::UnsupportedScheme("ftp".into()),
            AddressError::InvalidHost,
            AddressError::InvalidPort,
        ] {
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn the_result_is_stable_when_normalised_twice() {
        for input in [
            "192.168.1.20:7878",
            "https://abc.trycloudflare.com",
            "wss://jeu.example.org",
            "localhost",
        ] {
            let once = ok(input);
            assert_eq!(ok(&once), once);
        }
    }
}
