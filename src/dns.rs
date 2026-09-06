use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::{
    rdata::{self, svcb},
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
        RecordType::HTTPS => HickoryRecordType::HTTPS,
        RecordType::SVCB => HickoryRecordType::SVCB,
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
        RecordType::HTTPS => Ok(RData::HTTPS(rdata::HTTPS(parse_svcb(&record.data)?))),
        RecordType::SVCB => Ok(RData::SVCB(parse_svcb(&record.data)?)),
    }
}

/// Parse an RFC 9460 presentation-format RDATA string
/// ("SvcPriority TargetName SvcParams...") into an SVCB.
fn parse_svcb(data: &str) -> Result<svcb::SVCB, String> {
    let tokens: Vec<&str> = data.split_whitespace().collect();
    if tokens.len() < 2 {
        return Err("HTTPS/SVCB data must be '<priority> <target> [params...]'".into());
    }

    let svc_priority: u16 = tokens[0]
        .parse()
        .map_err(|e| format!("invalid SvcPriority '{}': {e}", tokens[0]))?;
    let target_name = Name::from_str(tokens[1]).map_err(|e| format!("invalid target name: {e}"))?;

    let mut svc_params: Vec<(svcb::SvcParamKey, svcb::SvcParamValue)> = Vec::new();
    for token in &tokens[2..] {
        let mut key_value = token.splitn(2, '=');
        let key = key_value.next().unwrap_or_default();
        let mut value = key_value.next();
        if let Some(v) = value.as_mut() {
            if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                *v = &v[1..v.len() - 1];
            }
        }

        let param_key: svcb::SvcParamKey = key
            .parse()
            .map_err(|e| format!("invalid SvcParamKey '{key}': {e}"))?;
        if svc_params.iter().any(|(k, _)| *k == param_key) {
            return Err(format!(
                "duplicate SvcParamKey '{key}' (keys MUST NOT be repeated)"
            ));
        }
        let param_value = parse_svc_param(param_key, value)?;
        svc_params.push((param_key, param_value));
    }

    if svc_priority == 0 {
        if !svc_params.is_empty() {
            return Err("SvcPriority 0 (alias mode) must not include SvcParams".into());
        }
        if target_name.is_root() {
            return Err("SvcPriority 0 (alias mode) must not use root ('.') as TargetName".into());
        }
    }

    Ok(svcb::SVCB::new(svc_priority, target_name, svc_params))
}

/// Parse a single SvcParam value in presentation format, per key.
fn parse_svc_param(
    key: svcb::SvcParamKey,
    value: Option<&str>,
) -> Result<svcb::SvcParamValue, String> {
    use svcb::SvcParamKey as K;
    use svcb::SvcParamValue as V;

    match key {
        K::Mandatory => {
            let value = value.ok_or("mandatory requires a comma-separated key list")?;
            let keys = value
                .split(',')
                .map(|k| {
                    k.parse::<svcb::SvcParamKey>()
                        .map_err(|e| format!("invalid mandatory key '{k}': {e}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if keys.is_empty() {
                return Err("mandatory requires at least one key".into());
            }
            Ok(V::Mandatory(svcb::Mandatory(keys)))
        }
        K::Alpn => {
            let value = value.ok_or("alpn requires a comma-separated protocol list")?;
            let ids = value.split(',').map(str::to_owned).collect::<Vec<_>>();
            if ids.is_empty() {
                return Err("alpn requires at least one protocol identifier".into());
            }
            Ok(V::Alpn(svcb::Alpn(ids)))
        }
        K::NoDefaultAlpn => {
            if value.is_some_and(|v| !v.is_empty()) {
                return Err("no-default-alpn must not have a value".into());
            }
            Ok(V::NoDefaultAlpn)
        }
        K::Port => {
            let value = value.ok_or("port requires a numeric value")?;
            let port: u16 = value
                .parse()
                .map_err(|e| format!("invalid port '{value}': {e}"))?;
            Ok(V::Port(port))
        }
        K::Ipv4Hint => {
            let value = value.ok_or("ipv4hint requires a comma-separated address list")?;
            let ips = value
                .split(',')
                .map(|ip| {
                    ip.parse::<std::net::Ipv4Addr>()
                        .map(rdata::A)
                        .map_err(|e| format!("invalid ipv4hint address '{ip}': {e}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(V::Ipv4Hint(svcb::IpHint(ips)))
        }
        K::Ipv6Hint => {
            let value = value.ok_or("ipv6hint requires a comma-separated address list")?;
            let ips = value
                .split(',')
                .map(|ip| {
                    ip.parse::<std::net::Ipv6Addr>()
                        .map(rdata::AAAA)
                        .map_err(|e| format!("invalid ipv6hint address '{ip}': {e}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(V::Ipv6Hint(svcb::IpHint(ips)))
        }
        K::EchConfigList => {
            let value = value.ok_or("ech requires a base64-encoded ECHConfigList")?;
            let bytes = data_encoding::BASE64
                .decode(value.as_bytes())
                .map_err(|e| format!("invalid base64 in ech: {e}"))?;
            Ok(V::EchConfigList(svcb::EchConfigList(bytes)))
        }
        K::Key(_) | K::Key65535 | K::Unknown(_) => {
            // Unknown keys use the raw value bytes as their wire format.
            Ok(V::Unknown(svcb::Unknown(
                value.unwrap_or("").as_bytes().to_vec(),
            )))
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
        HickoryRecordType::HTTPS => Some(RecordType::HTTPS),
        HickoryRecordType::SVCB => Some(RecordType::SVCB),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Record;
    use hickory_proto::rr::rdata::svcb::{
        Alpn, EchConfigList, IpHint, SvcParamKey, SvcParamValue, SVCB,
    };

    fn record(rtype: RecordType, data: &str) -> Record {
        Record {
            id: "test".into(),
            name: "example.com.".into(),
            record_type: rtype,
            ttl: 300,
            data: data.into(),
        }
    }

    fn https_svcb(data: &str) -> SVCB {
        match build_rdata(&record(RecordType::HTTPS, data)).expect("rdata should parse") {
            RData::HTTPS(h) => h.0,
            other => panic!("expected HTTPS rdata, got {other:?}"),
        }
    }

    fn find_param(svcb: &SVCB, key: SvcParamKey) -> &SvcParamValue {
        svcb.svc_params
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("param {key:?} not found"))
    }

    #[test]
    fn svcb_parses_priority_and_target() {
        let svcb = https_svcb("1 svc.example.com. alpn=h2");
        assert_eq!(svcb.svc_priority, 1);
        assert_eq!(svcb.target_name.to_string(), "svc.example.com.");
    }

    #[test]
    fn svcb_parses_alpn_list() {
        let svcb = https_svcb("1 . alpn=h2,h3");
        match find_param(&svcb, SvcParamKey::Alpn) {
            SvcParamValue::Alpn(Alpn(ids)) => {
                assert_eq!(ids, &vec!["h2".to_string(), "h3".to_string()])
            }
            other => panic!("expected alpn, got {other:?}"),
        }
    }

    #[test]
    fn svcb_parses_ech_base64() {
        // "aGVsbG8=" is base64 for b"hello"
        let svcb = https_svcb("1 . ech=aGVsbG8=");
        match find_param(&svcb, SvcParamKey::EchConfigList) {
            SvcParamValue::EchConfigList(EchConfigList(bytes)) => {
                assert_eq!(bytes, &b"hello".to_vec())
            }
            other => panic!("expected ech, got {other:?}"),
        }
    }

    #[test]
    fn svcb_parses_port() {
        let svcb = https_svcb("1 . port=8002");
        match find_param(&svcb, SvcParamKey::Port) {
            SvcParamValue::Port(p) => assert_eq!(*p, 8002),
            other => panic!("expected port, got {other:?}"),
        }
    }

    #[test]
    fn svcb_parses_ipv4_and_ipv6_hints() {
        let svcb = https_svcb("1 . ipv4hint=192.0.2.1,192.0.2.2 ipv6hint=2001:db8::1");
        match find_param(&svcb, SvcParamKey::Ipv4Hint) {
            SvcParamValue::Ipv4Hint(IpHint(ips)) => {
                assert_eq!(ips.len(), 2);
                assert_eq!(ips[0].0.to_string(), "192.0.2.1");
                assert_eq!(ips[1].0.to_string(), "192.0.2.2");
            }
            other => panic!("expected ipv4hint, got {other:?}"),
        }
        match find_param(&svcb, SvcParamKey::Ipv6Hint) {
            SvcParamValue::Ipv6Hint(IpHint(ips)) => {
                assert_eq!(ips.len(), 1);
                assert_eq!(ips[0].0.to_string(), "2001:db8::1");
            }
            other => panic!("expected ipv6hint, got {other:?}"),
        }
    }

    #[test]
    fn svcb_parses_mandatory_and_no_default_alpn() {
        let svcb = https_svcb("2 . alpn=h2 no-default-alpn mandatory=alpn");
        match find_param(&svcb, SvcParamKey::Mandatory) {
            SvcParamValue::Mandatory(keys) => {
                assert!(matches!(keys.0.as_slice(), [SvcParamKey::Alpn]))
            }
            other => panic!("expected mandatory, got {other:?}"),
        }
        assert!(matches!(
            find_param(&svcb, SvcParamKey::NoDefaultAlpn),
            SvcParamValue::NoDefaultAlpn
        ));
    }

    #[test]
    fn svcb_parses_unknown_key_as_wire_bytes() {
        let svcb = https_svcb("1 . key65333=ex1");
        assert!(svcb
            .svc_params
            .iter()
            .any(|(k, v)| matches!(k, SvcParamKey::Key(65333))
                && matches!(v, SvcParamValue::Unknown(u) if u.0 == b"ex1".to_vec())));
    }

    #[test]
    fn svcb_builds_svcb_record_type() {
        match build_rdata(&record(RecordType::SVCB, "1 . alpn=h2")).expect("should parse") {
            RData::SVCB(_) => {}
            other => panic!("expected SVCB rdata, got {other:?}"),
        }
    }

    #[test]
    fn svcb_rejects_duplicate_keys() {
        let err = build_rdata(&record(RecordType::HTTPS, "1 . alpn=h2 alpn=h3"))
            .expect_err("duplicate keys must be rejected");
        assert!(err.contains("duplicate"), "unexpected error: {err}");
    }

    #[test]
    fn svcb_rejects_alias_mode_with_params() {
        let err = build_rdata(&record(RecordType::HTTPS, "0 foo.example.com. alpn=h2"))
            .expect_err("alias mode with params must be rejected");
        assert!(err.contains("alias"), "unexpected error: {err}");
    }

    #[test]
    fn svcb_rejects_alias_mode_with_root_target() {
        let err = build_rdata(&record(RecordType::HTTPS, "0 ."))
            .expect_err("alias mode with root target must be rejected");
        assert!(err.contains("alias"), "unexpected error: {err}");
    }

    #[test]
    fn svcb_rejects_bad_priority() {
        assert!(build_rdata(&record(RecordType::HTTPS, "x . alpn=h2")).is_err());
    }

    #[test]
    fn svcb_rejects_bad_base64_ech() {
        assert!(build_rdata(&record(RecordType::HTTPS, "1 . ech=!!!")).is_err());
    }

    #[test]
    fn svcb_wire_round_trip() {
        let rr = build_record(&record(RecordType::HTTPS, "1 . alpn=h2,h3 ech=aGVsbG8="))
            .expect("record should build");
        let mut msg = Message::new(0, MessageType::Response, hickory_proto::op::OpCode::Query);
        msg.add_answer(rr);
        let bytes = msg.to_vec().expect("should encode");
        let decoded = Message::from_vec(&bytes).expect("should decode");
        match &decoded.answers[0].data {
            RData::HTTPS(h) => {
                assert_eq!(h.0.svc_priority, 1);
                assert!(matches!(
                    find_param(&h.0, SvcParamKey::Alpn),
                    SvcParamValue::Alpn(Alpn(ids)) if ids == &vec!["h2".to_string(), "h3".to_string()]
                ));
                assert!(matches!(
                    find_param(&h.0, SvcParamKey::EchConfigList),
                    SvcParamValue::EchConfigList(EchConfigList(b)) if b == &b"hello".to_vec()
                ));
            }
            other => panic!("expected HTTPS rdata after round trip, got {other:?}"),
        }
    }

    #[test]
    fn svcb_maps_to_hickory_types() {
        assert_eq!(to_hickory_type(RecordType::HTTPS), HickoryRecordType::HTTPS);
        assert_eq!(to_hickory_type(RecordType::SVCB), HickoryRecordType::SVCB);
    }
}
