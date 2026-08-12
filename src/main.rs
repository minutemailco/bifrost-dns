mod api;
mod config;
mod dns;
mod models;
mod store;

use config::Config;
use dns::Fallback;
use store::SharedStore;
use tracing_subscriber::EnvFilter;

/// Wait for a SIGTERM signal (Unix only).
#[cfg(unix)]
async fn terminate_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    term.recv().await;
}

#[cfg(not(unix))]
async fn terminate_signal() {
    std::future::pending::<()>().await;
}

#[tokio::main]
async fn main() {
    let config = Config::from_env();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("BifrostDNS v{} starting up", env!("CARGO_PKG_VERSION"));

    if config.fallback_enabled() {
        tracing::info!(
            "DNS fallback enabled: {}",
            config
                .fallback_dns
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        tracing::info!("DNS fallback disabled (unknown domains will return NXDOMAIN)");
    }

    let store = SharedStore::new();
    let fallback = if config.fallback_enabled() {
        Some(Fallback::new(config.fallback_dns.clone()))
    } else {
        None
    };

    let dns_addr = format!("0.0.0.0:{}", config.dns_port);
    let api_addr = format!("0.0.0.0:{}", config.api_port);

    // Spawn DNS servers (UDP + TCP)
    let udp_store = store.clone();
    let udp_addr = dns_addr.clone();
    let udp_fallback = fallback.clone();
    let udp_handle = tokio::spawn(async move {
        if let Err(e) = dns::run_udp(&udp_addr, udp_store, udp_fallback).await {
            tracing::error!("UDP DNS server error: {e}");
        }
    });

    let tcp_store = store.clone();
    let tcp_addr = dns_addr.clone();
    let tcp_fallback = fallback.clone();
    let tcp_handle = tokio::spawn(async move {
        if let Err(e) = dns::run_tcp(&tcp_addr, tcp_store, tcp_fallback).await {
            tracing::error!("TCP DNS server error: {e}");
        }
    });

    // Spawn API server
    let api_store = store.clone();
    let app = api::router(api_store);
    let listener = match tokio::net::TcpListener::bind(&api_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind API server on {api_addr}: {e}");
            return;
        }
    };
    tracing::info!("HTTP API server listening on {api_addr}");

    let api_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("API server error: {e}");
        }
    });

    // Wait for shutdown signal (SIGINT or SIGTERM).
    // SIGTERM is what Docker and systemd send by default.
    let shutdown = async {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
            }
            _ = terminate_signal() => {
                tracing::info!("received SIGTERM, shutting down");
            }
        }
    };
    shutdown.await;

    udp_handle.abort();
    tcp_handle.abort();
    api_handle.abort();
}
