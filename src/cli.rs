use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 15353;

/// CLI for managing BifrostDNS records and cache.
#[derive(Parser)]
#[command(name = "bifrost-dns")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Add a DNS record
    Add {
        /// Domain name (e.g. "example.com.")
        name: String,
        /// Record type (A, AAAA, CNAME, MX, TXT, NS, SRV)
        #[clap(rename_all = "UPPERCASE")]
        rtype: String,
        /// Record data (e.g. "192.168.1.1" or "10 mail.example.com.")
        data: String,
        /// TTL in seconds
        #[arg(long, default_value = "3600")]
        ttl: u32,
    },
    /// List DNS records
    List {
        /// Filter by name
        #[arg(long)]
        name: Option<String>,
        /// Filter by type
        #[arg(long = "type")]
        rtype: Option<String>,
    },
    /// Delete a DNS record by ID, or by filter
    Delete {
        /// Record ID to delete (use --name/--type for bulk delete instead)
        id: Option<String>,
        /// Filter by name for bulk delete
        #[arg(long)]
        name: Option<String>,
        /// Filter by type for bulk delete
        #[arg(long = "type")]
        rtype: Option<String>,
    },
    /// Check if BifrostDNS is running
    Health,
    /// Flush the fallback DNS cache (all, or for a specific domain)
    Flush {
        /// Domain name to flush (omit to flush everything)
        name: Option<String>,
    },
}

/// Connection config for the CLI.
struct Connection {
    host: String,
    port: u16,
}

impl Connection {
    fn from_env() -> Self {
        Self {
            host: std::env::var("BIFROST_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string()),
            port: std::env::var("BIFROST_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
        }
    }

    fn base_url(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Minimal HTTP client using raw TCP. Returns (status_code, body).
async fn http_request(
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("failed to connect to {addr}: {e}"))?;

    let content_len = body.map(|b| b.len()).unwrap_or(0);
    let request = match body {
        Some(b) => format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {content_len}\r\nConnection: close\r\n\r\n{b}"
        ),
        None => format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ),
    };

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("failed to send request: {e}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| format!("failed to read response: {e}"))?;

    let response_str =
        String::from_utf8(response).map_err(|e| format!("invalid UTF-8 response: {e}"))?;

    // Parse status code from first line: "HTTP/1.1 200 OK"
    let status_code = response_str
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("failed to parse HTTP status")?;

    // Extract body (everything after the blank line separating headers from body)
    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .to_string();

    // Handle chunked transfer encoding (simple case: single chunk)
    let body = if response_str.contains("transfer-encoding: chunked") {
        parse_chunked_body(&body)
    } else {
        body
    };

    Ok((status_code, body))
}

/// Simple chunked transfer encoding parser (handles single-chunk responses).
fn parse_chunked_body(raw: &str) -> String {
    // Format: "<hex_size>\r\n<data>\r\n0\r\n\r\n"
    // We just want the data between the first and second \r\n.
    let mut result = String::new();
    let mut remaining = raw;
    while !remaining.is_empty() {
        let line_end = match remaining.find("\r\n") {
            Some(pos) => pos,
            None => break,
        };
        let size_str = &remaining[..line_end];
        let size = match usize::from_str_radix(size_str.trim(), 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        if size == 0 {
            break;
        }
        let data_start = line_end + 2;
        if data_start + size <= remaining.len() {
            result.push_str(&remaining[data_start..data_start + size]);
        }
        remaining = &remaining[data_start + size + 2..];
    }
    result
}

pub async fn run() {
    let cli = Cli::parse();
    let conn = Connection::from_env();

    match cli.command {
        Command::Add {
            name,
            rtype,
            data,
            ttl,
        } => {
            let body =
                format!(r#"{{"name":"{name}","type":"{rtype}","ttl":{ttl},"data":"{data}"}}"#);
            match http_request(&conn.base_url(), "POST", "/api/v1/records", Some(&body)).await {
                Ok((201, body)) => {
                    if let Ok(record) = serde_json::from_str::<serde_json::Value>(&body) {
                        let id = record["id"].as_str().unwrap_or("?");
                        let name = record["name"].as_str().unwrap_or("?");
                        let rtype = record["type"].as_str().unwrap_or("?");
                        let data = record["data"].as_str().unwrap_or("?");
                        println!(
                            "Created: {id}  {name}  {rtype}  TTL {}  {data}",
                            record["ttl"]
                        );
                    } else {
                        println!("Record created.");
                    }
                }
                Ok((status, body)) => {
                    eprintln!("Error (HTTP {status}): {body}");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    eprintln!("Is BifrostDNS running on {}?", conn.base_url());
                    std::process::exit(1);
                }
            }
        }
        Command::List { name, rtype } => {
            let mut path = "/api/v1/records".to_string();
            let mut params = Vec::new();
            if let Some(n) = &name {
                params.push(format!("name={n}"));
            }
            if let Some(t) = &rtype {
                params.push(format!("type={t}"));
            }
            if !params.is_empty() {
                path.push('?');
                path.push_str(&params.join("&"));
            }

            match http_request(&conn.base_url(), "GET", &path, None).await {
                Ok((200, body)) => match serde_json::from_str::<Vec<serde_json::Value>>(&body) {
                    Ok(records) if records.is_empty() => {
                        println!("No records found.");
                    }
                    Ok(records) => {
                        println!(
                            "{:<38}  {:<30}  {:>6}  {:<8}  DATA",
                            "ID", "NAME", "TYPE", "TTL"
                        );
                        println!("{}", "-".repeat(100));
                        for r in records {
                            println!(
                                "{:<38}  {:<30}  {:>6}  {:<8}  {}",
                                r["id"].as_str().unwrap_or("?"),
                                r["name"].as_str().unwrap_or("?"),
                                r["type"].as_str().unwrap_or("?"),
                                r["ttl"].as_i64().unwrap_or(0),
                                r["data"].as_str().unwrap_or("?"),
                            );
                        }
                    }
                    Err(_) => println!("Failed to parse response: {body}"),
                },
                Ok((status, body)) => {
                    eprintln!("Error (HTTP {status}): {body}");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    eprintln!("Is BifrostDNS running on {}?", conn.base_url());
                    std::process::exit(1);
                }
            }
        }
        Command::Delete { id, name, rtype } => {
            if let Some(id) = id {
                let path = format!("/api/v1/records/{id}");
                match http_request(&conn.base_url(), "DELETE", &path, None).await {
                    Ok((204, _)) => println!("Deleted: {id}"),
                    Ok((404, _)) => {
                        eprintln!("Not found: {id}");
                        std::process::exit(1);
                    }
                    Ok((status, body)) => {
                        eprintln!("Error (HTTP {status}): {body}");
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                let mut path = "/api/v1/records".to_string();
                let mut params = Vec::new();
                if let Some(n) = &name {
                    params.push(format!("name={n}"));
                }
                if let Some(t) = &rtype {
                    params.push(format!("type={t}"));
                }
                if !params.is_empty() {
                    path.push('?');
                    path.push_str(&params.join("&"));
                }
                match http_request(&conn.base_url(), "DELETE", &path, None).await {
                    Ok((204, _)) => println!("Deleted matching records."),
                    Ok((status, body)) => {
                        eprintln!("Error (HTTP {status}): {body}");
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Command::Health => match http_request(&conn.base_url(), "GET", "/health", None).await {
            Ok((200, body)) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    let version = v["version"].as_str().unwrap_or("unknown");
                    println!("BifrostDNS is healthy (v{version})");
                } else {
                    println!("BifrostDNS is healthy");
                }
            }
            Ok((status, _)) => {
                eprintln!("BifrostDNS responded with HTTP {status}");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Cannot connect to BifrostDNS on {}: {e}", conn.base_url());
                eprintln!("Is the server running?");
                std::process::exit(1);
            }
        },
        Command::Flush { name } => {
            let path = match &name {
                Some(n) => format!("/api/v1/cache?name={n}"),
                None => "/api/v1/cache".to_string(),
            };
            match http_request(&conn.base_url(), "DELETE", &path, None).await {
                Ok((200, body)) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                        let count = v["flushed"].as_i64().unwrap_or(0);
                        match &name {
                            Some(n) => println!("Flushed {count} entries for {n}"),
                            None => println!("Cache flushed ({count} entries removed)"),
                        }
                    } else {
                        println!("Cache flushed");
                    }
                }
                Ok((status, body)) => {
                    eprintln!("Error (HTTP {status}): {body}");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    eprintln!("Is BifrostDNS running on {}?", conn.base_url());
                    std::process::exit(1);
                }
            }
        }
    }

    // Give tokio a moment to flush output.
    tokio::time::sleep(Duration::from_millis(10)).await;
}
