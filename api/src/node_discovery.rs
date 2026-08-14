//! Automatic node gateway discovery
//!
//! Implements the Nox node discovery protocol with four strategies:
//!   1. DNS SRV  — _nox._tcp.<address>
//!   2. DNS TXT  — _nox.<address>, record value: ng=<url>
//!   3. NodeInfo — /.well-known/nodeinfo, link rel="nox/1.0"
//!   4. Manual   — /.well-known/nox directly (https then http)
//!
//! # Examples
//! ```no_run
//! use noxapi::node_discovery::find_node_gateway;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let gateway = find_node_gateway("example.com").await?;
//! let gateway = find_node_gateway("example.com:3042").await?;
//! let gateway = find_node_gateway("192.168.1.100").await?;
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, warn};

const TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PORT: u16 = 3042;
const WELL_KNOWN_PATH: &str = "/.well-known/nox";
const NODEINFO_PATH: &str = "/.well-known/nodeinfo";
const NOX_NODEINFO_REL: &str = "nox/1.0";

// ─── NoxWellKnown document ────────────────────────────────────────────────────

/// Minimal subset of the `/.well-known/nox` JSON document we need.
#[derive(Debug, Deserialize)]
struct NoxWellKnownDoc {
    gateway: NoxWellKnownGateway,
}

#[derive(Debug, Deserialize)]
struct NoxWellKnownGateway {
    /// Full API base URL, e.g. "https://nox.example.com/api/"
    api: String,
}

// ─── DNS-over-HTTPS response structures ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DnsResponse {
    #[serde(rename = "Status")]
    status: i32,
    #[serde(rename = "Answer")]
    answer: Option<Vec<DnsRecord>>,
}

#[derive(Debug, Deserialize)]
struct DnsRecord {
    data: String,
}

// ─── NodeInfo structures ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NodeInfoLinks {
    links: Option<Vec<NodeInfoLink>>,
}

#[derive(Debug, Deserialize)]
struct NodeInfoLink {
    rel: String,
    href: String,
}

// ─── SRV record ───────────────────────────────────────────────────────────────

#[derive(Debug)]
struct SrvRecord {
    priority: u16,
    weight: u16,
    port: u16,
    target: String,
}

impl SrvRecord {
    /// Parse a DNS SRV data string: "<priority> <weight> <port> <target>"
    fn parse(data: &str) -> Option<Self> {
        let parts: Vec<&str> = data.split_whitespace().collect();
        if parts.len() < 4 {
            return None;
        }
        Some(SrvRecord {
            priority: parts[0].parse().ok()?,
            weight: parts[1].parse().ok()?,
            port: parts[2].parse().ok()?,
            target: parts[3].trim_end_matches('.').to_string(),
        })
    }
}

// ─── Main discovery function ──────────────────────────────────────────────────

/// Discover the API gateway URL for a given address.
///
/// Tries four strategies in order, returning the first successful result:
///   1. DNS SRV  — _nox._tcp.<address>
///   2. DNS TXT  — _nox.<address>, record value: ng=<url>
///   3. NodeInfo — /.well-known/nodeinfo, link rel="nox/1.0"
///   4. Manual   — /.well-known/nox directly (https then http)
pub async fn find_node_gateway(address: &str) -> Result<String> {
    debug!("Starting gateway discovery for: {}", address);

    let (host, explicit_port) = parse_address(address)?;

    // For IP addresses or localhost: skip DNS strategies, try direct connection
    if is_ip_or_localhost(&host) {
        debug!("Address is IP/localhost, trying direct connection");
        let port = explicit_port.unwrap_or(DEFAULT_PORT);
        for scheme in ["https", "http"] {
            let well_known_url = format!("{}://{}:{}{}", scheme, host, port, WELL_KNOWN_PATH);
            if let Some(base) = fetch_gateway_from_well_known(&well_known_url).await {
                debug!("Direct connection successful: {}", base);
                return Ok(base);
            }
        }
        let fallback = format!("http://{}:{}", host, port);
        warn!("Direct connection failed, using fallback: {}", fallback);
        return Ok(fallback);
    }

    // Strategy 1: DNS SRV
    debug!("Strategy 1 – DNS SRV for _nox._tcp.{}", host);
    if let Some(url) = discover_via_srv(&host).await {
        return Ok(url);
    }

    // Strategy 2: DNS TXT
    debug!("Strategy 2 – DNS TXT for _nox.{}", host);
    if let Some(url) = discover_via_txt(&host).await {
        return Ok(url);
    }

    // Strategy 3: NodeInfo
    debug!("Strategy 3 – NodeInfo for {}", host);
    if let Some(url) = discover_via_nodeinfo(&host, explicit_port).await {
        return Ok(url);
    }

    // Strategy 4: Manual well-known (standard ports; honour explicit port if given)
    debug!("Strategy 4 – Manual well-known for {}", host);
    let port_suffix = explicit_port.map(|p| format!(":{}", p)).unwrap_or_default();
    for scheme in ["https", "http"] {
        let well_known_url = format!("{}://{}{}{}", scheme, host, port_suffix, WELL_KNOWN_PATH);
        if let Some(base) = fetch_gateway_from_well_known(&well_known_url).await {
            debug!("Found gateway via manual well-known: {}", base);
            return Ok(base);
        }
    }

    // Fallback: original address with default port
    let port = explicit_port.unwrap_or(DEFAULT_PORT);
    let fallback = format!("http://{}:{}", host, port);
    warn!(
        "All discovery strategies failed, falling back to: {}",
        fallback
    );
    Ok(fallback)
}

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Strategy 1 — DNS SRV: _nox._tcp.<host>
async fn discover_via_srv(host: &str) -> Option<String> {
    let dns_url = format!(
        "https://dns.google/resolve?name=_nox._tcp.{}&type=SRV",
        host
    );

    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;

    let resp = client.get(&dns_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let dns: DnsResponse = resp.json().await.ok()?;
    if dns.status != 0 {
        return None;
    }

    let mut records: Vec<SrvRecord> = dns
        .answer?
        .iter()
        .filter_map(|r| SrvRecord::parse(&r.data))
        .collect();

    if records.is_empty() {
        return None;
    }

    // Lower priority first, then higher weight first
    records.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.weight.cmp(&a.weight)));

    for record in &records {
        for scheme in ["https", "http"] {
            let well_known_url = format!(
                "{}://{}:{}{}",
                scheme, record.target, record.port, WELL_KNOWN_PATH
            );
            if let Some(base) = fetch_gateway_from_well_known(&well_known_url).await {
                debug!("Found gateway via SRV: {}", base);
                return Some(base);
            }
        }
    }

    None
}

/// Strategy 2 — DNS TXT: _nox.<host>, look for ng=<url>
async fn discover_via_txt(host: &str) -> Option<String> {
    let dns_url = format!("https://dns.google/resolve?name=_nox.{}&type=TXT", host);

    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;

    let resp = client.get(&dns_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let dns: DnsResponse = resp.json().await.ok()?;
    if dns.status != 0 {
        return None;
    }

    for record in dns.answer.unwrap_or_default() {
        if let Some(well_known_url) = parse_ng_from_txt(&record.data) {
            // ng= value is the full /.well-known/nox URL; fetch it to get the API base
            if let Some(base) = fetch_gateway_from_well_known(&well_known_url).await {
                debug!("Found gateway via TXT ng= record: {}", base);
                return Some(base);
            }
        }
    }

    None
}

/// Strategy 3 — NodeInfo: /.well-known/nodeinfo, link rel="nox/1.0"
async fn discover_via_nodeinfo(host: &str, explicit_port: Option<u16>) -> Option<String> {
    let port_suffix = explicit_port.map(|p| format!(":{}", p)).unwrap_or_default();

    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;

    for scheme in ["https", "http"] {
        let url = format!("{}://{}{}{}", scheme, host, port_suffix, NODEINFO_PATH);

        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => continue,
        };

        let doc: NodeInfoLinks = match resp.json().await {
            Ok(d) => d,
            Err(_) => continue,
        };

        let links = match doc.links {
            Some(l) => l,
            None => continue,
        };

        let link = match links.into_iter().find(|l| l.rel == NOX_NODEINFO_REL) {
            Some(l) => l,
            None => continue,
        };

        if let Some(base) = fetch_gateway_from_well_known(&link.href).await {
            debug!("Found gateway via NodeInfo: {}", base);
            return Some(base);
        }
    }

    None
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Parse `ng=<url>` from a DNS TXT record value (may be quoted, semicolon-separated).
fn parse_ng_from_txt(data: &str) -> Option<String> {
    let data = data.trim_matches('"');
    for part in data.split([';', ' ']) {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("ng=") {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// Fetch the `/.well-known/nox` document at `url`, parse it, and return the
/// API base URL derived from `gateway.api` (without the trailing `/api/` path).
/// Returns `None` on any error (network, non-2xx, parse failure).
async fn fetch_gateway_from_well_known(url: &str) -> Option<String> {
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let doc: NoxWellKnownDoc = resp.json().await.ok()?;
    // Derive the server root by stripping the trailing /api path
    let api = doc.gateway.api.trim_end_matches('/');
    let base = api
        .strip_suffix("/api")
        .unwrap_or(api)
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        return None;
    }
    Some(base)
}

/// Check if address is an IP or localhost.
fn is_ip_or_localhost(address: &str) -> bool {
    address.parse::<std::net::IpAddr>().is_ok() || address == "localhost"
}

/// Parse address into `(host, explicit_port)`. Port is `None` if not specified.
fn parse_address(address: &str) -> Result<(String, Option<u16>)> {
    let cleaned = address
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    if let Some(colon_pos) = cleaned.rfind(':') {
        let host = &cleaned[..colon_pos];
        let port_str = cleaned[colon_pos + 1..].split('/').next().unwrap_or("");
        if let Ok(port) = port_str.parse::<u16>() {
            return Ok((host.to_string(), Some(port)));
        }
    }

    Ok((cleaned.to_string(), None))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ip_or_localhost() {
        assert!(is_ip_or_localhost("127.0.0.1"));
        assert!(is_ip_or_localhost("192.168.1.1"));
        assert!(is_ip_or_localhost("localhost"));
        assert!(!is_ip_or_localhost("example.com"));
        assert!(!is_ip_or_localhost("subdomain.example.com"));
    }

    #[test]
    fn test_parse_address() {
        assert_eq!(
            parse_address("example.com").unwrap(),
            ("example.com".to_string(), None)
        );
        assert_eq!(
            parse_address("example.com:8080").unwrap(),
            ("example.com".to_string(), Some(8080))
        );
        assert_eq!(
            parse_address("http://example.com:8080").unwrap(),
            ("example.com".to_string(), Some(8080))
        );
        assert_eq!(
            parse_address("https://example.com").unwrap(),
            ("example.com".to_string(), None)
        );
    }

    #[test]
    fn test_parse_ng_from_txt() {
        assert_eq!(
            parse_ng_from_txt("\"ng=https://gateway.example.com\""),
            Some("https://gateway.example.com".to_string())
        );
        assert_eq!(
            parse_ng_from_txt("ng=https://gateway.example.com"),
            Some("https://gateway.example.com".to_string())
        );
        assert_eq!(
            parse_ng_from_txt("v=nox;ng=https://gateway.example.com;other=value"),
            Some("https://gateway.example.com".to_string())
        );
        assert_eq!(parse_ng_from_txt("v=nox;other=value"), None);
    }

    #[test]
    fn test_parse_srv_record() {
        let record = SrvRecord::parse("10 20 443 gateway.example.com.").unwrap();
        assert_eq!(record.priority, 10);
        assert_eq!(record.weight, 20);
        assert_eq!(record.port, 443);
        assert_eq!(record.target, "gateway.example.com");
    }
}
