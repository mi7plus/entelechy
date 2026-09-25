//! Minimal blocking HTTP/1.1 transport over the [`crate::ApiServer`] dispatcher.
//!
//! PRD v11 references: section 16.2 (gRPC/REST API; UI actions map to API
//! operations), 16.6 (protocol version negotiation), 17.2 (authentication),
//! Q30 (loopback-only development tokens in local mode).
//!
//! This is a dependency-free HTTP/1.1 server (std::net only) suitable for local
//! mode and tests. It maps `POST /v1/<operation>` to an [`crate::ApiRequest`],
//! carrying the caller in `Authorization: Bearer <dev-token>`, the client
//! protocol in `X-Protocol-Version: <major>.<minor>`, and the tenant in
//! `X-Tenant`. A production deployment would replace dev tokens with OIDC/mTLS
//! (Q30), but the dispatcher, error mapping and version negotiation are identical.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use entelechy_identity::{AuthContext, Credential, Principal, TimeSource};
use entelechy_protocol::ProtocolVersion;

use crate::{ApiError, ApiRequest, ApiServer};

/// Maximum accepted request-body size. A `Content-Length` above this is rejected
/// with `413` rather than triggering an unbounded allocation — a loopback client
/// must not be able to exhaust server memory (PRD 17 robustness).
const MAX_BODY_BYTES: usize = 1024 * 1024; // 1 MiB

/// Maximum number of header lines accepted, bounding the header-read loop so a
/// client cannot stream headers indefinitely.
const MAX_HEADER_LINES: usize = 100;

/// Per-connection read/write timeout. `serve` handles connections serially, so a
/// single stalled client (partial request, or `Content-Length` with no body) must
/// not wedge the whole server — the connection is dropped once it goes idle.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// A wall-clock time source backed by the system clock (PRD 17.2). Returns
/// `None` only if the clock is before the Unix epoch, which fails closed.
pub struct SystemClock;

impl TimeSource for SystemClock {
    fn now(&self) -> Option<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }
}

/// An HTTP server wrapping the API dispatcher (PRD 16.2).
pub struct HttpServer {
    api: ApiServer,
    tokens: BTreeMap<String, Principal>,
}

impl HttpServer {
    /// Wrap an [`ApiServer`].
    pub fn new(api: ApiServer) -> Self {
        Self {
            api,
            tokens: BTreeMap::new(),
        }
    }

    /// Register a loopback-only development token mapped to a principal (Q30).
    pub fn add_dev_token(&mut self, token: impl Into<String>, principal: Principal) {
        self.tokens.insert(token.into(), principal);
    }

    /// Bind a loopback listener on the given port (Q30: local dev is loopback
    /// only). Port 0 picks an ephemeral port.
    pub fn bind_local(port: u16) -> std::io::Result<TcpListener> {
        TcpListener::bind(("127.0.0.1", port))
    }

    /// Serve connections until the listener is closed (blocking). Each connection
    /// handles one request then closes (`Connection: close`).
    pub fn serve(&self, listener: &TcpListener) -> std::io::Result<()> {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    // Best-effort per connection; a bad request must not kill the
                    // server.
                    let _ = self.handle_stream(s);
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }

    /// Handle exactly one connection (used by `serve` and by tests).
    pub fn handle_stream(&self, mut stream: TcpStream) -> std::io::Result<()> {
        // Bound how long a single connection can hold the (serial) server.
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let response = match self.read_request(&mut stream) {
            Ok(req) => self.dispatch(req),
            Err(bad) => bad,
        };
        write_response(&mut stream, &response)
    }

    /// Parse an HTTP request into an [`ApiRequest`], or produce an error response.
    fn read_request(&self, stream: &mut TcpStream) -> Result<ApiRequest, HttpResponse> {
        let mut reader = BufReader::new(stream.try_clone().map_err(|_| bad_request("io error"))?);

        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .map_err(|_| bad_request("io error"))?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        if method != "POST" {
            return Err(HttpResponse::json(
                405,
                &serde_json::json!({ "error": "method not allowed; use POST" }),
            ));
        }
        let operation = target.strip_prefix("/v1/").unwrap_or("").to_string();
        if operation.is_empty() {
            return Err(bad_request("path must be /v1/<operation>"));
        }

        // Headers (count-bounded so a client cannot stream them indefinitely).
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        let mut header_lines = 0usize;
        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|_| bad_request("io error"))?;
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            header_lines += 1;
            if header_lines > MAX_HEADER_LINES {
                return Err(HttpResponse::json(
                    431,
                    &serde_json::json!({ "error": "too many header fields" }),
                ));
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }

        // Body per Content-Length, capped to bound allocation (413 above the cap).
        let len: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if len > MAX_BODY_BYTES {
            return Err(HttpResponse::json(
                413,
                &serde_json::json!({
                    "error": format!(
                        "payload too large: {len} bytes exceeds {MAX_BODY_BYTES}-byte limit"
                    )
                }),
            ));
        }
        let mut body = vec![0u8; len];
        if len > 0 {
            reader
                .read_exact(&mut body)
                .map_err(|_| bad_request("truncated body"))?;
        }
        let payload: serde_json::Value = if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&body).map_err(|_| bad_request("invalid JSON body"))?
        };

        // Auth: loopback dev bearer token → principal (Q30).
        let principal = headers
            .get("authorization")
            .and_then(|h| {
                h.strip_prefix("Bearer ")
                    .or_else(|| h.strip_prefix("bearer "))
            })
            .and_then(|tok| self.tokens.get(tok.trim()).cloned())
            .ok_or_else(|| {
                HttpResponse::json(
                    401,
                    &serde_json::json!({ "error": "missing or unknown bearer token" }),
                )
            })?;

        // Protocol version header (default 1.0).
        let protocol = headers
            .get("x-protocol-version")
            .and_then(|v| parse_version(v))
            .unwrap_or(ProtocolVersion::new(1, 0));

        let tenant = headers
            .get("x-tenant")
            .cloned()
            .unwrap_or_else(|| "local".to_string());

        Ok(ApiRequest {
            operation,
            auth: AuthContext {
                principal,
                tenant,
                credential: dev_credential(),
            },
            protocol,
            payload,
        })
    }

    fn dispatch(&self, req: ApiRequest) -> HttpResponse {
        match self.api.handle(&req, &SystemClock) {
            Ok(value) => HttpResponse::json(200, &value),
            Err(e) => {
                let (code, msg) = match &e {
                    ApiError::UnsupportedProtocol(_) => (400, e.to_string()),
                    ApiError::Unauthenticated(_) => (401, e.to_string()),
                    ApiError::Unauthorized(_) => (403, e.to_string()),
                    ApiError::UnknownOperation(_) => (404, e.to_string()),
                    ApiError::Handler(_, _) => (500, e.to_string()),
                };
                HttpResponse::json(code, &serde_json::json!({ "error": msg }))
            }
        }
    }
}

/// A rendered HTTP response.
pub struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }
    /// The status code (for tests).
    pub fn status(&self) -> u16 {
        self.status
    }
}

fn bad_request(msg: &str) -> HttpResponse {
    HttpResponse::json(400, &serde_json::json!({ "error": msg }))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn write_response(stream: &mut TcpStream, resp: &HttpResponse) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        reason(resp.status),
        resp.body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&resp.body)?;
    stream.flush()
}

fn parse_version(s: &str) -> Option<ProtocolVersion> {
    let (maj, min) = s.trim().split_once('.')?;
    Some(ProtocolVersion::new(maj.parse().ok()?, min.parse().ok()?))
}

/// Build a credential for a loopback dev token: valid until far future, since
/// dev tokens are loopback-only (Q30). Server modes use OIDC/mTLS instead.
pub fn dev_credential() -> Credential {
    Credential {
        issued_at: 0,
        expires_at: u64::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_identity::PrincipalKind;
    use entelechy_protocol::VersionRange;
    use std::thread;

    fn test_server() -> HttpServer {
        let mut api = ApiServer::new(VersionRange::new(
            ProtocolVersion::new(1, 0),
            ProtocolVersion::new(1, 3),
        ));
        api.register(
            "status",
            |_p| true,
            |p, _payload| Ok(serde_json::json!({ "ok": true, "principal": p.id })),
        );
        let mut server = HttpServer::new(api);
        server.add_dev_token(
            "dev-token-1",
            Principal::new("dev", PrincipalKind::Operator),
        );
        server
    }

    /// Send a raw request to a one-shot server and return (status_line, body).
    fn roundtrip(request: &str) -> (String, String) {
        let listener = HttpServer::bind_local(0).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let server = test_server();
            let (stream, _) = listener.accept().unwrap();
            server.handle_stream(stream).unwrap();
        });

        let mut client = TcpStream::connect(addr).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).unwrap();
        handle.join().unwrap();

        let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
        let status_line = head.lines().next().unwrap_or("").to_string();
        (status_line, body.to_string())
    }

    fn post(op: &str, headers: &str, body: &str) -> String {
        format!(
            "POST /v1/{op} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn authorized_request_succeeds() {
        let (status, body) = roundtrip(&post(
            "status",
            "Authorization: Bearer dev-token-1\r\nX-Protocol-Version: 1.2\r\n",
            "{}",
        ));
        assert!(status.contains("200"), "status={status}");
        assert!(body.contains("\"ok\":true"), "body={body}");
        assert!(body.contains("\"principal\":\"dev\""));
    }

    #[test]
    fn missing_token_is_401() {
        let (status, _) = roundtrip(&post("status", "X-Protocol-Version: 1.2\r\n", "{}"));
        assert!(status.contains("401"), "status={status}");
    }

    #[test]
    fn unsupported_protocol_is_400() {
        let (status, _) = roundtrip(&post(
            "status",
            "Authorization: Bearer dev-token-1\r\nX-Protocol-Version: 2.0\r\n",
            "{}",
        ));
        assert!(status.contains("400"), "status={status}");
    }

    #[test]
    fn unknown_operation_is_404() {
        let (status, _) = roundtrip(&post(
            "nope",
            "Authorization: Bearer dev-token-1\r\nX-Protocol-Version: 1.0\r\n",
            "{}",
        ));
        assert!(status.contains("404"), "status={status}");
    }

    #[test]
    fn oversized_content_length_is_413_without_allocating() {
        // Declares a body far larger than the cap. The server must reject on the
        // header (before allocating or reading the body), so this returns fast.
        let huge = MAX_BODY_BYTES + 1;
        let req = format!(
            "POST /v1/status HTTP/1.1\r\nHost: localhost\r\n\
             Authorization: Bearer dev-token-1\r\nContent-Length: {huge}\r\n\r\n{{}}"
        );
        let (status, body) = roundtrip(&req);
        assert!(status.contains("413"), "status={status}");
        assert!(body.contains("payload too large"), "body={body}");
    }

    #[test]
    fn too_many_header_fields_is_431() {
        let mut filler = String::new();
        for i in 0..(MAX_HEADER_LINES + 5) {
            filler.push_str(&format!("X-Filler-{i}: v\r\n"));
        }
        let req = format!(
            "POST /v1/status HTTP/1.1\r\nHost: localhost\r\n\
             Authorization: Bearer dev-token-1\r\n{filler}Content-Length: 0\r\n\r\n"
        );
        let (status, _) = roundtrip(&req);
        assert!(status.contains("431"), "status={status}");
    }

    #[test]
    fn truncated_body_is_400() {
        use std::net::Shutdown;
        let listener = HttpServer::bind_local(0).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let server = test_server();
            let (stream, _) = listener.accept().unwrap();
            let _ = server.handle_stream(stream);
        });

        let mut client = TcpStream::connect(addr).unwrap();
        // Declares 100 bytes but sends 2, then closes the write half so the
        // server's read_exact hits EOF immediately (no timeout wait).
        let req = "POST /v1/status HTTP/1.1\r\nHost: localhost\r\n\
                   Authorization: Bearer dev-token-1\r\nContent-Length: 100\r\n\r\n{}";
        client.write_all(req.as_bytes()).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).unwrap();
        handle.join().unwrap();

        assert!(
            resp.lines().next().unwrap_or("").contains("400"),
            "resp={resp}"
        );
    }
}
