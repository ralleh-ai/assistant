//! Allowlisted HTTP fetch handler — the second real (non-filesystem) tool.
//!
//! Implements DEVELOPMENT.md §8.5 / §11.1 egress controls: the handler will
//! only contact hosts on an explicit allowlist, refuses non-http(s) schemes,
//! does not follow redirects, and blocks private / link-local / special
//! destinations after DNS resolution (SSRF / DNS-rebinding defense).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::redirect;
use url::Url;

use crate::handler::{ToolHandler, ToolInvocation, ToolResult};

const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 15;

/// GET (only) a URL whose host is on a configured egress allowlist.
pub struct HttpFetchHandler {
    client: Client,
    timeout: Duration,
    allowed_hosts: Vec<String>,
    max_response_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpFetchError {
    #[error("http_fetch requires a non-empty allowed_hosts egress allowlist")]
    EmptyAllowlist,
    #[error("failed to build HTTP client: {0}")]
    ClientBuild(String),
    #[error("arguments must be a JSON object with a string \"url\" field")]
    MissingUrlArgument,
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("only http and https URLs are allowed")]
    UnsupportedScheme,
    #[error("URLs with embedded credentials are not allowed")]
    UserinfoForbidden,
    #[error("host '{host}' is not on the egress allowlist")]
    HostNotAllowed { host: String },
    #[error("destination IP {ip} is blocked (private, loopback, link-local, or special-use)")]
    BlockedDestination { ip: IpAddr },
    #[error(
        "host '{host}' resolves to non-public IP {ip} (DNS rebinding / SSRF guard); \
         allowlist the IP literal only if intentional"
    )]
    NonPublicResolution { host: String, ip: IpAddr },
    #[error("DNS resolution failed for host '{host}': {reason}")]
    DnsFailed { host: String, reason: String },
    #[error("HTTP request failed: {0}")]
    Request(String),
    #[error("response body exceeds max_response_bytes ({0})")]
    ResponseTooLarge(usize),
    #[error("response body is not valid UTF-8")]
    NotUtf8,
}

/// Classification used for SSRF guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpClass {
    /// Globally routable unicast — OK for hostname allowlists.
    Public,
    /// 127.0.0.0/8 or ::1 — only via explicit IP allowlist entry.
    Loopback,
    /// RFC1918 / ULA / CGNAT — only via explicit IP allowlist entry.
    Private,
    /// Link-local / metadata-adjacent — never allowed, even if allowlisted.
    LinkLocal,
    /// Multicast, unspecified, documentation, etc. — never allowed.
    Special,
}

fn classify_ip(ip: IpAddr) -> IpClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(ip: Ipv4Addr) -> IpClass {
    let o = ip.octets();
    // Unspecified / "this" network
    if o[0] == 0 {
        return IpClass::Special;
    }
    // Loopback 127.0.0.0/8
    if o[0] == 127 {
        return IpClass::Loopback;
    }
    // Link-local 169.254.0.0/16 (includes cloud metadata 169.254.169.254)
    if o[0] == 169 && o[1] == 254 {
        return IpClass::LinkLocal;
    }
    // RFC1918
    if o[0] == 10 {
        return IpClass::Private;
    }
    if o[0] == 172 && (16..=31).contains(&o[1]) {
        return IpClass::Private;
    }
    if o[0] == 192 && o[1] == 168 {
        return IpClass::Private;
    }
    // CGNAT 100.64.0.0/10
    if o[0] == 100 && (64..=127).contains(&o[1]) {
        return IpClass::Private;
    }
    // Documentation / benchmark / benchmarking ranges commonly blocked for SSRF
    if o[0] == 192 && o[1] == 0 && o[2] == 2 {
        return IpClass::Special; // 192.0.2.0/24 TEST-NET-1
    }
    if o[0] == 198 && o[1] == 51 && o[2] == 100 {
        return IpClass::Special; // TEST-NET-2
    }
    if o[0] == 203 && o[1] == 0 && o[2] == 113 {
        return IpClass::Special; // TEST-NET-3
    }
    // Multicast / reserved
    if o[0] >= 224 {
        return IpClass::Special;
    }
    IpClass::Public
}

fn classify_v6(ip: Ipv6Addr) -> IpClass {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    if ip.is_loopback() {
        return IpClass::Loopback;
    }
    if ip.is_unspecified() {
        return IpClass::Special;
    }
    // fe80::/10 link-local
    let segments = ip.segments();
    if (segments[0] & 0xffc0) == 0xfe80 {
        return IpClass::LinkLocal;
    }
    // fc00::/7 unique local
    if (segments[0] & 0xfe00) == 0xfc00 {
        return IpClass::Private;
    }
    // Multicast ff00::/8
    if (segments[0] & 0xff00) == 0xff00 {
        return IpClass::Special;
    }
    IpClass::Public
}

fn is_never_allowable(class: IpClass) -> bool {
    matches!(class, IpClass::LinkLocal | IpClass::Special)
}

impl HttpFetchHandler {
    /// Construct a handler that will only contact the given hosts (exact,
    /// case-insensitive hostname match). An empty allowlist is rejected at
    /// construction time — fail closed rather than "allow nothing silently
    /// forever" which looks like a misconfiguration.
    pub fn new(allowed_hosts: Vec<String>) -> Result<Self, HttpFetchError> {
        Self::with_limits(
            allowed_hosts,
            DEFAULT_MAX_RESPONSE_BYTES,
            DEFAULT_TIMEOUT_SECS,
        )
    }

    pub fn with_limits(
        allowed_hosts: Vec<String>,
        max_response_bytes: usize,
        timeout_secs: u64,
    ) -> Result<Self, HttpFetchError> {
        if allowed_hosts.is_empty() {
            return Err(HttpFetchError::EmptyAllowlist);
        }
        let allowed_hosts: Vec<String> = allowed_hosts
            .into_iter()
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect();
        if allowed_hosts.is_empty() {
            return Err(HttpFetchError::EmptyAllowlist);
        }

        let timeout = Duration::from_secs(timeout_secs);
        let client = Client::builder()
            .timeout(timeout)
            // Never follow redirects: a 302 to an off-allowlist host would
            // otherwise bypass the egress check on the original URL.
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|e| HttpFetchError::ClientBuild(e.to_string()))?;

        Ok(Self {
            client,
            timeout,
            allowed_hosts,
            max_response_bytes,
        })
    }

    pub fn allowed_hosts(&self) -> &[String] {
        &self.allowed_hosts
    }

    fn host_allowed(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.allowed_hosts.iter().any(|allowed| allowed == &host)
    }

    /// After allowlist match: block unsafe literals and hostname→private DNS.
    ///
    /// Returns the set of validated, pinned socket addresses for a **hostname**
    /// destination (empty for an IP literal, where no name resolution — and so
    /// no rebinding window — is involved). The caller pins the actual
    /// connection to exactly these addresses via
    /// [`reqwest::blocking::ClientBuilder::resolve_to_addrs`], closing the
    /// TOCTOU gap where `reqwest` would otherwise re-resolve DNS independently
    /// of this validation (DNS-rebinding / SSRF).
    fn assert_safe_destination(
        &self,
        host: &str,
        port: u16,
    ) -> Result<Vec<SocketAddr>, HttpFetchError> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            let class = classify_ip(ip);
            if is_never_allowable(class) {
                return Err(HttpFetchError::BlockedDestination { ip });
            }
            // Loopback/private literals are only reachable because they were
            // explicitly allowlisted (caller already checked host_allowed).
            // The IP is fixed in the URL, so there is nothing to pin.
            return Ok(Vec::new());
        }

        // Hostname path: resolve exactly once, require every resolved address
        // to be public, and hand the validated set back so the connection is
        // pinned to it. This stops allowlisted names that rebind to
        // loopback/RFC1918/metadata between validation and connect.
        let resolved: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .map_err(|e| HttpFetchError::DnsFailed {
                host: host.to_string(),
                reason: e.to_string(),
            })?
            .collect();

        if resolved.is_empty() {
            return Err(HttpFetchError::DnsFailed {
                host: host.to_string(),
                reason: "no addresses returned".into(),
            });
        }
        for addr in &resolved {
            let ip = addr.ip();
            if classify_ip(ip) != IpClass::Public {
                return Err(HttpFetchError::NonPublicResolution {
                    host: host.to_string(),
                    ip,
                });
            }
        }
        Ok(resolved)
    }

    /// Validate the URL and return it alongside the pinned addresses (empty
    /// for an IP-literal host) the connection must be restricted to.
    fn validate_url(&self, raw: &str) -> Result<(Url, Vec<SocketAddr>), HttpFetchError> {
        let url = Url::parse(raw).map_err(|e| HttpFetchError::InvalidUrl(e.to_string()))?;
        match url.scheme() {
            "http" | "https" => {}
            _ => return Err(HttpFetchError::UnsupportedScheme),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(HttpFetchError::UserinfoForbidden);
        }
        let host = url
            .host_str()
            .ok_or_else(|| HttpFetchError::InvalidUrl("URL has no host".into()))?;
        if !self.host_allowed(host) {
            return Err(HttpFetchError::HostNotAllowed {
                host: host.to_string(),
            });
        }
        let port = url.port_or_known_default().unwrap_or(0);
        let pinned = self.assert_safe_destination(host, port)?;
        Ok((url, pinned))
    }
}

impl ToolHandler for HttpFetchHandler {
    fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, String> {
        let raw_url = invocation
            .arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or(HttpFetchError::MissingUrlArgument)
            .map_err(|e| e.to_string())?;

        let (url, pinned) = self.validate_url(raw_url).map_err(|e| e.to_string())?;

        // For a hostname destination, pin this request's DNS resolution to the
        // exact addresses we just validated so `reqwest` cannot re-resolve to a
        // rebound (private/metadata) IP. IP-literal destinations need no
        // pinning and reuse the shared pooled client.
        let pinned_client = if pinned.is_empty() {
            None
        } else {
            let host = url.host_str().unwrap_or_default().to_string();
            Some(
                Client::builder()
                    .timeout(self.timeout)
                    .redirect(redirect::Policy::none())
                    .resolve_to_addrs(&host, &pinned)
                    .build()
                    .map_err(|e| HttpFetchError::ClientBuild(e.to_string()).to_string())?,
            )
        };
        let client = pinned_client.as_ref().unwrap_or(&self.client);

        let response = client
            .get(url.clone())
            .send()
            .map_err(|e| HttpFetchError::Request(e.to_string()).to_string())?;

        let status = response.status().as_u16();
        let final_url = response.url().clone();
        // Defense in depth: even with redirects disabled, refuse if the
        // client somehow ended on a different / unsafe host. The connection
        // was already pinned to validated IPs, so a plain allowlist check
        // suffices here (no re-resolution needed).
        if let Some(host) = final_url.host_str() {
            if !self.host_allowed(host) {
                return Err(HttpFetchError::HostNotAllowed {
                    host: host.to_string(),
                }
                .to_string());
            }
        }

        let bytes = response
            .bytes()
            .map_err(|e| HttpFetchError::Request(e.to_string()).to_string())?;
        if bytes.len() > self.max_response_bytes {
            return Err(HttpFetchError::ResponseTooLarge(self.max_response_bytes).to_string());
        }
        let body =
            String::from_utf8(bytes.to_vec()).map_err(|_| HttpFetchError::NotUtf8.to_string())?;

        Ok(ToolResult {
            summary: format!(
                "fetched {} bytes from {} (HTTP {status})",
                body.len(),
                raw_url
            ),
            data: serde_json::json!({
                "status": status,
                "url": final_url.as_str(),
                "body": body,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    use crate::handler::ToolInvocation;

    fn invocation(url: &str) -> ToolInvocation {
        ToolInvocation {
            capability: "tool.http.fetch".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            arguments: serde_json::json!({ "url": url }),
        }
    }

    #[test]
    fn classifies_common_ssrf_targets() {
        assert_eq!(classify_ip("127.0.0.1".parse().unwrap()), IpClass::Loopback);
        assert_eq!(classify_ip("10.0.0.5".parse().unwrap()), IpClass::Private);
        assert_eq!(classify_ip("172.16.1.1".parse().unwrap()), IpClass::Private);
        assert_eq!(
            classify_ip("192.168.1.1".parse().unwrap()),
            IpClass::Private
        );
        assert_eq!(
            classify_ip("169.254.169.254".parse().unwrap()),
            IpClass::LinkLocal
        );
        assert_eq!(classify_ip("8.8.8.8".parse().unwrap()), IpClass::Public);
        assert_eq!(classify_ip("::1".parse().unwrap()), IpClass::Loopback);
        assert_eq!(
            classify_ip("::ffff:127.0.0.1".parse().unwrap()),
            IpClass::Loopback
        );
    }

    #[test]
    fn rejects_empty_allowlist_at_construction() {
        assert!(HttpFetchHandler::new(vec![]).is_err());
    }

    #[test]
    fn rejects_missing_url_argument() {
        let handler = HttpFetchHandler::new(vec!["example.com".into()]).unwrap();
        let inv = ToolInvocation {
            capability: "tool.http.fetch".into(),
            tenant_id: "t1".into(),
            device_id: "d1".into(),
            actor_id: "u1".into(),
            arguments: serde_json::json!({}),
        };
        let err = handler.invoke(&inv).unwrap_err();
        assert!(err.contains("url"));
    }

    #[test]
    fn rejects_host_not_on_allowlist() {
        let handler = HttpFetchHandler::new(vec!["example.com".into()]).unwrap();
        let err = handler
            .invoke(&invocation("https://evil.example.net/x"))
            .unwrap_err();
        assert!(err.contains("not on the egress allowlist"));
    }

    #[test]
    fn rejects_non_http_schemes() {
        let handler = HttpFetchHandler::new(vec!["example.com".into()]).unwrap();
        let err = handler
            .invoke(&invocation("file:///etc/passwd"))
            .unwrap_err();
        assert!(err.contains("http and https"));
    }

    #[test]
    fn rejects_urls_with_userinfo() {
        let handler = HttpFetchHandler::new(vec!["example.com".into()]).unwrap();
        let err = handler
            .invoke(&invocation("https://user:pass@example.com/"))
            .unwrap_err();
        assert!(err.contains("credentials"));
    }

    #[test]
    fn rejects_link_local_metadata_even_when_allowlisted() {
        let handler = HttpFetchHandler::new(vec!["169.254.169.254".into()]).unwrap();
        let err = handler
            .invoke(&invocation("http://169.254.169.254/latest/meta-data/"))
            .unwrap_err();
        assert!(
            err.contains("blocked") || err.contains("169.254"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn rejects_private_ip_literal_unless_allowlisted() {
        let handler = HttpFetchHandler::new(vec!["example.com".into()]).unwrap();
        let err = handler
            .invoke(&invocation("http://192.168.0.1/"))
            .unwrap_err();
        assert!(err.contains("not on the egress allowlist"));
    }

    #[test]
    fn rejects_localhost_name_even_if_allowlisted() {
        // Hostname path must not reach loopback via DNS (rebinding guard).
        let handler = HttpFetchHandler::new(vec!["localhost".into()]).unwrap();
        let err = handler
            .invoke(&invocation("http://localhost/"))
            .unwrap_err();
        assert!(
            err.contains("non-public") || err.contains("resolves"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn fetches_from_allowlisted_loopback_literal() {
        // Explicit IP allowlist is the supported way to hit local mocks.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let host = format!("127.0.0.1:{}", addr.port());

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            use std::io::{Read, Write};
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = b"hello from allowlisted fetch";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        });

        let handler = HttpFetchHandler::new(vec!["127.0.0.1".into()]).unwrap();
        let result = handler
            .invoke(&invocation(&format!("http://{host}/data")))
            .unwrap();
        assert!(result.summary.contains("200"));
        assert_eq!(result.data["body"], "hello from allowlisted fetch");
        assert_eq!(result.data["status"], 200);
    }

    #[test]
    fn allowlisted_host_still_rejects_sibling_host() {
        let handler =
            HttpFetchHandler::new(vec!["127.0.0.1".into(), "example.com".into()]).unwrap();
        assert!(handler.host_allowed("127.0.0.1"));
        assert!(handler.host_allowed("EXAMPLE.COM"));
        assert!(!handler.host_allowed("evil.com"));
    }
}
