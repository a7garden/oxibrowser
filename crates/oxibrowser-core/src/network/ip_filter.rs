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
        let _n = octets.len();
        let a = octets.first().copied().unwrap_or(0);
        let b = octets.get(1).copied().unwrap_or(0);
        let c = octets.get(2).copied().unwrap_or(0);
        let d = octets.get(3).copied().unwrap_or(0);
        let network = (a << 24) | (b << 16) | (c << 8) | d;
        let shift = 32 - bits;
        let mask = if shift >= 32 { 0 } else { 0xFFFFFFFF_u32.wrapping_shl(shift) };
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

/// IP filter for SSRF prevention.
#[derive(Debug, Clone, Default)]
pub struct IpFilter {
    blocked: Vec<CidrRange>,
    allowed: Vec<CidrRange>,
}

impl IpFilter {
    /// Create an IP filter that blocks private IP ranges.
    pub fn block_private() -> Self {
        Self {
            blocked: vec![
                CidrRange::v4("127.0.0.0/8"),
                CidrRange::v4("10.0.0.0/8"),
                CidrRange::v4("172.16.0.0/12"),
                CidrRange::v4("192.168.0.0/16"),
                CidrRange::v4("169.254.0.0/16"),
                CidrRange::v4("0.0.0.0/8"),
                CidrRange::v4("100.64.0.0/10"),
                CidrRange::v4("192.0.0.0/24"),
                CidrRange::v4("192.0.2.0/24"),
                CidrRange::v4("198.51.100.0/24"),
                CidrRange::v4("203.0.113.0/24"),
                CidrRange::v4("224.0.0.0/4"),
                CidrRange::v4("240.0.0.0/4"),
            ],
            allowed: vec![],
        }
    }

    /// Create an empty IP filter (allows everything).
    pub fn empty() -> Self {
        Self { blocked: vec![], allowed: vec![] }
    }

    /// Add a CIDR range to the block list.
    pub fn add_block(&mut self, cidr: &str) {
        self.blocked.push(CidrRange::v4(cidr));
    }

    /// Add a CIDR range to the allow list.
    pub fn add_allow(&mut self, cidr: &str) {
        self.allowed.push(CidrRange::v4(cidr));
    }

    /// Check if an IP address is allowed.
    pub fn is_allowed(&self, addr: &IpAddr) -> bool {
        if self.allowed.iter().any(|r| r.contains(addr)) {
            return true;
        }
        !self.blocked.iter().any(|r| r.contains(addr))
    }

    /// Check if a hostname is allowed (always true without DNS resolution).
    pub fn is_hostname_allowed(&self, _hostname: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_loopback_blocked() {
        use std::net::Ipv4Addr;
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
        use std::net::Ipv4Addr;
        let f = IpFilter::block_private();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        assert!(f.is_allowed(&ip(8, 8, 8, 8)));
        assert!(f.is_allowed(&ip(1, 1, 1, 1)));
    }

    #[test]
    fn test_cidr_range() {
        use std::net::Ipv4Addr;
        let cidr = CidrRange::v4("10.0.0.0/8");
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        // 10.255.255.255 is inside 10.0.0.0/8
        assert!(cidr.contains(&ip(10, 255, 255, 255)));
        // 11.0.0.1 is outside 10.0.0.0/8
        assert!(!cidr.contains(&ip(11, 0, 0, 1)));
    }

    #[test]
    fn test_empty_filter_allows() {
        use std::net::Ipv4Addr;
        let f = IpFilter::empty();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        assert!(f.is_allowed(&ip(127, 0, 0, 1)));
        assert!(f.is_allowed(&ip(8, 8, 8, 8)));
    }

    #[test]
    fn test_allow_takes_priority() {
        use std::net::Ipv4Addr;
        let mut f = IpFilter::empty();
        let ip = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
        f.add_block("10.0.0.0/8");
        f.add_allow("10.0.0.5/32");
        assert!(!f.is_allowed(&ip(10, 0, 0, 1)));
        assert!(f.is_allowed(&ip(10, 0, 0, 5)));
    }
}
