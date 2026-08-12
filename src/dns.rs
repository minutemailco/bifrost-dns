use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::{
    rdata::{self},
    Name, RData, Record, RecordType as HickoryRecordType,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::cache::SharedCache;
use crate::models::RecordType;
use crate::store::SharedStore;

/// Timeout for each upstream fallback query.
const FALLBACK_TIMEOUT: Duration = Duration::from_secs(2);

/// Holds the upstream DNS servers for fallback forwarding.
/// When a query misses the local store, the raw query bytes are
/// relayed to these servers in order until one responds.
#[derive(Debug, Clone)]
pub struct Fallback {
    servers: Vec<SocketAddr>,
}

impl Fallback {
    pub fn new(servers: Vec<SocketAddr>) -> Self {
        Self { servers }
    }

    pub fn is_enabled(&self) -> bool {
        !self.servers.is_empty()
    }

    /// Forward a raw DNS query to the first upstream server that responds.
    /// Returns the raw response bytes, or `None` if all servers time out.
    pub async fn forward(&self, query: &[u8]) -> Option<Vec<u8>> {
        for server in &self.servers {
            match Self::forward_to(server, query).await {
                Some(resp) => {
                    debug!(
                        "fallback query forwarded to {server} ({} bytes)",
                        resp.len()
                    );
                    return Some(resp);
                }
                None => {
                    warn!("fallback server {server} timed out, trying next");
                }
            }
        }
        None
    }

    /// Send a raw DNS query to a single upstream server via UDP.
    async fn forward_to(server: &SocketAddr, query: &[u8]) -> Option<Vec<u8>> {
        let sock = UdpSocket::bind("0.0.0.0:0").await.ok()?;
        sock.connect(server).await.ok()?;

        if sock.send(query).await.is_err() {
            return None;
        }

        let mut buf = vec![0u8; 4096];
        match timeout(FALLBACK_TIMEOUT, sock.recv(&mut buf)).await {
            Ok(Ok(len)) => {
                buf.truncate(len);
                Some(buf)
            }
            _ => None,
        }
    }
}

/// Map our RecordType enum to hickory's RecordType.
fn to_hickory_type(rtype: RecordType) -> HickoryRecordType {
    match rtype {
        RecordType::A => HickoryRecordType::A,
        RecordType::AAAA => HickoryRecordType::AAAA,
        RecordType::CNAME => HickoryRecordType::CNAME,
        RecordType::MX => HickoryRecordType::MX,
        RecordType::TXT => HickoryRecordType::TXT,
        RecordType::NS => HickoryRecordType::NS,
        RecordType::SRV => HickoryRecordType::SRV,
    }
}

/// Parse RData string into an `RData` enum variant based on record type.
fn build_rdata(record: &crate::models::Record) -> Result<RData, String> {
    match record.record_type {
        RecordType::A => {
            let ip: IpAddr = record
                .data
                .parse()
                .map_err(|e| format!("invalid A data: {e}"))?;
            match ip {
                IpAddr::V4(v4) => Ok(RData::A(rdata::A(v4))),
                _ => Err("expected IPv4 for A record".into()),
            }
        }
        RecordType::AAAA => {
            let ip: IpAddr = record
                .data
                .parse()
                .map_err(|e| format!("invalid AAAA data: {e}"))?;
            match ip {
                IpAddr::V6(v6) => Ok(RData::AAAA(rdata::AAAA(v6))),
                _ => Err("expected IPv6 for AAAA record".into()),
            }
        }
        RecordType::CNAME => {
            let name = Name::from_str(&record.data).map_err(|e| format!("invalid CNAME: {e}"))?;
            Ok(RData::CNAME(rdata::CNAME(name)))
        }
        RecordType::NS => {
            let name = Name::from_str(&record.data).map_err(|e| format!("invalid NS: {e}"))?;
            Ok(RData::NS(rdata::NS(name)))
        }
        RecordType::MX => {
            // Format: "10 mail.example.com."
            let parts: Vec<&str> = record.data.splitn(2, char::is_whitespace).collect();
            if parts.len() != 2 {
                return Err("MX data must be '<priority> <host>'".into());
            }
            let preference = parts[0]
                .parse::<u16>()
                .map_err(|e| format!("invalid MX priority: {e}"))?;
            let exchange =
                Name::from_str(parts[1]).map_err(|e| format!("invalid MX exchange: {e}"))?;
            Ok(RData::MX(rdata::MX::new(preference, exchange)))
        }
        RecordType::TXT => Ok(RData::TXT(rdata::TXT::new(vec![record.data.clone()]))),
        RecordType::SRV => {
            // Format: "10 5 5060 sipserver.example.com."
            let parts: Vec<&str> = record.data.split_whitespace().collect();
            if parts.len() != 4 {
                return Err("SRV data must be '<priority> <weight> <port> <target>'".into());
            }
            let priority = parts[0]
                .parse::<u16>()
                .map_err(|e| format!("invalid SRV priority: {e}"))?;
            let weight = parts[1]
                .parse::<u16>()
                .map_err(|e| format!("invalid SRV weight: {e}"))?;
            let port = parts[2]
                .parse::<u16>()
                .map_err(|e| format!("invalid SRV port: {e}"))?;
            let target =
                Name::from_str(parts[3]).map_err(|e| format!("invalid SRV target: {e}"))?;
            Ok(RData::SRV(rdata::SRV::new(priority, weight, port, target)))
        }
    }
}

/// Build a hickory `Record` from our model.
fn build_record(record: &crate::models::Record) -> Result<Record, String> {
    let name = Name::from_str(&record.name).map_err(|e| format!("invalid name: {e}"))?;
    let rdata = build_rdata(record)?;
    Ok(Record::from_rdata(name, record.ttl, rdata))
}

/// Process a raw DNS query and produce a raw DNS response.
///
/// Query resolution order:
/// 1. Look up in the local mock store → if matches, return them.
/// 2. If no local match and fallback is enabled → forward raw query upstream.
/// 3. Otherwise → return NXDOMAIN (for known types) or NotImp (for unknown types).
async fn handle_query(
    raw: &[u8],
    store: &SharedStore,
    fallback: Option<&Fallback>,
    cache: &SharedCache,
) -> Option<Vec<u8>> {
    let request = Message::from_vec(raw).ok()?;
    let request_id = request.metadata.id;

    // If there are no queries, it's a malformed message.
    if request.queries.is_empty() {
        let mut response =
            Message::new(request_id, MessageType::Response, request.metadata.op_code);
        response.metadata.response_code = ResponseCode::FormErr;
        return response.to_vec().ok();
    }

    let query = &request.queries[0];
    let name = query.name().to_string();
    let hickory_type = query.query_type();

    // Map to our record type. Unsupported types (PTR, SOA, etc.) can
    // still be forwarded to fallback if configured.
    let our_type = match hickory_type {
        HickoryRecordType::A => Some(RecordType::A),
        HickoryRecordType::AAAA => Some(RecordType::AAAA),
        HickoryRecordType::CNAME => Some(RecordType::CNAME),
        HickoryRecordType::MX => Some(RecordType::MX),
        HickoryRecordType::TXT => Some(RecordType::TXT),
        HickoryRecordType::NS => Some(RecordType::NS),
        HickoryRecordType::SRV => Some(RecordType::SRV),
        _ => None,
    };

    // Step 1: Try the local mock store (only for supported types).
    if let Some(rt) = our_type {
        let records = store.lookup(&name, rt);

        if !records.is_empty() {
            let mut response =
                Message::new(request_id, MessageType::Response, request.metadata.op_code);
            response.metadata.recursion_desired = request.metadata.recursion_desired;
            for q in &request.queries {
                response.add_query(q.clone());
            }

            for record in &records {
                match build_record(record) {
                    Ok(rr) => {
                        response.add_answer(rr);
                    }
                    Err(e) => warn!("failed to build record {}: {}", record.id, e),
                }
            }

            response.metadata.response_code = ResponseCode::NoError;
            debug!(
                "DNS query {} {} -> {} answer(s) [local]",
                name,
                to_hickory_type(rt),
                response.answers.len()
            );
            return response.to_vec().ok();
        }
    }

    // Step 2: No local match — try fallback cache first, then upstream.
    if let Some(fb) = fallback {
        if fb.is_enabled() {
            // Check the fallback cache before hitting upstream.
            if let Some(cached) = cache.get(&name, hickory_type) {
                debug!(
                    "DNS query {} {} -> cache hit [fallback cache]",
                    name, hickory_type
                );
                return patch_response_id(&cached, request_id);
            }

            // Cache miss — forward to upstream.
            debug!(
                "DNS query {} {} -> forwarding to upstream [fallback]",
                name, hickory_type
            );
            if let Some(upstream_resp) = fb.forward(raw).await {
                // Cache the upstream response for future queries.
                cache.put(&name, hickory_type, upstream_resp.clone());
                // Patch the response ID to match the request ID.
                return patch_response_id(&upstream_resp, request_id);
            }
            // All upstream servers failed — fall through to NXDOMAIN.
            warn!("fallback exhausted for {name} {hickory_type}");
        }
    }

    // Step 3: No fallback or fallback failed.
    // Return NotImp for types we don't support locally, NXDOMAIN for
    // supported types that simply had no matching records.
    let mut response = Message::new(request_id, MessageType::Response, request.metadata.op_code);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    for q in &request.queries {
        response.add_query(q.clone());
    }
    response.metadata.response_code = if our_type.is_some() {
        ResponseCode::NXDomain
    } else {
        ResponseCode::NotImp
    };
    response.to_vec().ok()
}

/// Rewrite the transaction ID (first 2 bytes) of a raw DNS response
/// to match the original request. The upstream server generates its
/// own ID; we need to restore the client's.
fn patch_response_id(raw: &[u8], request_id: u16) -> Option<Vec<u8>> {
    if raw.len() < 2 {
        return None;
    }
    let mut out = raw.to_vec();
    let bytes = request_id.to_be_bytes();
    out[0] = bytes[0];
    out[1] = bytes[1];
    Some(out)
}

/// Run the UDP DNS server.
pub async fn run_udp(
    addr: &str,
    store: SharedStore,
    fallback: Option<Fallback>,
    cache: SharedCache,
) -> std::io::Result<()> {
    let sock = Arc::new(UdpSocket::bind(addr).await?);
    tracing::info!("UDP DNS server listening on {addr}");

    let mut buf = [0u8; 4096];

    loop {
        let (len, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!("UDP recv error: {e}");
                continue;
            }
        };

        let data = buf[..len].to_vec();
        let sock = sock.clone();
        let store = store.clone();
        let fb = fallback.clone();
        let cache = cache.clone();

        tokio::spawn(async move {
            if let Some(resp) = handle_query(&data, &store, fb.as_ref(), &cache).await {
                // RFC 1035 §4.2.1: UDP responses should be <= 512 bytes.
                // If exceeded, truncate and set the TC flag so the client
                // knows to retry over TCP.
                let resp = if resp.len() > 512 {
                    let mut msg = match Message::from_vec(&resp) {
                        Ok(m) => m,
                        Err(_) => {
                            warn!("failed to parse response for truncation");
                            return;
                        }
                    };
                    msg.metadata.truncation = true;
                    match msg.to_vec() {
                        Ok(truncated) if truncated.len() <= 512 => truncated,
                        // If still too large (minimal response), hard-truncate.
                        Ok(_) => {
                            let mut v = resp.clone();
                            v.truncate(512);
                            v
                        }
                        Err(e) => {
                            warn!("failed to encode truncated response: {e}");
                            return;
                        }
                    }
                } else {
                    resp
                };
                if let Err(e) = sock.send_to(&resp, peer).await {
                    warn!("UDP send error to {peer}: {e}");
                }
            }
        });
    }
}

/// Run the TCP DNS server.
pub async fn run_tcp(
    addr: &str,
    store: SharedStore,
    fallback: Option<Fallback>,
    cache: SharedCache,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("TCP DNS server listening on {addr}");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("TCP accept error: {e}");
                continue;
            }
        };

        let store = store.clone();
        let fb = fallback.clone();
        let cache = cache.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_tcp_connection(stream, &store, fb.as_ref(), &cache).await {
                debug!("TCP connection from {peer} ended: {e}");
            }
        });
    }
}

/// Handle a single DNS-over-TCP connection.
async fn handle_tcp_connection(
    mut stream: TcpStream,
    store: &SharedStore,
    fallback: Option<&Fallback>,
    cache: &SharedCache,
) -> std::io::Result<()> {
    // DNS over TCP: 2-byte length prefix (big-endian)
    let len = stream.read_u16().await? as usize;

    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    if let Some(resp) = handle_query(&buf, store, fallback, cache).await {
        let resp_len = resp.len() as u16;
        let mut packet = resp_len.to_be_bytes().to_vec();
        packet.extend_from_slice(&resp);
        stream.write_all(&packet).await?;
    }

    Ok(())
}
