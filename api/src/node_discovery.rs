//! Automatic node gateway discovery
//!
//! This module implements the Nox node discovery protocol:
//! 1. If the address is an IP or localhost, tries HTTP directly
//! 2. For domain names, tries /.well-known/nox endpoint (HTTPS then HTTP)
//! 3. Falls back to DNS TXT record lookup (_nox.{domain})
//!
//! # Examples
//! ```no_run
//! use noxapi::node_discovery::find_node_gateway;
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Discover from domain
//! let gateway = find_node_gateway("example.com").await?;
//!
//! // With explicit port
//! let gateway = find_node_gateway("example.com:3042").await?;
//!
//! // IP address
//! let gateway = find_node_gateway("192.168.1.100").await?;
//! # Ok(())
//! # }
//! ```

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, warn};

const TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_PORT: u16 = 3042;

/// DNS TXT record response from Google DNS API
///
/// ```json
/// {
///   "Status": 0,
///   "Answer": [
///     {"name": "_nox.example.com", "type": 16, "data": "\"mg=https://gateway.example.com\""}
///   ]
/// }
/// ```
///
/// DNS TXT records should be formatted as: `mg=gateway_url`
/// Example TXT record: `_nox.example.com TXT "mg=https://gateway.example.com"`
#[derive(Debug, Deserialize)]
struct DnsResponse {
    #[serde(rename = "Status")]
    status: i32,
    #[serde(rename = "Answer")]
    answer: Option<Vec<DnsTxtRecord>>,
}

#[derive(Debug, Deserialize)]
struct DnsTxtRecord {
    data: String,
}

impl DnsTxtRecord {
    fn parse_gateway(&self) -> Option<String> {
        // Remove quotes from TXT record
        let data = self.data.trim_matches('"');

        // Split by semicolon and find mg= entry
        for part in data.split(';') {
            let kv: Vec<&str> = part.split('=').collect();
            if kv.len() == 2 && kv[0].trim() == "mg" {
                return Some(kv[1].trim().to_string());
            }
        }
        None
    }
}

/// Discover node gateway from address
///
/// Implements automatic gateway discovery with multiple fallback strategies:
/// - Direct connection for IP addresses
/// - .well-known/nox endpoint checking (HTTPS → HTTP)
/// - DNS TXT record resolution via Google DNS API
///
/// # Arguments
/// * `address` - Domain name, IP address, or URL with optional port
///
/// # Returns
/// Discovered gateway URL or the original address if discovery fails
pub async fn find_node_gateway(address: &str) -> Result<String> {
    debug!("Starting gateway discovery for: {}", address);

    // Parse address to extract host and port
    let (host, port) = parse_address(address)?;

    // Strategy 1: If it's an IP address or localhost, try direct connection
    if is_ip_or_localhost(&host) {
        debug!("Address is IP/localhost, attempting direct connection");
        if try_well_known(&format!("http://{}:{}", host, port)).await {
            let url = format!("http://{}:{}", host, port);
            debug!("Direct connection successful: {}", url);
            return Ok(url);
        }
    }

    // Strategy 2: For domain names, try .well-known endpoint
    if !is_ip_or_localhost(&host) {
        debug!("Trying .well-known/nox endpoint for domain: {}", host);

        // Try HTTPS first
        if try_well_known(&format!("https://{}:{}", host, port)).await {
            let url = format!("https://{}:{}", host, port);
            debug!("Found gateway via HTTPS .well-known: {}", url);
            return Ok(url);
        }

        // Fallback to HTTP
        if try_well_known(&format!("http://{}:{}", host, port)).await {
            let url = format!("http://{}:{}", host, port);
            debug!("Found gateway via HTTP .well-known: {}", url);
            return Ok(url);
        }
    }

    // Strategy 3: DNS TXT record lookup
    if !is_ip_or_localhost(&host) {
        debug!("Attempting DNS TXT lookup for: {}", host);
        match resolve_dns_txt(&host).await {
            Ok(gateways) if !gateways.is_empty() => {
                debug!("Found {} gateways via DNS TXT", gateways.len());
                return Ok(gateways[0].clone());
            }
            Ok(_) => debug!("No gateways found in DNS TXT"),
            Err(e) => warn!("DNS TXT lookup failed: {}", e),
        }
    }

    // Strategy 4: Return the original address
    debug!("All discovery strategies failed, using original address");
    Ok(format!("http://{}:{}", host, port))
}

/// Check if address is an IP or localhost
fn is_ip_or_localhost(address: &str) -> bool {
    address.parse::<std::net::IpAddr>().is_ok() || address == "localhost"
}

/// Parse address into host and port components
fn parse_address(address: &str) -> Result<(String, u16)> {
    // Remove protocol if present
    let cleaned = address
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    // Try to parse as URL with port
    if let Some(colon_pos) = cleaned.rfind(':') {
        let host = &cleaned[..colon_pos];
        let port_str = &cleaned[colon_pos + 1..];

        // Remove trailing path if present
        let port_str = port_str.split('/').next().unwrap_or(port_str);

        if let Ok(port) = port_str.parse::<u16>() {
            return Ok((host.to_string(), port));
        }
    }

    // No port specified, use default
    Ok((cleaned.to_string(), DEFAULT_PORT))
}

/// Test if /.well-known/nox endpoint responds
async fn try_well_known(base_url: &str) -> bool {
    let url = format!("{}/.well-known/nox", base_url);

    match reqwest::Client::builder().timeout(TIMEOUT).build() {
        Ok(client) => match client.get(&url).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// Resolve node gateway from DNS TXT record (_nox.{domain})
async fn resolve_dns_txt(domain: &str) -> Result<Vec<String>> {
    let url = format!("https://dns.google/resolve?name=_nox.{}&type=TXT", domain);

    let client = reqwest::Client::builder().timeout(TIMEOUT).build()?;

    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!("DNS query failed"));
    }

    let dns_response: DnsResponse = response.json().await?;

    if dns_response.status != 0 {
        return Err(anyhow!("DNS query returned error status"));
    }

    let mut gateways = Vec::new();

    if let Some(answers) = dns_response.answer {
        for record in answers {
            if let Some(gateway) = record.parse_gateway() {
                gateways.push(gateway);
            }
        }
    }

    if gateways.is_empty() {
        return Err(anyhow!("No gateway found in DNS TXT records"));
    }

    Ok(gateways)
}

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
            ("example.com".to_string(), DEFAULT_PORT)
        );
        assert_eq!(
            parse_address("example.com:8080").unwrap(),
            ("example.com".to_string(), 8080)
        );
        assert_eq!(
            parse_address("http://example.com:8080").unwrap(),
            ("example.com".to_string(), 8080)
        );
        assert_eq!(
            parse_address("https://example.com").unwrap(),
            ("example.com".to_string(), DEFAULT_PORT)
        );
    }

    #[test]
    fn test_parse_gateway_from_txt() {
        let record = DnsTxtRecord {
            data: "\"mg=https://gateway.example.com\"".to_string(),
        };
        assert_eq!(
            record.parse_gateway(),
            Some("https://gateway.example.com".to_string())
        );

        let record = DnsTxtRecord {
            data: "mg=https://gateway.example.com".to_string(),
        };
        assert_eq!(
            record.parse_gateway(),
            Some("https://gateway.example.com".to_string())
        );

        let record = DnsTxtRecord {
            data: "v=nox;mg=https://gateway.example.com;other=value".to_string(),
        };
        assert_eq!(
            record.parse_gateway(),
            Some("https://gateway.example.com".to_string())
        );
    }
}
