# BifrostDNS — Mock DNS Server by [MinuteMail.co](https://minutemail.co)

A lightweight mock DNS server for testing. Manage DNS records via a REST API and serve them over UDP + TCP. Built for testing DNS-dependent user flows (domain verification, email routing, etc.) in the [MinuteMail](https://minutemail.co) testing platform.

**Why?** When testing email delivery or domain verification flows, you need to control what DNS records resolve to — without managing real domains. BifrostDNS lets you spin up a DNS server, add records via API, and point your test suite at it.

## Features

- **7 record types**: A, AAAA, CNAME, MX, TXT, NS, SRV
- **UDP + TCP** DNS server
- **REST API** for full CRUD on records
- **CLI** built into the same binary — manage records without `curl`
- **DNS fallback** — forward unknown queries to real upstream DNS servers
- **Fallback cache** — cache upstream responses to avoid slowing down browsing
- **In-memory** — no persistence, no database, fast startup
- **Tiny Docker image** (~5-8 MB, `scratch`-based)
- **Zero external runtime dependencies**
- **One-command install** on Linux with systemd integration

---

> **⚠️ Security:** The HTTP API has no authentication. Do not expose port 15353 to untrusted networks. See [SECURITY.md](SECURITY.md) for details.

---

## Quick Start

### Option 1: Docker

```bash
# Run with default ports (requires port 53 privileges on host)
docker run -p 53:53/udp -p 53:53 -p 15353:15353 \
  ghcr.io/minutemailco/bifrost-dns:latest

# With DNS fallback enabled (forward unknown queries to real DNS)
docker run -p 53:53/udp -p 53:53 -p 15353:15353 \
  -e FALLBACK_DNS=1.1.1.1:53,8.8.8.8:53 \
  ghcr.io/minutemailco/bifrost-dns:latest

# Or run unprivileged on alternate ports
docker run -e DNS_PORT=8053 -p 8053:8053/udp -p 8053:8053 -p 15353:15353 \
  ghcr.io/minutemailco/bifrost-dns:latest
```

### Option 2: Install on Linux (systemd)

```bash
git clone https://github.com/minutemailco/bifrost-dns.git
cd bifrost-dns

# Install binary + systemd service (fallback enabled by default)
sudo ./install.sh
sudo systemctl enable --now bifrost-dns

# Install and also set BifrostDNS as the system DNS resolver
sudo ./install.sh --set-dns

# Install binary only, no systemd service
sudo ./install.sh --no-service
```

The install script will:
1. Build the release binary
2. Install it to `/usr/local/bin/bifrost-dns`
3. Create a systemd service with DNS fallback enabled
4. (Optional) Point your system DNS to `127.0.0.1` — auto-detects `systemd-resolved`, NetworkManager, or `/etc/resolv.conf`

If you don't pass `--set-dns`, the script will prompt you at the end. Either way, it backs up the old config and prints revert instructions.

### Option 3: Run from source

```bash
cargo run --release
```

---

## Configuration

All configuration is via environment variables:

| Env Var | Default | Description |
|---------|---------|-------------|
| `DNS_PORT` | `53` | DNS server port (UDP + TCP) |
| `API_PORT` | `15353` | HTTP API port |
| `LOG_LEVEL` | `info` | Log level: `error`, `warn`, `info`, `debug`, `trace` |
| `FALLBACK_DNS` | *(unset)* | Comma-separated upstream DNS servers for fallback forwarding (e.g. `1.1.1.1:53,8.8.8.8:53`). You can omit the port for bare IPs (`1.1.1.1,8.8.8.8`). When unset, unknown queries return NXDOMAIN. |
| `CACHE_TTL` | `300` | Maximum fallback cache duration in seconds. Per-entry TTL is the minimum of this and the record's actual TTL. |

### DNS Fallback

When `FALLBACK_DNS` is set, BifrostDNS acts as both a mock server **and** a forwarding resolver:

1. **Query arrives** → BifrostDNS checks its in-memory store.
2. **Local hit** → returns the mock record(s) immediately.
3. **Local miss** → checks the fallback cache.
4. **Cache hit** → returns the cached response instantly.
5. **Cache miss** → forwards the query to the first upstream server.
6. **Upstream timeout** → tries the next server (2-second timeout per server).
7. **Response received** → caches it and returns it.
8. **All upstreams fail** → returns NXDOMAIN.

This is essential when running BifrostDNS as a system DNS resolver (outside Docker), where blocking all non-mocked domains would break internet access.

**Example:**

```bash
# Enable fallback to Cloudflare and Google DNS
FALLBACK_DNS=1.1.1.1,8.8.8.8 ./bifrost-dns

# Now mock records work alongside real DNS:
dig @localhost test.example.com A    # → mock record from store
dig @localhost google.com A          # → real IP forwarded from 1.1.1.1
```

---

## Running Without Docker

Running BifrostDNS directly on a Linux machine is straightforward. The `install.sh` script handles everything, but here's what happens under the hood.

### Build

```bash
cargo build --release
# Binary is at target/release/bifrost-dns
```

### Run

```bash
# Minimal — mock only, NXDOMAIN for everything else
./target/release/bifrost-dns

# With fallback — mock records + real DNS for everything else
FALLBACK_DNS=1.1.1.1,8.8.8.8 ./target/release/bifrost-dns

# Custom ports
DNS_PORT=8053 API_PORT=8088 FALLBACK_DNS=1.1.1.1 ./target/release/bifrost-dns
```

### Install as a Systemd Service

The included `install.sh` script:

1. Builds the release binary
2. Installs it to `/usr/local/bin/bifrost-dns`
3. Creates `/etc/systemd/system/bifrost-dns.service` with DNS fallback enabled

```bash
sudo ./install.sh
sudo systemctl enable --now bifrost-dns

# Check status
sudo systemctl status bifrost-dns

# View logs
sudo journalctl -u bifrost-dns -f

# Override configuration (without editing the unit file)
sudo systemctl edit bifrost-dns
# In the drop-in, add:
# [Service]
# Environment=FALLBACK_DNS=9.9.9.9
# Environment=LOG_LEVEL=debug
```

### Set BifrostDNS as Your System DNS Resolver

Once BifrostDNS is running on `127.0.0.1:53`, point your system at it:

**systemd-resolved:**
```bash
# Set the global DNS resolver
sudo resolvectl dns global 127.0.0.1

# Or for a specific interface
sudo resolvectl dns eth0 127.0.0.1
```

**`/etc/resolv.conf` (traditional):**
```bash
# Back up the existing file
sudo cp /etc/resolv.conf /etc/resolv.conf.bak

# Point to BifrostDNS
echo "nameserver 127.0.0.1" | sudo tee /etc/resolv.conf

# Restore later with:
# sudo cp /etc/resolv.conf.bak /etc/resolv.conf
```

**NetworkManager:**
```bash
nmcli connection modify --active <connection-name> ipv4.dns 127.0.0.1
nmcli connection modify --active <connection-name> ipv4.ignore-auto-dns yes
nmcli connection up <connection-name>
```

> **Important:** When BifrostDNS is your system DNS resolver, `FALLBACK_DNS` must be set — otherwise every non-mocked domain returns NXDOMAIN and your machine loses internet access. The `install.sh` script enables fallback by default.

---

## API Reference

Base path: `/api/v1`

### Add a Record

```bash
curl -X POST http://localhost:15353/api/v1/records \
  -H "Content-Type: application/json" \
  -d '{"name":"example.com","type":"A","ttl":3600,"data":"192.168.1.1"}'
```

Response (`201 Created`):

```json
{
  "id": "a1b2c3d4-...",
  "name": "example.com.",
  "type": "A",
  "ttl": 3600,
  "data": "192.168.1.1"
}
```

### List Records

```bash
# All records
curl http://localhost:15353/api/v1/records

# Filter by name and/or type
curl "http://localhost:15353/api/v1/records?name=example.com.&type=A"
```

### Get a Record

```bash
curl http://localhost:15353/api/v1/records/{id}
```

### Delete a Record

```bash
curl -X DELETE http://localhost:15353/api/v1/records/{id}
```

### Delete All (or Filtered) Records

```bash
# Delete all
curl -X DELETE http://localhost:15353/api/v1/records

# Delete filtered
curl -X DELETE "http://localhost:15353/api/v1/records?name=example.com.&type=A"
```

### Health Check

```bash
curl http://localhost:15353/health
# {"status":"ok","version":"0.1.0"}
```

---

## CLI

The `bifrost-dns` binary includes a CLI for managing records and cache without `curl`. It connects to a running BifrostDNS server.

```bash
# Add a record
bifrost-dns add test.example.com A 192.168.1.1 --ttl 3600
bifrost-dns add test.example.com MX "10 mail.example.com."
bifrost-dns add test.example.com TXT "v=spf1 -all"

# List records (with optional filters)
bifrost-dns list
bifrost-dns list --name test.example.com
bifrost-dns list --type A

# Delete by ID
bifrost-dns delete a1b2c3d4-...

# Delete by filter
bifrost-dns delete --name test.example.com
bifrost-dns delete --name test.example.com --type A

# Check server health
bifrost-dns health

# Flush the fallback DNS cache
bifrost-dns flush
```

The CLI reads `BIFROST_HOST` (default `127.0.0.1`) and `BIFROST_PORT` (default `15353`) to find the server:

```bash
BIFROST_PORT=15353 bifrost-dns list
```

---

## Record Data Formats

| Type   | Data Format                           | Example                                |
| ------ | ------------------------------------- | -------------------------------------- |
| A      | IPv4 address                          | `192.168.1.1`                          |
| AAAA   | IPv6 address                          | `::1`                                  |
| CNAME  | Target hostname                       | `target.example.com.`                  |
| NS     | Nameserver hostname                   | `ns1.example.com.`                     |
| MX     | `<priority> <host>`                   | `10 mail.example.com.`                 |
| TXT    | Arbitrary text                        | `v=spf1 include:_spf.example.com ~all` |
| SRV    | `<priority> <weight> <port> <target>` | `10 5 5060 sip.example.com.`           |

---

## Querying DNS

```bash
# Query a mock record
dig @localhost test.example.com A
dig @localhost test.example.com MX
dig @localhost test.example.com TXT

# With fallback enabled, real domains work too
dig @localhost google.com A

# Using nslookup
nslookup test.example.com localhost
```

---

## Development

```bash
# Run tests
cargo test

# Lint
cargo clippy -- -D warnings
cargo fmt --check

# Build release binary
cargo build --release

# Build Docker image
docker build -t bifrost-dns .
```

---

## Versioning

This project uses [Semantic Versioning](https://semver.org/). Git tags (`v0.1.0`, `v1.0.0`, etc.) trigger automated Docker image builds.

## License

[MIT](LICENSE) — © [MinuteMail.co](https://minutemail.co)
