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
    HTTPS,
    SVCB,
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
            RecordType::HTTPS => write!(f, "HTTPS"),
            RecordType::SVCB => write!(f, "SVCB"),
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
            "HTTPS" => Ok(RecordType::HTTPS),
            "SVCB" => Ok(RecordType::SVCB),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_type_parses_https_case_insensitively() {
        assert_eq!("HTTPS".parse::<RecordType>(), Ok(RecordType::HTTPS));
        assert_eq!("https".parse::<RecordType>(), Ok(RecordType::HTTPS));
    }

    #[test]
    fn record_type_parses_svcb_case_insensitively() {
        assert_eq!("SVCB".parse::<RecordType>(), Ok(RecordType::SVCB));
        assert_eq!("svcb".parse::<RecordType>(), Ok(RecordType::SVCB));
    }

    #[test]
    fn record_type_displays_https_and_svcb() {
        assert_eq!(RecordType::HTTPS.to_string(), "HTTPS");
        assert_eq!(RecordType::SVCB.to_string(), "SVCB");
    }

    #[test]
    fn record_type_round_trips_through_serde() {
        let json = serde_json::to_string(&RecordType::HTTPS).unwrap();
        assert_eq!(json, r#""HTTPS""#);
        let parsed: RecordType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, RecordType::HTTPS);
    }

    #[test]
    fn record_type_rejects_unknown_type() {
        assert!("HTTPSRR".parse::<RecordType>().is_err());
    }
}
