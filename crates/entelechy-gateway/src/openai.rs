//! OpenAI-compatible model gateway for self-hosted open-source models
//! (PRD 8.3, 5.10, Q7).
//!
//! Self-hosted OSS runtimes — Ollama, vLLM, llama.cpp's server, LM Studio,
//! LocalAI, text-generation-webui — all expose the OpenAI `/v1/chat/completions`
//! API over plain HTTP on localhost. This gateway speaks that API with a
//! dependency-free `std::net` HTTP/1.1 client (no TLS), so a local model is the
//! default for tests and the 30-minute quickstart (Q7). Every call captures the
//! provider identity fields for drift detection (PV-1, 5.10).
//!
//! Enabled with the `openai` feature. Hosted `https://` providers need a
//! TLS-enabled transport (a follow-up feature); this module targets the
//! self-hosted `http://` case the PRD requires first.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::model::{ModelError, ModelGateway, ModelRequest, ModelResponse, ProviderIdentity};

/// Configuration for an OpenAI-compatible endpoint (PRD Q7).
#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    /// Base URL including the API version, e.g. `http://localhost:11434/v1`
    /// (Ollama), `http://localhost:8000/v1` (vLLM), `http://localhost:1234/v1`
    /// (LM Studio).
    pub base_url: String,
    /// Optional bearer token (vLLM `--api-key`, LocalAI, etc.); omit for Ollama.
    pub api_key: Option<String>,
    /// Request timeout.
    pub timeout: Duration,
}

/// An OpenAI-compatible model gateway (PRD 8.3).
#[derive(Clone, Debug)]
pub struct OpenAiGateway {
    config: OpenAiConfig,
    /// Short provider label recorded in the identity (e.g. `ollama`, `vllm`).
    provider_label: String,
}

impl OpenAiGateway {
    /// Create a gateway for any OpenAI-compatible endpoint.
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            config: OpenAiConfig {
                base_url: base_url.into(),
                api_key,
                timeout: Duration::from_secs(120),
            },
            provider_label: "openai-compatible".into(),
        }
    }

    /// A gateway pointed at a local Ollama server (default `http://localhost:11434/v1`).
    pub fn ollama() -> Self {
        let mut g = Self::new("http://localhost:11434/v1", None);
        g.provider_label = "ollama".into();
        g
    }

    /// A gateway pointed at a local vLLM server (`http://localhost:8000/v1`).
    pub fn vllm(api_key: Option<String>) -> Self {
        let mut g = Self::new("http://localhost:8000/v1", api_key);
        g.provider_label = "vllm".into();
        g
    }

    /// A gateway pointed at a local llama.cpp server (`http://localhost:8080/v1`).
    pub fn llama_cpp() -> Self {
        let mut g = Self::new("http://localhost:8080/v1", None);
        g.provider_label = "llama.cpp".into();
        g
    }

    /// Set a request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }
}

impl ModelGateway for OpenAiGateway {
    fn infer(&self, req: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let body = chat_request_body(&req.model, &req.prompt, req.temperature);
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let response = http_post_json(&url, self.config.api_key.as_deref(), &body, self.config.timeout)?;
        let parsed = parse_chat_response(&response)?;
        Ok(ModelResponse {
            text: parsed.content,
            identity: ProviderIdentity {
                provider: self.provider_label.clone(),
                advertised_model: parsed.model.unwrap_or_else(|| req.model.clone()),
                endpoint: self.config.base_url.clone(),
                request_schema_version: "openai.chat.v1".into(),
                sampling: serde_json::json!({ "temperature": req.temperature }),
                revision: parsed.system_fingerprint,
            },
        })
    }
}

/// Build the JSON body for a non-streaming chat completion.
pub fn chat_request_body(model: &str, prompt: &str, temperature: f64) -> String {
    let doc = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt }],
        "temperature": temperature,
        "stream": false,
    });
    doc.to_string()
}

/// The fields extracted from a chat-completions response.
#[derive(Debug, PartialEq)]
pub struct ChatParsed {
    /// The assistant message content.
    pub content: String,
    /// The model the server reports (advertised model — PV-1).
    pub model: Option<String>,
    /// The server's `system_fingerprint`, if any (provider revision — 5.10).
    pub system_fingerprint: Option<String>,
}

/// Parse an OpenAI-compatible chat-completions response body (PV-1).
pub fn parse_chat_response(body: &str) -> Result<ChatParsed, ModelError> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| ModelError::Provider(format!("invalid JSON: {e}")))?;
    // Surface an API error object if present.
    if let Some(err) = v.get("error") {
        return Err(ModelError::Provider(err.to_string()));
    }
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| ModelError::Provider("response missing choices[0].message.content".into()))?
        .to_string();
    Ok(ChatParsed {
        content,
        model: v.get("model").and_then(|m| m.as_str()).map(String::from),
        system_fingerprint: v
            .get("system_fingerprint")
            .and_then(|f| f.as_str())
            .map(String::from),
    })
}

/// A parsed endpoint URL.
#[derive(Debug, PartialEq, Eq)]
pub struct Url {
    /// `http` or `https`.
    pub scheme: String,
    /// Host.
    pub host: String,
    /// Port (defaults: 80 for http, 443 for https).
    pub port: u16,
    /// Request path.
    pub path: String,
}

/// Parse an `http(s)://host[:port]/path` URL.
pub fn parse_url(url: &str) -> Result<Url, ModelError> {
    let (scheme, rest, default_port) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r, 443u16)
    } else if let Some(r) = url.strip_prefix("http://") {
        ("http", r, 80u16)
    } else {
        return Err(ModelError::Provider(format!("URL must be http:// or https:// (got '{url}')")));
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>().map_err(|_| ModelError::Provider(format!("bad port in '{authority}'")))?,
        ),
        None => (authority.to_string(), default_port),
    };
    if host.is_empty() {
        return Err(ModelError::Provider("empty host in URL".into()));
    }
    Ok(Url { scheme: scheme.into(), host, port, path: path.to_string() })
}

/// Read+Write transport (a plain TCP stream, or a TLS stream over one).
trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// Open a transport for the URL. `https` requires the `openai-tls` feature.
fn connect(url: &Url, timeout: Duration) -> Result<Box<dyn ReadWrite>, ModelError> {
    let tcp = TcpStream::connect((url.host.as_str(), url.port))
        .map_err(|e| ModelError::Provider(format!("connect {}:{} failed: {e}", url.host, url.port)))?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();

    if url.scheme == "https" {
        #[cfg(feature = "openai-tls")]
        {
            let connector = native_tls::TlsConnector::new()
                .map_err(|e| ModelError::Provider(format!("TLS init failed: {e}")))?;
            let stream = connector
                .connect(&url.host, tcp)
                .map_err(|e| ModelError::Provider(format!("TLS handshake with {} failed: {e}", url.host)))?;
            return Ok(Box::new(stream));
        }
        #[cfg(not(feature = "openai-tls"))]
        {
            return Err(ModelError::Provider(
                "https endpoints require the `openai-tls` feature; self-hosted http:// works without it".into(),
            ));
        }
    }
    Ok(Box::new(tcp))
}

/// POST a JSON body to an `http(s)://` URL and return the response body (PRD Q7).
fn http_post_json(
    url: &str,
    api_key: Option<&str>,
    body: &str,
    timeout: Duration,
) -> Result<String, ModelError> {
    let parsed = parse_url(url)?;
    let mut stream = connect(&parsed, timeout)?;
    let host = &parsed.host;
    let path = &parsed.path;

    let mut auth = String::new();
    if let Some(key) = api_key {
        auth = format!("Authorization: Bearer {key}\r\n");
    }
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| ModelError::Provider(format!("write failed: {e}")))?;
    stream.flush().ok();

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| ModelError::Provider(format!("read failed: {e}")))?;

    let text = String::from_utf8_lossy(&raw);
    let (head, body_part) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| ModelError::Provider("malformed HTTP response".into()))?;

    // Check status line.
    let status_ok = head
        .lines()
        .next()
        .map(|l| l.contains(" 200"))
        .unwrap_or(false);
    let is_chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
    let decoded = if is_chunked {
        dechunk(body_part)
    } else {
        body_part.to_string()
    };
    if !status_ok {
        return Err(ModelError::Provider(format!(
            "HTTP status not 200: {}",
            head.lines().next().unwrap_or("")
        )));
    }
    Ok(decoded)
}

/// Decode an HTTP/1.1 chunked transfer body (minimal).
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let Some((size_line, after)) = rest.split_once("\r\n") else { break };
        let size = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        if after.len() < size {
            out.push_str(after);
            break;
        }
        out.push_str(&after[..size]);
        // Skip the chunk and its trailing CRLF.
        rest = after.get(size + 2..).unwrap_or("");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_chat_request() {
        let body = chat_request_body("llama3.1", "hello", 0.2);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "llama3.1");
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "hello");
        assert_eq!(v["stream"], false);
    }

    #[test]
    fn parses_a_chat_response() {
        let body = r#"{
            "model": "llama3.1:8b",
            "system_fingerprint": "fp_local_1",
            "choices": [{ "message": { "role": "assistant", "content": "hi there" } }]
        }"#;
        let parsed = parse_chat_response(body).unwrap();
        assert_eq!(parsed.content, "hi there");
        assert_eq!(parsed.model.as_deref(), Some("llama3.1:8b"));
        assert_eq!(parsed.system_fingerprint.as_deref(), Some("fp_local_1"));
    }

    #[test]
    fn surfaces_api_errors_and_missing_content() {
        assert!(parse_chat_response(r#"{"error":{"message":"model not found"}}"#).is_err());
        assert!(parse_chat_response(r#"{"choices":[]}"#).is_err());
        assert!(parse_chat_response("not json").is_err());
    }

    #[test]
    fn parses_endpoint_urls() {
        let u = parse_url("http://localhost:11434/v1/chat/completions").unwrap();
        assert_eq!((u.scheme.as_str(), u.host.as_str(), u.port, u.path.as_str()), ("http", "localhost", 11434, "/v1/chat/completions"));
        // Default ports by scheme.
        assert_eq!(parse_url("http://model.local/v1").unwrap().port, 80);
        let https = parse_url("https://api.openai.com/v1").unwrap();
        assert_eq!((https.scheme.as_str(), https.port), ("https", 443));
        // Unknown scheme is rejected.
        assert!(parse_url("ftp://x/y").is_err());
    }

    #[cfg(not(feature = "openai-tls"))]
    #[test]
    fn https_requires_the_tls_feature() {
        // Without openai-tls, an https call fails fast (before any TCP connect)
        // with a clear message; self-hosted http:// works without it.
        let gw = OpenAiGateway::new("https://api.openai.com/v1", Some("sk-x".into()));
        let err = gw
            .infer(&ModelRequest { model: "gpt".into(), prompt: "hi".into(), temperature: 0.0 })
            .unwrap_err();
        assert!(format!("{err}").contains("openai-tls"), "{err}");
    }

    #[test]
    fn dechunks_a_body() {
        // "Hello" (5) + " World" (6) chunks, terminated by a 0 chunk.
        let chunked = "5\r\nHello\r\n6\r\n World\r\n0\r\n\r\n";
        assert_eq!(dechunk(chunked), "Hello World");
    }

    #[test]
    fn preset_endpoints() {
        assert_eq!(OpenAiGateway::ollama().config.base_url, "http://localhost:11434/v1");
        assert_eq!(OpenAiGateway::vllm(None).config.base_url, "http://localhost:8000/v1");
        assert_eq!(OpenAiGateway::llama_cpp().config.base_url, "http://localhost:8080/v1");
    }
}
