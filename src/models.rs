use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[allow(clippy::upper_case_acronyms)]
pub enum RecordType {
    A,
    AAAA,
    CNAME,
    MX,
    TXT,
    NS,
    SRV,
}

impl std::fmt::Display for RecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordType::A => write!(f, "A"),
            RecordType::AAAA => write!(f, "AAAA"),
            RecordType::CNAME => write!(f, "CNAME"),
            RecordType::MX => write!(f, "MX"),
            RecordType::TXT => write!(f, "TXT"),
            RecordType::NS => write!(f, "NS"),
            RecordType::SRV => write!(f, "SRV"),
        }
    }
}

impl std::str::FromStr for RecordType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "A" => Ok(RecordType::A),
            "AAAA" => Ok(RecordType::AAAA),
            "CNAME" => Ok(RecordType::CNAME),
            "MX" => Ok(RecordType::MX),
            "TXT" => Ok(RecordType::TXT),
            "NS" => Ok(RecordType::NS),
            "SRV" => Ok(RecordType::SRV),
            other => Err(format!("unsupported record type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub record_type: RecordType,
    pub ttl: u32,
    pub data: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateRecord {
    pub name: String,
    #[serde(rename = "type")]
    pub record_type: RecordType,
    pub ttl: u32,
    pub data: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

/// Normalize a domain name to canonical FQDN form (lowercase, trailing dot).
pub fn normalize_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with('.') {
        lower
    } else {
        format!("{lower}.")
    }
}
