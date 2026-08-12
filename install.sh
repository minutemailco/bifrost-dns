#!/usr/bin/env bash
#
# BifrostDNS — Install script for Linux
#
# Builds the release binary and installs it to /usr/local/bin,
# along with a systemd service unit for running as a daemon.
#
# Usage:
#   sudo ./install.sh              # install with default config
#   sudo ./install.sh --no-service # install binary only, no systemd unit
#   sudo ./install.sh --set-dns    # also point system DNS to 127.0.0.1
#
set -euo pipefail

BINARY_NAME="bifrost-dns"
INSTALL_PREFIX="/usr/local/bin"
SERVICE_FILE="/etc/systemd/system/bifrost-dns.service"
DNS_PORT="${DNS_PORT:-53}"
API_PORT="${API_PORT:-15353}"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*"; exit 1; }

# --- Detect and configure the system DNS resolver ---
configure_system_dns() {
    echo ""

    warn "This will change your system DNS resolver to 127.0.0.1 (BifrostDNS)."
    warn "If BifrostDNS stops or FALLBACK_DNS is disabled, DNS resolution"
    warn "on this machine will break."
    echo ""

    # --- Stop BifrostDNS temporarily so we have a clean state ---
    info "Stopping BifrostDNS for reconfiguration..."
    systemctl stop bifrost-dns 2>/dev/null || true

    # --- Method 1: systemd-resolved ---
    if command -v resolvectl &>/dev/null && systemctl is-active --quiet systemd-resolved 2>/dev/null; then
        info "Configuring systemd-resolved..."

        # Back up resolved.conf if not already backed up.
        local resolved_conf=""
        for candidate in /etc/systemd/resolved.conf /etc/systemd-resolved.conf; do
            if [[ -f "$candidate" ]]; then
                resolved_conf="$candidate"
                break
            fi
        done

        if [[ -n "$resolved_conf" ]]; then
            local backup="${resolved_conf}.bifrost-dns-backup"
            if [[ ! -f "$backup" ]]; then
                cp "$resolved_conf" "$backup"
            fi
        fi

        # Create a drop-in to disable the stub listener on port 53.
        # This frees port 53 for BifrostDNS to bind to 0.0.0.0:53.
        info "Disabling systemd-resolved stub listener on port 53..."
        mkdir -p /etc/systemd/resolved.conf.d
        tee /etc/systemd/resolved.conf.d/bifrost-dns.conf << 'DROPIN' >/dev/null
[Resolve]
DNSStubListener=no
DROPIN

        info "Restarting systemd-resolved..."
        systemctl restart systemd-resolved
        sleep 1

        # Repoint /etc/resolv.conf — the stub at 127.0.0.53 is now dead.
        # Write atomically (temp file + mv) to avoid leaving the system
        # without DNS if the script crashes mid-write.
        if [[ -L /etc/resolv.conf || -f /etc/resolv.conf ]]; then
            if [[ ! -f /etc/resolv.conf.bifrost-dns-backup ]]; then
                cp /etc/resolv.conf /etc/resolv.conf.bifrost-dns-backup
            fi
        fi
        echo "nameserver 127.0.0.1" > /tmp/bifrost-dns-resolv.conf
        mv /tmp/bifrost-dns-resolv.conf /etc/resolv.conf
        info "resolv.conf set to nameserver 127.0.0.1"

        # Start BifrostDNS now that port 53 is free.
        info "Starting BifrostDNS..."
        systemctl start bifrost-dns

        # Verify it came up.
        if systemctl is-active --quiet bifrost-dns; then
            info "BifrostDNS is running on port 53."
        else
            error "BifrostDNS failed to start. Check: sudo journalctl -u bifrost-dns -n 20"
        fi

        echo ""
        info "System DNS set to BifrostDNS (127.0.0.1) via systemd-resolved."
        echo ""
        info "To revert later:"
        echo "    sudo systemctl stop bifrost-dns"
        echo "    sudo rm /etc/systemd/resolved.conf.d/bifrost-dns.conf"
        if [[ -n "$resolved_conf" && -f "${resolved_conf}.bifrost-dns-backup" ]]; then
            echo "    sudo cp ${resolved_conf}.bifrost-dns-backup ${resolved_conf}"
        fi
        if [[ -f /etc/resolv.conf.bifrost-dns-backup ]]; then
            echo "    sudo cp /etc/resolv.conf.bifrost-dns-backup /etc/resolv.conf"
        fi
        echo "    sudo systemctl restart systemd-resolved"
        echo ""
        return
    fi

    # --- Method 2: NetworkManager ---
    if command -v nmcli &>/dev/null && systemctl is-active --quiet NetworkManager 2>/dev/null; then
        info "Configuring NetworkManager..."

        local conn
        conn="$(nmcli -t -f NAME,DEVICE connection show --active | head -1 | cut -d: -f1)"
        if [[ -z "$conn" ]]; then
            warn "No active NetworkManager connection found, falling back to /etc/resolv.conf"
        else
            # Start BifrostDNS.
            info "Starting BifrostDNS..."
            systemctl start bifrost-dns

            nmcli connection modify "$conn" ipv4.dns 127.0.0.1
            nmcli connection modify "$conn" ipv4.ignore-auto-dns yes
            nmcli connection up "$conn" >/dev/null 2>&1 || true
            echo ""
            info "System DNS set to 127.0.0.1 via NetworkManager (connection: ${conn})."
            echo ""
            info "To revert later:"
            echo "    sudo nmcli connection modify \"${conn}\" ipv4.ignore-auto-dns no"
            echo "    sudo nmcli connection modify \"${conn}\" ipv4.dns \"\""
            echo "    sudo nmcli connection up \"${conn}\""
            echo ""
            return
        fi
    fi

    # --- Method 3: /etc/resolv.conf (fallback) ---
    info "Configuring /etc/resolv.conf..."

    if [[ ! -f /etc/resolv.conf.bifrost-dns-backup ]]; then
        cp /etc/resolv.conf /etc/resolv.conf.bifrost-dns-backup
        info "Backed up /etc/resolv.conf to /etc/resolv.conf.bifrost-dns-backup"
    else
        warn "Backup already exists at /etc/resolv.conf.bifrost-dns-backup, not overwriting."
    fi

    # Try to remove immutable flag if set.
    chattr -i /etc/resolv.conf 2>/dev/null || true

    cat > /etc/resolv.conf << 'EOF'
# Managed by BifrostDNS — do not edit manually.
# To revert: sudo cp /etc/resolv.conf.bifrost-dns-backup /etc/resolv.conf
nameserver 127.0.0.1
EOF

    # Start BifrostDNS.
    info "Starting BifrostDNS..."
    systemctl start bifrost-dns

    echo ""
    info "System DNS set to 127.0.0.1 via /etc/resolv.conf."
    echo ""
    info "To revert later:"
    echo "    sudo cp /etc/resolv.conf.bifrost-dns-backup /etc/resolv.conf"
    echo ""
}

# --- Pre-flight checks ---
[[ "$(uname -s)" == "Linux" ]] || error "This install script is for Linux only."
[[ "$(id -u)" -eq 0 ]] || error "Please run as root (use sudo)."

INSTALL_SERVICE=true
SET_SYSTEM_DNS=false
for arg in "$@"; do
    case "$arg" in
        --no-service) INSTALL_SERVICE=false ;;
        --set-dns)    SET_SYSTEM_DNS=true ;;
        *) warn "Unknown argument: $arg" ;;
    esac
done

# --- Locate or install Rust toolchain ---
if ! command -v cargo &>/dev/null; then
    if command -v rustup &>/dev/null; then
        info "Rust is installed via rustup but cargo is not on PATH."
        info "Sourcing cargo environment..."
        # shellcheck disable=SC1091
        source "${HOME}/.cargo/env" 2>/dev/null || true
    fi
fi

if ! command -v cargo &>/dev/null; then
    warn "Rust toolchain not found."
    echo ""
    echo "  BifrostDNS requires Rust to build. Install it with:"
    echo "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo ""
    read -rp "  Install Rust now via rustup? [y/N] " response
    if [[ "$response" =~ ^[Yy]$ ]]; then
        info "Installing Rust via rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck disable=SC1091
        source "${HOME}/.cargo/env"
    else
        error "Rust is required. Aborting."
    fi
fi

# --- Build ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

info "Building release binary..."
cargo build --release

BINARY_PATH="target/release/${BINARY_NAME}"
[[ -f "$BINARY_PATH" ]] || error "Build succeeded but binary not found at ${BINARY_PATH}"

BINARY_SIZE=$(du -h "$BINARY_PATH" | cut -f1)
info "Built ${BINARY_NAME} (${BINARY_SIZE})"

# --- Check that required ports are available ---
check_port() {
    local port="$1"
    local proto="$2"

    # We only care about binds on 0.0.0.0 or * (all interfaces).
    # Loopback-only binds (127.0.0.x) don't conflict with BifrostDNS on 0.0.0.0:port.
    if command -v ss &>/dev/null; then
        if [[ "$proto" == "udp" ]]; then
            # Match lines where the local address is 0.0.0.0:port or *:port
            if ss -lun 2>/dev/null | grep -E "^(UNCONN|ESTAB).*\s(0\.0\.0\.0|\*):${port}\b" >/dev/null; then
                return 1
            fi
        else
            if ss -ltn 2>/dev/null | grep -E "^(LISTEN|ESTAB).*\s(0\.0\.0\.0|\*):${port}\b" >/dev/null; then
                return 1
            fi
        fi
    else
        # Fallback: parse /proc/net — check for 00000000:PORT (all interfaces).
        local proc_file
        [[ "$proto" == "udp" ]] && proc_file="/proc/net/udp" || proc_file="/proc/net/tcp"
        local hex_port
        hex_port=$(printf '%04X' "$port")
        if grep -E "^[[:space:]]*[0-9]+:[[:space:]]*00000000:${hex_port}\b" "$proc_file" 2>/dev/null | grep -qv "0100007F"; then
            return 1
        fi
    fi
    return 0
}

if ! check_port "$DNS_PORT" udp; then
    error "UDP port ${DNS_PORT} is already in use. Stop the conflicting service or set DNS_PORT."
fi
if ! check_port "$DNS_PORT" tcp; then
    error "TCP port ${DNS_PORT} is already in use. Stop the conflicting service or set DNS_PORT."
fi
if ! check_port "$API_PORT" tcp; then
    error "TCP port ${API_PORT} is already in use (API server). Stop the conflicting service or set API_PORT."
fi

info "Ports ${DNS_PORT}/udp, ${DNS_PORT}/tcp, ${API_PORT}/tcp are available."

# --- Install binary ---
info "Installing binary to ${INSTALL_PREFIX}/${BINARY_NAME}"
install -m 0755 "$BINARY_PATH" "${INSTALL_PREFIX}/${BINARY_NAME}"

# --- Install systemd service ---
if [[ "$INSTALL_SERVICE" == "true" ]]; then
    if [[ -f "$SERVICE_FILE" ]]; then
        warn "Systemd service file already exists at ${SERVICE_FILE}, overwriting."
    fi

    info "Installing systemd service to ${SERVICE_FILE}"
    cat > "$SERVICE_FILE" << UNIT
[Unit]
Description=BifrostDNS — Mock DNS Server by MinuteMail.co
Documentation=https://github.com/minutemailco/bifrost-dns
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/bifrost-dns

# --- Configuration via environment variables ---
Environment=DNS_PORT=${DNS_PORT}
Environment=API_PORT=${API_PORT}
Environment=LOG_LEVEL=info

# Fallback DNS servers — when a query misses the mock store, it is
# forwarded to these upstream servers. This is essential when running
# as a system DNS resolver. Set to a comma-separated list.
# To disable fallback, comment out this line.
Environment=FALLBACK_DNS=1.1.1.1:53,8.8.8.8:53

# Run as a dedicated non-root user with only the capability to bind
# to ports < 1024 (port 53).
User=bifrost-dns
Group=bifrost-dns
AmbientCapabilities=CAP_NET_BIND_SERVICE
NoNewPrivileges=true

Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT

    # Create the dedicated user/group if they don't exist.
    if ! id -u bifrost-dns &>/dev/null; then
        info "Creating system user 'bifrost-dns'..."
        useradd --system --no-create-home --shell /usr/sbin/nologin bifrost-dns
    fi

    info "Reloading systemd daemon..."
    systemctl daemon-reload

    info "Enabling BifrostDNS service..."
    systemctl enable bifrost-dns

    # --- Optionally set BifrostDNS as the system DNS resolver ---
    if [[ "$SET_SYSTEM_DNS" == "true" ]]; then
        configure_system_dns
    elif [[ "$INSTALL_SERVICE" == "true" ]]; then
        echo ""
        warn "BifrostDNS is not currently set as your system DNS resolver."
        read -rp "Point this machine's system DNS to 127.0.0.1 now? [y/N] " response
        if [[ "$response" =~ ^[Yy]$ ]]; then
            configure_system_dns
        fi
    fi

    # Start BifrostDNS if not already running (configure_system_dns may
    # have started it already).
    if ! systemctl is-active --quiet bifrost-dns 2>/dev/null; then
        info "Starting BifrostDNS service..."
        systemctl start bifrost-dns
    fi

    echo ""
    info "Installation complete!"
    echo ""
    echo "  BifrostDNS is running."
    echo ""
    echo "  Check status:"
    echo "    sudo systemctl status bifrost-dns"
    echo ""
    echo "  View logs:"
    echo "    sudo journalctl -u bifrost-dns -f"
    echo ""
    echo "  Restart after config changes:"
    echo "    sudo systemctl restart bifrost-dns"
    echo ""
    echo "  Edit configuration:"
    echo "    sudo systemctl edit bifrost-dns"
    echo "    (override Environment= lines in the drop-in file)"
    echo ""
else
    echo ""
    info "Installation complete (binary only, no systemd service)!"
    echo ""
    echo "  Run manually:"
    echo "    ${INSTALL_PREFIX}/${BINARY_NAME}"
    echo ""
    echo "  Or with custom env:"
    echo "    DNS_PORT=53 API_PORT=15353 FALLBACK_DNS=1.1.1.1:53 ${INSTALL_PREFIX}/${BINARY_NAME}"
    echo ""
fi
