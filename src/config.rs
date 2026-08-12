use std::env;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub dns_port: u16,
    pub api_port: u16,
    pub log_level: String,
    /// Upstream DNS servers to forward queries when no local record matches.
    /// Empty = disabled (returns NXDOMAIN for unknown names).
    pub fallback_dns: Vec<SocketAddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dns_port: 53,
            api_port: 8080,
            log_level: "info".to_string(),
            fallback_dns: Vec::new(),
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(port) = env::var("DNS_PORT") {
            if let Ok(p) = port.parse() {
                config.dns_port = p;
            }
        }

        if let Ok(port) = env::var("API_PORT") {
            if let Ok(p) = port.parse() {
                config.api_port = p;
            }
        }

        if let Ok(level) = env::var("LOG_LEVEL") {
            config.log_level = level;
        }

        if let Ok(raw) = env::var("FALLBACK_DNS") {
            config.fallback_dns = raw
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter_map(|s| {
                    // Allow bare IPs (default port 53) and full socket addrs.
                    // Handle IPv6: bare addresses like "::1" or "fe80::1"
                    // contain colons and must be bracketed before appending port.
                    if s.starts_with('[') || s.matches(':').count() > 1 {
                        // Already bracketed or bare IPv6 — check if port is included.
                        if s.starts_with('[') {
                            s.parse().ok()
                        } else {
                            format!("[{s}]:53").parse().ok()
                        }
                    } else if s.contains(':') {
                        // IPv4:port or hostname:port
                        s.parse().ok()
                    } else {
                        // Bare IPv4 or hostname
                        format!("{s}:53").parse().ok()
                    }
                })
                .collect();
        }

        config
    }

    pub fn fallback_enabled(&self) -> bool {
        !self.fallback_dns.is_empty()
    }
}
