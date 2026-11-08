use std::net::Ipv4Addr;

/// `ws://<ip>:<port>/ws` for every non-loopback IPv4 address of this machine: what a host
/// shares with friends on the same network. Link-local (`169.254.x.x`) and unspecified
/// addresses are skipped too, since nobody can join through them.
///
/// Empty when the machine has no usable network interface. The server must be bound to
/// `0.0.0.0` (or the interface itself) for these addresses to work.
pub fn lan_ws_urls(port: u16) -> Vec<String> {
    let addrs = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(_) => None,
        });
    ws_urls_for(addrs, port)
}

/// The formatting and filtering behind [`lan_ws_urls`], separated so it can be tested with
/// known addresses.
pub(crate) fn ws_urls_for(addrs: impl IntoIterator<Item = Ipv4Addr>, port: u16) -> Vec<String> {
    let mut seen = Vec::new();
    addrs
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local())
        .filter(|ip| {
            let fresh = !seen.contains(ip);
            seen.push(*ip);
            fresh
        })
        .map(|ip| format!("ws://{ip}:{port}/ws"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_drops_loopback_link_local_and_duplicates() {
        let addrs = [
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(192, 168, 1, 23),
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::new(169, 254, 7, 7),
            Ipv4Addr::new(10, 0, 0, 5),
            Ipv4Addr::new(192, 168, 1, 23),
        ];
        assert_eq!(
            ws_urls_for(addrs, 7878),
            ["ws://192.168.1.23:7878/ws", "ws://10.0.0.5:7878/ws"]
        );
        assert!(ws_urls_for([Ipv4Addr::LOCALHOST], 1).is_empty());
    }

    #[test]
    fn real_interfaces_never_yield_loopback() {
        for url in lan_ws_urls(4242) {
            assert!(
                url.starts_with("ws://") && url.ends_with(":4242/ws"),
                "{url}"
            );
            assert!(!url.contains("127."), "{url}");
        }
    }
}
