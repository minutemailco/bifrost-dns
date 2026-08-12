use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use hickory_proto::op::Message;
use hickory_proto::rr::RecordType as HickoryRecordType;
use tracing::debug;

/// TTL for caching NXDOMAIN responses (negative caching).
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);

/// A cache entry for a fallback DNS response.
#[derive(Debug)]
struct CacheEntry {
    response: Vec<u8>,
    created_at: Instant,
    ttl: Duration,
}

/// DNS response cache for fallback queries only.
/// Mock records from the local store are never cached.
#[derive(Debug)]
pub struct DnsCache {
    entries: HashMap<(String, HickoryRecordType), CacheEntry>,
    max_ttl: Duration,
}

impl DnsCache {
    pub fn new(max_ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            max_ttl,
        }
    }

    /// Look up a cached response. Returns `None` if not present or expired.
    /// Expired entries are removed on access.
    pub fn get(&mut self, name: &str, query_type: HickoryRecordType) -> Option<Vec<u8>> {
        let key = (name.to_string(), query_type);
        if let Some(entry) = self.entries.get(&key) {
            if entry.created_at.elapsed() < entry.ttl {
                debug!(
                    "cache hit for {} {:?} ({} bytes, age {:?})",
                    name,
                    query_type,
                    entry.response.len(),
                    entry.created_at.elapsed()
                );
                return Some(entry.response.clone());
            }
        }
        // Expired or not found — remove if expired.
        self.entries.remove(&key);
        None
    }

    /// Store a fallback response in the cache. The TTL is derived from
    /// the response's record TTLs, capped at `max_ttl`. NXDOMAIN responses
    /// get a short negative-cache TTL.
    pub fn put(&mut self, name: &str, query_type: HickoryRecordType, response: Vec<u8>) {
        let ttl = self.extract_ttl(&response);
        let key = (name.to_string(), query_type);
        debug!(
            "caching fallback response for {} {:?} (TTL {:?}, {} bytes)",
            name,
            query_type,
            ttl,
            response.len()
        );
        self.entries.insert(
            key,
            CacheEntry {
                response,
                created_at: Instant::now(),
                ttl,
            },
        );
    }

    /// Flush all cached entries. Returns the number of entries removed.
    pub fn flush(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }

    /// Flush cached entries for a specific domain name.
    /// Removes all record types for that name. Returns the count removed.
    pub fn flush_domain(&mut self, name: &str) -> usize {
        let target = if name.ends_with('.') {
            name.to_ascii_lowercase()
        } else {
            format!("{}.", name.to_ascii_lowercase())
        };
        let to_remove: Vec<_> = self
            .entries
            .keys()
            .filter(|(n, _)| *n == target)
            .cloned()
            .collect();
        for key in &to_remove {
            self.entries.remove(key);
        }
        to_remove.len()
    }

    /// Get cache stats.
    pub fn stats(&self) -> CacheStats {
        let size_bytes: usize = self.entries.values().map(|e| e.response.len()).sum();
        CacheStats {
            entries: self.entries.len(),
            size_bytes,
        }
    }

    /// Extract the effective TTL from a DNS response.
    /// Uses the minimum TTL across all answer records, capped at max_ttl.
    /// For NXDOMAIN responses, uses a short negative-cache TTL.
    fn extract_ttl(&self, response: &[u8]) -> Duration {
        if let Ok(msg) = Message::from_vec(response) {
            if msg.answers.is_empty() {
                return NEGATIVE_CACHE_TTL.min(self.max_ttl);
            }
            let min_ttl = msg.answers.iter().map(|r| r.ttl).min().unwrap_or(0);
            let derived = Duration::from_secs(min_ttl as u64);
            derived.min(self.max_ttl)
        } else {
            NEGATIVE_CACHE_TTL
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct CacheStats {
    pub entries: usize,
    pub size_bytes: usize,
}

/// Thread-safe wrapper around `DnsCache`.
#[derive(Clone)]
pub struct SharedCache {
    inner: Arc<RwLock<DnsCache>>,
}

impl SharedCache {
    pub fn new(max_ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(DnsCache::new(max_ttl))),
        }
    }

    pub fn get(&self, name: &str, query_type: HickoryRecordType) -> Option<Vec<u8>> {
        self.inner.write().unwrap().get(name, query_type)
    }

    pub fn put(&self, name: &str, query_type: HickoryRecordType, response: Vec<u8>) {
        self.inner.write().unwrap().put(name, query_type, response);
    }

    pub fn flush(&self) -> usize {
        self.inner.write().unwrap().flush()
    }

    pub fn flush_domain(&self, name: &str) -> usize {
        self.inner.write().unwrap().flush_domain(name)
    }

    pub fn stats(&self) -> CacheStats {
        self.inner.read().unwrap().stats()
    }
}
