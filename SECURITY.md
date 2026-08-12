# Security Policy

## API Authentication

BifrostDNS does **not** implement API authentication. The HTTP API (default port 15353) allows anyone who can reach it to create, read, and delete DNS records.

**When running as a system DNS resolver** (via `install.sh --set-dns` or manual configuration), this means anyone on your network can inject DNS records that will be served by BifrostDNS.

## Recommendations

### Docker

The API port is not exposed by default in the Docker examples. Only expose it if you understand the risks:

```bash
# Safe: only DNS ports exposed, API is internal
docker run -p 53:53/udp -p 53:53 ghcr.io/minutemailco/bifrost-dns:latest

# Less safe: API exposed (only do this on a trusted network)
docker run -p 53:53/udp -p 53:53 -p 15353:15353 ghcr.io/minutemailco/bifrost-dns:latest
```

### Bare metal / systemd

The systemd unit binds the API to `0.0.0.0:15353` by default. To restrict access:

1. **Bind API to localhost only** — override the unit:
   ```bash
   sudo systemctl edit bifrost-dns
   ```
   This won't work yet since the binary always binds `0.0.0.0`. As a workaround, use a firewall:
   
2. **Firewall the API port**:
   ```bash
   # ufw (Ubuntu/Debian)
   sudo ufw deny 15353

   # iptables
   sudo iptables -A INPUT -p tcp --dport 15353 -j DROP
   ```

### General

- Only run BifrostDNS on trusted networks
- Do not expose the API port to the public internet
- Use network segmentation to isolate BifrostDNS if needed

## Reporting a Vulnerability

If you discover a security issue, please email security@minutemail.co instead of opening a public issue.
