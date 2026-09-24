//! Server-mode API surface (transport-independent core).
//!
//! PRD v11 references: section 16.2 (API and console), 16.6 (protocol
//! compatibility), 17.2 (authentication & authorization), AU-1 (authenticated
//! identity + separate authorization; expiry fails closed), PC-1 (fail closed on
//! unsupported protocol).
//!
//! This crate provides the transport-independent request dispatcher: it
//! negotiates protocol compatibility, authenticates the caller (failing closed on
//! expiry or uncertain time), and enforces that **authentication never implies
//! authorization** (PRD 17.2) by requiring a per-operation authorizer. A future
//! HTTP/gRPC layer (deferred until the local core stabilizes, PRD 24) wraps this
//! dispatcher; "UI actions map to API operations" (PRD 16.2).
#![forbid(unsafe_code)]

pub mod http;

use std::collections::BTreeMap;

use entelechy_identity::{AuthContext, Principal, TimeSource};
use entelechy_protocol::{ProtocolVersion, VersionRange};

pub use http::{HttpServer, SystemClock};

/// An API request (PRD 16.2). Authentication context is explicit on every
/// request (PRD 17.2).
#[derive(Clone, Debug)]
pub struct ApiRequest {
    /// The operation name (maps 1:1 to a CLI/UI action — PRD 16.2).
    pub operation: String,
    /// The caller's authentication context.
    pub auth: AuthContext,
    /// The protocol version the client speaks.
    pub protocol: ProtocolVersion,
    /// The operation payload.
    pub payload: serde_json::Value,
}

/// Why an API request was rejected.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ApiError {
    /// The client protocol is unsupported (PC-1, fail closed).
    #[error("unsupported protocol {0}; upgrade required")]
    UnsupportedProtocol(ProtocolVersion),
    /// Authentication failed (expired / uncertain time — AU-1, fail closed).
    #[error("unauthenticated: {0}")]
    Unauthenticated(entelechy_identity::AuthError),
    /// The principal is authenticated but not authorized for this operation
    /// (PRD 17.2: authentication never implies authorization).
    #[error("unauthorized for operation '{0}'")]
    Unauthorized(String),
    /// No such operation is registered.
    #[error("unknown operation '{0}'")]
    UnknownOperation(String),
    /// The handler returned an application error.
    #[error("operation '{0}' failed: {1}")]
    Handler(String, String),
}

/// A handler for an operation: runs with the authenticated principal.
type Handler =
    Box<dyn Fn(&Principal, &serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;
/// An authorizer for an operation: decides whether a principal may invoke it.
type Authorizer = Box<dyn Fn(&Principal) -> bool + Send + Sync>;

struct Operation {
    authorizer: Authorizer,
    handler: Handler,
}

/// The transport-independent API dispatcher (PRD 16.2).
pub struct ApiServer {
    supported: VersionRange,
    operations: BTreeMap<String, Operation>,
}

impl ApiServer {
    /// Create a server advertising a supported protocol range (PRD 16.6).
    pub fn new(supported: VersionRange) -> Self {
        Self {
            supported,
            operations: BTreeMap::new(),
        }
    }

    /// Register an operation with its authorizer and handler. The authorizer
    /// enforces authorization separately from authentication (PRD 17.2).
    pub fn register(
        &mut self,
        operation: impl Into<String>,
        authorizer: impl Fn(&Principal) -> bool + Send + Sync + 'static,
        handler: impl Fn(&Principal, &serde_json::Value) -> Result<serde_json::Value, String>
            + Send
            + Sync
            + 'static,
    ) {
        self.operations.insert(
            operation.into(),
            Operation {
                authorizer: Box::new(authorizer),
                handler: Box::new(handler),
            },
        );
    }

    /// Handle a request through the full gate sequence: protocol → authentication
    /// → authorization → dispatch (PRD 16.2, 16.6, 17.2). Every stage fails closed.
    pub fn handle(
        &self,
        req: &ApiRequest,
        clock: &dyn TimeSource,
    ) -> Result<serde_json::Value, ApiError> {
        // 1. Protocol compatibility (PC-1, fail closed).
        if !self.supported.contains(req.protocol) {
            return Err(ApiError::UnsupportedProtocol(req.protocol));
        }
        // 2. Authentication (AU-1, fail closed on expiry / uncertain time).
        let principal = req
            .auth
            .authenticate(clock)
            .map_err(ApiError::Unauthenticated)?;
        // 3. Operation lookup.
        let op = self
            .operations
            .get(&req.operation)
            .ok_or_else(|| ApiError::UnknownOperation(req.operation.clone()))?;
        // 4. Authorization — never implied by authentication (PRD 17.2).
        if !(op.authorizer)(principal) {
            return Err(ApiError::Unauthorized(req.operation.clone()));
        }
        // 5. Dispatch.
        (op.handler)(principal, &req.payload)
            .map_err(|e| ApiError::Handler(req.operation.clone(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_identity::{Credential, FixedClock, PrincipalKind};

    fn ctx(kind: PrincipalKind, expires_at: u64) -> AuthContext {
        AuthContext {
            principal: Principal::new("p", kind),
            tenant: "local".into(),
            credential: Credential {
                issued_at: 0,
                expires_at,
            },
        }
    }

    fn server() -> ApiServer {
        let mut s = ApiServer::new(VersionRange::new(
            ProtocolVersion::new(1, 0),
            ProtocolVersion::new(1, 3),
        ));
        s.register(
            "approve_release",
            |p| p.kind == PrincipalKind::Approver,
            |_p, _payload| Ok(serde_json::json!({ "ok": true })),
        );
        s
    }

    fn req(op: &str, auth: AuthContext, protocol: ProtocolVersion) -> ApiRequest {
        ApiRequest {
            operation: op.into(),
            auth,
            protocol,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn happy_path() {
        let s = server();
        let r = req(
            "approve_release",
            ctx(PrincipalKind::Approver, 1000),
            ProtocolVersion::new(1, 2),
        );
        assert_eq!(
            s.handle(&r, &FixedClock(Some(10))).unwrap(),
            serde_json::json!({"ok": true})
        );
    }

    #[test]
    fn unsupported_protocol_fails_closed() {
        let s = server();
        let r = req(
            "approve_release",
            ctx(PrincipalKind::Approver, 1000),
            ProtocolVersion::new(2, 0),
        );
        assert!(matches!(
            s.handle(&r, &FixedClock(Some(10))),
            Err(ApiError::UnsupportedProtocol(_))
        ));
    }

    #[test]
    fn expired_credential_fails_closed() {
        let s = server();
        let r = req(
            "approve_release",
            ctx(PrincipalKind::Approver, 100),
            ProtocolVersion::new(1, 2),
        );
        assert!(matches!(
            s.handle(&r, &FixedClock(Some(200))),
            Err(ApiError::Unauthenticated(_))
        ));
        // Uncertain time also fails closed.
        assert!(matches!(
            s.handle(&r, &FixedClock(None)),
            Err(ApiError::Unauthenticated(_))
        ));
    }

    #[test]
    fn authenticated_but_unauthorized_is_rejected() {
        let s = server();
        // An operator is authenticated but not authorized to approve releases.
        let r = req(
            "approve_release",
            ctx(PrincipalKind::Operator, 1000),
            ProtocolVersion::new(1, 2),
        );
        assert_eq!(
            s.handle(&r, &FixedClock(Some(10))),
            Err(ApiError::Unauthorized("approve_release".into()))
        );
    }

    #[test]
    fn unknown_operation() {
        let s = server();
        let r = req(
            "nope",
            ctx(PrincipalKind::Approver, 1000),
            ProtocolVersion::new(1, 2),
        );
        assert!(matches!(
            s.handle(&r, &FixedClock(Some(10))),
            Err(ApiError::UnknownOperation(_))
        ));
    }
}
