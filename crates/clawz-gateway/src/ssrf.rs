//! SSRF protection for outbound HTTP to user-influenced endpoints.
//!
//! Connectors and deploy targets can carry caller-supplied URLs (OAuth token
//! endpoints, API base URLs). A naive host-string check is bypassable via DNS
//! rebinding, so this module provides a custom [`reqwest::dns::Resolve`]r that
//! resolves the hostname and drops any address in a private/loopback/link-local
//! range *before* the connection is made. [`guarded_client`] also disables
//! redirect following so a 3xx cannot redirect into the internal network.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Returns `true` if connecting to `ip` could reach internal infrastructure and
/// therefore must be refused for outbound requests to untrusted hosts.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                // 0.0.0.0/8 "this network"
                || v4.octets()[0] == 0
                // 100.64.0.0/10 carrier-grade NAT
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|m| is_blocked_ip(IpAddr::V4(m)))
        }
    }
}

/// A DNS resolver that filters out blocked (internal) addresses, refusing the
/// lookup entirely when no public address remains.
#[derive(Debug, Clone, Default)]
pub struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            let allowed: Vec<SocketAddr> = resolved.filter(|sa| !is_blocked_ip(sa.ip())).collect();
            if allowed.is_empty() {
                return Err(
                    format!("SSRF guard: refused to connect to non-public host '{host}'").into(),
                );
            }
            let addrs: Addrs = Box::new(allowed.into_iter());
            Ok(addrs)
        })
    }
}

/// Build a reqwest client that refuses connections to internal addresses and
/// does not follow redirects. Use for any request to a caller-influenced URL.
pub fn guarded_client() -> reqwest::Client {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_internal_allows_public() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "{ip} should be blocked");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            assert!(!is_blocked_ip(ip.parse().unwrap()), "{ip} should be allowed");
        }
    }
}
