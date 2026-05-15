//! IP filter for SSRF prevention.

use std::net::IpAddr;

/// CIDR range for IPv4 blocking.
#[derive(Debug, Clone, Copy)]
pub struct CidrRange {
    network: u32,
    mask: u32,
}

impl CidrRange {
    /// Parse a CIDR string like "10.0.0.0/8".
    pub fn v4(s: &str) -> Self {
        let parts: Vec<&str> = s.split('/').collect();
        let ip = parts[0];
        let bits = parts.get(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(32).min(32);

        // Parse IP in network byte order
        let octets: Vec<u32> = ip.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect();
        let a = octets.first().copied().unwrap_or(0);
        let b = octets.get(1).copied().unwrap_or(0);
        let c = octets.get(2).copied().unwrap_or(0);
        let d = octets.get(3).copied().unwrap_or(0);
        let raw_ip = (a << 24) | (b << 16) | (c << 8) | d;
        let shift = 32 - bits;
        let mask = if shift >= 32 { 0 } else { 0xFFFFFFFF_u32.wrapping_shl(shift) };
        // Apply mask to network address so contains() works correctly
        let network = raw_ip & mask;
        Self { network, mask }
    }

    /// Check if an IPv4 address is within this CIDR range.
    pub fn contains(&self, addr: &IpAddr) -> bool {
        match addr {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                let bits = ((octets[0] as u32) << 24)
                    | ((octets[1] as u32) << 16)
                    | ((octets[2] as u32) << 8)
                    | (octets[3] as u32);
                (bits & self.mask) == self.network
            }
            IpAddr::V6(_) => false,
        }
    }
}

/// CIDR range for IPv6 blocking.
#[derive(Debug, Clone, Copy)]
pub struct CidrRangeV6 {
    network: u128,
    mask: u128,
}

impl CidrRangeV6 {
    /// Parse an IPv6 CIDR string like "::1/128" or "fc00::/7".
    pub fn v6(s: &str) -> Self {
        let parts: Vec<&str> = s.split('/').collect();
        let ip_str = parts[0];
        let bits = parts.get(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(128).min(128);

        let addr: std::net::Ipv6Addr = ip_str.parse().unwrap_or(std::net::Ipv6Addr::LOCALHOST);
        let raw_ip = u128::from(addr);
        let shift = 128 - bits;
        let mask = if shift >= 128 { 0 } else { !0u128 << shift };
        let network = raw_ip & mask;
        Self { network, mask }
    }

    /// Check if an IPv6 address is within this CIDR range.
    pub fn contains(&self, addr: &IpAddr) -> bool {
        match addr {
            IpAddr::V6(ipv6) => {
                let bits = u128::from(*ipv6);
                (bits & self.mask) == self.network
            }
            IpAddr::V4(_) => false,
        }
    }
}

/// IP filter for SSRF prevention.
#[derive(Debug, Clone, Default)]
pub struct IpFilter {
    blocked: Vec<CidrRange>,
    blocked_v6: Vec<CidrRangeV6>,
    allowed: Vec<CidrRange>,
    allowed_v6: Vec<CidrRangeV6>,
}

impl IpFilter {
    /// Create an IP filter that blocks private/special IP ranges (IPv4 + IPv6).
    pub fn block_private() -> Self {
        Self {
            blocked: vec![
                // IPv4 loopback
                CidrRange::v4("127.0.0.0/8"),
                // RFC 1918 private
                CidrRange::v4("10.0.0.0/8"),
                CidrRange::v4("172.16.0.0/12"),
                CidrRange::v4("192.168.0.0/16"),
                // Link-local
                CidrRange::v4("169.254.0.0/16"),
                // "This" network
                CidrRange::v4("0.0.0.0/8"),
                // CGNAT
                CidrRange::v4("100.64.0.0/10"),
                // IETF protocol assignments, documentation, TEST-NET
                CidrRange::v4("192.0.0.0/24"),
                CidrRange::v4("192.0.2.0/24"),
                CidrRange::v4("198.51.100.0/24"),
                CidrRange::v4("203.0.113.0/24"),
                // Multicast & reserved
                CidrRange::v4("224.0.0.0/4"),
                CidrRange::v4("240.0.0.0/4"),
            ],
            blocked_v6: vec![
                // IPv6 loopback
                CidrRangeV6::v6("::1/128"),
                // IPv6 ULA (Unique Local Addresses) fc00::/7
                CidrRangeV6::v6("fc00::/7"),
                // IPv6 link-local fe80::/10
                CidrRangeV6::v6("fe80::/10"),
                // IPv4-mapped IPv6 addresses ::ffff:0:0/96
                CidrRangeV6::v6("::ffff:0.0.0.0/96"),
            ],
            allowed: vec![],
            allowed_v6: vec![],
        }
    }

    /// Create an empty IP filter (allows everything).
    pub fn empty() -> Self {
        Self {
            blocked: vec![],
            blocked_v6: vec![],
            allowed: vec![],
            allowed_v6: vec![],
        }
    }

    /// Add an IPv4 CIDR range to the block list.
    pub fn add_block(&mut self, cidr: &str) {
        self.blocked.push(CidrRange::v4(cidr));
    }

    /// Add an IPv4 CIDR range to the allow list.
    pub fn add_allow(&mut self, cidr: &str) {
        self.allowed.push(CidrRange::v4(cidr));
    }

    /// Check if an IP address is allowed.
    pub fn is_allowed(&self, addr: &IpAddr) -> bool {
        // Check IPv4-mapped IPv6 addresses by extracting the IPv4 and checking against v4 blocks
        if let IpAddr::V6(v6) = addr {
            if let Some(v4) = v6.to_ipv4_mapped() {
                // Also check the mapped IPv4 against IPv4 rules
                if !self.is_allowed(&IpAddr::V4(v4)) {
                    return false;
                }
            }
        }

        // Check v4/v6 allow lists
        match addr {
            IpAddr::V4(_) => {
                if self.allowed.iter().any(|r| r.contains(addr)) {
                    return true;
                }
            }
            IpAddr::V6(_) => {
                if self.allowed_v6.iter().any(|r| r.contains(addr)) {
                    return true;
                }
            }
        }

        // Check v4/v6 block lists
        match addr {
            IpAddr::V4(_) => !self.blocked.iter().any(|r| r.contains(addr)),
            IpAddr::V6(_) => {
                // Check v6 blocks and also reject if it maps to a blocked v4
                if self.blocked_v6.iter().any(|r| r.contains(addr)) {
                    return false;
                }
                true
            }
        }
    }

    /// Check if a hostname is allowed by resolving DNS and checking all IPs.
    /// Returns `false` if any resolved IP is blocked.
    /// Returns `true` if DNS resolution fails (fail-open) or all IPs are allowed.
    ///
    /// This is a synchronous version. For async, use `is_hostname_allowed_async`.
    pub fn is_hostname_allowed(&self, hostname: &str) -> bool {
        // Try to parse as IP first (no DNS needed)
        if let Ok(addr) = hostname.parse::<IpAddr>() {
            return self.is_allowed(&addr);
        }

        // Try synchronous DNS resolution via std::net
        // Format as host:0 for lookup
        let lookup_target = format!("{}:0", hostname);
        match std::net::ToSocketAddrs::to_socket_addrs(&lookup_target as &str) {
            Ok(addrs) => {
                for socket_addr in addrs {
                    if !self.is_allowed(&socket_addr.ip()) {
                        return false;
                    }
                }
                true
            }
            Err(_) => {
                // DNS resolution failed — fail open (log in production)
                true
            }
        }
    }

    /// Check if a hostname is allowed by resolving DNS asynchronously.
    #[cfg(feature = "async-dns")]
    pub async fn is_hostname_allowed_async(&self, hostname: &str) -> bool {
        if let Ok(addr) = hostname.parse::<IpAddr>() {
            return self.is_allowed(&addr);
        }

        let lookup_target = format!("{}:0", hostname);
        match tokio::net::lookup_host(&lookup_target).await {
            Ok(addrs) => {
                for socket_addr in addrs {
                    if !self.is_allowed(&socket_addr.ip()) {
                        return false;
                    }
                }
                true
            }
            Err(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_loopback_blocked() {
        let f = IpFilter::block_private();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        assert!(!f.is_allowed(&ip(127, 0, 0, 1)));
        assert!(!f.is_allowed(&ip(127, 255, 255, 255)));
        assert!(!f.is_allowed(&ip(10, 0, 0, 1)));
        assert!(!f.is_allowed(&ip(10, 255, 255, 255)));
        assert!(!f.is_allowed(&ip(172, 16, 0, 1)));
        assert!(!f.is_allowed(&ip(192, 168, 1, 1)));
    }

    #[test]
    fn test_public_allowed() {
        let f = IpFilter::block_private();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        assert!(f.is_allowed(&ip(8, 8, 8, 8)));
        assert!(f.is_allowed(&ip(1, 1, 1, 1)));
    }

    #[test]
    fn test_cidr_range() {
        let cidr = CidrRange::v4("10.0.0.0/8");
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        // 10.255.255.255 is inside 10.0.0.0/8
        assert!(cidr.contains(&ip(10, 255, 255, 255)));
        // 11.0.0.1 is outside 10.0.0.0/8
        assert!(!cidr.contains(&ip(11, 0, 0, 1)));
    }

    #[test]
    fn test_empty_filter_allows() {
        let f = IpFilter::empty();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        assert!(f.is_allowed(&ip(127, 0, 0, 1)));
        assert!(f.is_allowed(&ip(8, 8, 8, 8)));
    }

    #[test]
    fn test_allow_takes_priority() {
        let mut f = IpFilter::empty();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        f.add_block("10.0.0.0/8");
        f.add_allow("10.0.0.5/32");
        assert!(!f.is_allowed(&ip(10, 0, 0, 1)));
        assert!(f.is_allowed(&ip(10, 0, 0, 5)));
    }

    #[test]
    fn test_ipv6_loopback_blocked() {
        let f = IpFilter::block_private();
        assert!(!f.is_allowed(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn test_ipv6_ula_blocked() {
        let f = IpFilter::block_private();
        assert!(!f.is_allowed(&"fc00::1".parse::<IpAddr>().unwrap()));
        assert!(!f.is_allowed(&"fd12:3456::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_ipv6_link_local_blocked() {
        let f = IpFilter::block_private();
        assert!(!f.is_allowed(&"fe80::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_ipv4_mapped_ipv6_blocked() {
        let f = IpFilter::block_private();
        // ::ffff:127.0.0.1 should be blocked (mapped from blocked IPv4)
        let mapped = "::ffff:127.0.0.1".parse::<IpAddr>().unwrap();
        assert!(!f.is_allowed(&mapped));
        // ::ffff:10.0.0.1 should be blocked (mapped from blocked IPv4)
        let mapped2 = "::ffff:10.0.0.1".parse::<IpAddr>().unwrap();
        assert!(!f.is_allowed(&mapped2));
    }

    #[test]
    fn test_ipv6_public_allowed() {
        let f = IpFilter::block_private();
        // Public IPv6 addresses should be allowed
        assert!(f.is_allowed(&"2001:4860:4860::8888".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn test_cidr_mask_applied_to_network() {
        // If someone passes "10.1.2.3/8", the network should be "10.0.0.0"
        let cidr = CidrRange::v4("10.1.2.3/8");
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        assert!(cidr.contains(&ip(10, 0, 0, 1)));
        assert!(cidr.contains(&ip(10, 255, 255, 255)));
        assert!(!cidr.contains(&ip(11, 0, 0, 1)));
    }

    #[test]
    fn test_hostname_ip_literal() {
        let f = IpFilter::block_private();
        assert!(!f.is_hostname_allowed("127.0.0.1"));
        assert!(f.is_hostname_allowed("8.8.8.8"));
    }
}
