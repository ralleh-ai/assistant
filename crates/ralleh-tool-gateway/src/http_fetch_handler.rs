//! Allowlisted HTTP fetch handler — the second real (non-filesystem) tool.
//!
//! Implements DEVELOPMENT.md §8.5 / §11.1 egress controls: the handler will
//! only contact hosts on an explicit allowlist, refuses non-http(s) schemes,
//! and does not follow redirects (redirect-based SSRF). Policy still decides
//! *whether* a tenant may call `tool.http.fetch`; this handler enforces its
//! own egress sandbox regardless.

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
    #[error("HTTP request failed: {0}")]
    Request(String),
    #[error("response body exceeds max_response_bytes ({0})")]
    ResponseTooLarge(usize),
    #[error("response body is not valid UTF-8")]
    NotUtf8,
}

impl HttpFetchHandler {
    /// Construct a handler that will only contact the given hosts (exact,
    /// case-insensitive hostname match). An empty allowlist is rejected at
    /// construction time — fail closed rather than "allow nothing silently
    /// forever" which looks like a misconfiguration.
    pub fn new(allowed_hosts: Vec<String>) -> Result<Self, HttpFetchError> {
        Self::with_limits(allowed_hosts, DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECS)
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

        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            // Never follow redirects: a 302 to an off-allowlist host would
            // otherwise bypass the egress check on the original URL.
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|e| HttpFetchError::ClientBuild(e.to_string()))?;

        Ok(Self {
            client,
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

    fn validate_url(&self, raw: &str) -> Result<Url, HttpFetchError> {
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
        Ok(url)
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

        let url = self.validate_url(raw_url).map_err(|e| e.to_string())?;

        let response = self
            .client
            .get(url.clone())
            .send()
            .map_err(|e| HttpFetchError::Request(e.to_string()).to_string())?;

        let status = response.status().as_u16();
        let final_url = response.url().clone();
        // Defense in depth: even with redirects disabled, refuse if the
        // client somehow ended on a different host.
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
        let body = String::from_utf8(bytes.to_vec())
            .map_err(|_| HttpFetchError::NotUtf8.to_string())?;

        Ok(ToolResult {
            summary: format!("fetched {} bytes from {} (HTTP {status})", body.len(), raw_url),
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
    fn fetches_from_allowlisted_mock_server() {
        // Tiny blocking HTTP server so we don't need the async wiremock
        // runtime inside this sync ToolHandler test.
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
        // Port is part of host_str for URLs with an explicit port — ensure
        // a bare IP allowlist entry does not accidentally permit :9999
        // unless that exact host:port was listed. Here we listed "127.0.0.1"
        // without a port, so http://127.0.0.1:1/ (port 1) should fail the
        // host match (host_str is "127.0.0.1" actually for Url - wait,
        // Url::host_str() returns hostname WITHOUT port. So 127.0.0.1
        // allowlist WOULD match 127.0.0.1:anyport.
        //
        // That's correct hostname-based allowlisting: ports aren't part of
        // the host identity for our matcher. Assert the happy path of
        // hostname matching instead.
        assert!(handler.host_allowed("127.0.0.1"));
        assert!(handler.host_allowed("EXAMPLE.COM"));
        assert!(!handler.host_allowed("evil.com"));
    }
}
