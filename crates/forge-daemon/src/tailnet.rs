//! Finding this Mac's Tailscale address.
//!
//! The daemon binds loopback always and the tailnet only when asked. Nothing
//! here reaches the network: it reads the interfaces this machine already has,
//! and an address that is not there yet simply is not found.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use crate::exec;

/// Where macOS keeps it. An absolute path, because the daemon runs under
/// launchd with whatever `PATH` launchd felt like providing.
const IFCONFIG: &str = "/sbin/ifconfig";

/// Reading the interface list is instant or broken.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The carrier-grade NAT range Tailscale hands out (`100.64.0.0/10`).
///
/// An address in this range on one of *this machine's own* interfaces is a
/// tailnet address: nothing else assigns one locally.
pub fn is_tailnet_v4(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && (64..128).contains(&second)
}

/// Tailscale's ULA prefix, `fd7a:115c:a1e0::/48`.
pub fn is_tailnet_v6(address: Ipv6Addr) -> bool {
    let [a, b, c, d, e, f, ..] = address.octets();
    [a, b, c, d, e, f] == [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0]
}

pub fn is_tailnet(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_tailnet_v4(v4),
        IpAddr::V6(v6) => is_tailnet_v6(v6),
    }
}

/// This machine's Tailscale address, if Tailscale is up.
pub async fn address() -> Result<IpAddr, TailnetError> {
    let output = exec::run(IFCONFIG, &["-a"], None, TIMEOUT)
        .await
        .map_err(|source| TailnetError::Ifconfig(source.to_string()))?;

    if !output.success() {
        return Err(TailnetError::Ifconfig(output.first_error_line().to_owned()));
    }

    find(&output.stdout).ok_or(TailnetError::NotUp)
}

/// The first tailnet address in `ifconfig -a` output.
///
/// IPv4 wins over IPv6 whatever the order on screen: it is the address people
/// see in the Tailscale UI and type into the app.
pub fn find(ifconfig: &str) -> Option<IpAddr> {
    let mut v6 = None;

    for address in addresses(ifconfig) {
        match address {
            IpAddr::V4(_) if is_tailnet(address) => return Some(address),
            IpAddr::V6(_) if is_tailnet(address) && v6.is_none() => v6 = Some(address),
            _ => {}
        }
    }

    v6
}

/// Every address `ifconfig` reported, in the order it reported them.
///
/// The interface each belongs to is deliberately ignored. Tailscale's utun
/// index is not stable across reboots, and the App Store build uses a
/// NetworkExtension tunnel with a name of its own, so the address range is the
/// reliable half of the answer.
fn addresses(ifconfig: &str) -> impl Iterator<Item = IpAddr> + '_ {
    ifconfig.lines().filter_map(|line| {
        let mut words = line.split_whitespace();

        match words.next()? {
            "inet" => words.next()?.parse().ok().map(IpAddr::V4),
            // `inet6 fe80::1%utun4/64` — the scope and prefix are not part of
            // the address.
            "inet6" => {
                let literal = words.next()?;
                let literal = literal.split(['%', '/']).next()?;
                literal.parse().ok().map(IpAddr::V6)
            }
            _ => None,
        }
    })
}

#[derive(Debug, thiserror::Error)]
pub enum TailnetError {
    #[error("cannot read this Mac's network interfaces: {0}")]
    Ifconfig(String),
    #[error(
        "no Tailscale address on this Mac. Is Tailscale running and logged in? \
         Set tailscale_bind to an explicit address to bind something else."
    )]
    NotUp,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `ifconfig -a` on a Mac running Tailscale.
    const IFCONFIG_OUTPUT: &str = "\
lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384
\tinet 127.0.0.1 netmask 0xff000000
\tinet6 ::1 prefixlen 128
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
\tinet 192.168.1.24 netmask 0xffffff00 broadcast 192.168.1.255
\tinet6 fe80::18b:5cff:fe2a:1%en0 prefixlen 64 scopeid 0x6
utun4: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1280
\tinet6 fd7a:115c:a1e0::9d01:2b4f prefixlen 128
\tinet 100.101.102.103 --> 100.101.102.103 netmask 0xffffffff
";

    #[test]
    fn the_tailnet_address_is_found_among_the_others() {
        assert_eq!(
            find(IFCONFIG_OUTPUT),
            Some("100.101.102.103".parse().unwrap())
        );
    }

    #[test]
    fn a_mac_without_tailscale_has_no_tailnet_address() {
        let without = IFCONFIG_OUTPUT
            .lines()
            .filter(|line| !line.contains("100.101") && !line.contains("fd7a"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(find(&without), None);
    }

    #[test]
    fn ipv6_answers_when_that_is_all_there_is() {
        let v6_only = IFCONFIG_OUTPUT.replace(
            "\tinet 100.101.102.103 --> 100.101.102.103 netmask 0xffffffff\n",
            "",
        );

        assert_eq!(
            find(&v6_only),
            Some("fd7a:115c:a1e0::9d01:2b4f".parse().unwrap())
        );
    }

    #[test]
    fn the_range_is_the_tailnet_one_and_not_its_neighbours() {
        assert!(is_tailnet_v4("100.64.0.0".parse().unwrap()));
        assert!(is_tailnet_v4("100.127.255.255".parse().unwrap()));
        // 100.63.x and 100.128.x are ordinary public addresses.
        assert!(!is_tailnet_v4("100.63.255.255".parse().unwrap()));
        assert!(!is_tailnet_v4("100.128.0.0".parse().unwrap()));
        assert!(!is_tailnet_v4("192.168.1.24".parse().unwrap()));
    }

    #[test]
    fn the_v6_prefix_is_tailscales_and_not_any_ula() {
        assert!(is_tailnet_v6("fd7a:115c:a1e0::1".parse().unwrap()));
        // A private ULA someone else generated.
        assert!(!is_tailnet_v6("fd00::1".parse().unwrap()));
        assert!(!is_tailnet_v6("fe80::1".parse().unwrap()));
    }

    #[test]
    fn a_scoped_ipv6_literal_loses_its_scope_and_prefix() {
        let scoped = "\tinet6 fd7a:115c:a1e0::5%utun4 prefixlen 128\n";

        assert_eq!(find(scoped), Some("fd7a:115c:a1e0::5".parse().unwrap()));
    }

    #[test]
    fn nothing_in_nothing() {
        assert_eq!(find(""), None);
        assert_eq!(find("garbage that is not ifconfig output"), None);
    }
}
