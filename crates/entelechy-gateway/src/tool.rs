//! Tool gateway: capability invocation, effect journaling boundary (PRD 8.3).
//!
//! The ToolGateway performs capability checks, approval gates, egress policy and
//! effect journaling. This slice provides the trait plus a native in-process
//! registry sufficient for the Phase 0 simulated helpdesk (PRD 21).

use std::collections::HashMap;

/// Errors from a tool gateway.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// No such capability is registered.
    #[error("unknown capability: {0}")]
    UnknownCapability(String),
    /// The tool returned an application-level error.
    #[error("tool '{0}' failed: {1}")]
    Failed(String, String),
}

/// A tool invocation carrying its operation key for idempotency (PRD 7.6).
#[derive(Clone, Debug)]
pub struct ToolCall {
    /// Capability name.
    pub capability: String,
    /// Arguments.
    pub args: serde_json::Value,
    /// Operation key: derived from run, node path and logical attempt (PRD 7.6).
    pub operation_key: String,
}

/// Tool gateway abstraction (PRD 8.3).
pub trait ToolGateway {
    /// Invoke a capability. Effect journaling and reconciliation are layered by
    /// the runtime around this call (PRD 7.6, 8.5).
    fn call(&mut self, call: &ToolCall) -> Result<serde_json::Value, ToolError>;
}

/// A native tool: a boxed function over JSON args.
type NativeTool = Box<dyn FnMut(&serde_json::Value) -> Result<serde_json::Value, String> + Send>;

/// An in-process tool registry for native tools and the Phase 0 simulator.
#[derive(Default)]
pub struct NativeToolGateway {
    tools: HashMap<String, NativeTool>,
}

impl NativeToolGateway {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a native tool under a capability name.
    pub fn register(
        &mut self,
        capability: impl Into<String>,
        tool: impl FnMut(&serde_json::Value) -> Result<serde_json::Value, String> + Send + 'static,
    ) {
        self.tools.insert(capability.into(), Box::new(tool));
    }
}

impl ToolGateway for NativeToolGateway {
    fn call(&mut self, call: &ToolCall) -> Result<serde_json::Value, ToolError> {
        let tool = self
            .tools
            .get_mut(&call.capability)
            .ok_or_else(|| ToolError::UnknownCapability(call.capability.clone()))?;
        tool(&call.args).map_err(|e| ToolError::Failed(call.capability.clone(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_tool_roundtrip() {
        let mut gw = NativeToolGateway::new();
        gw.register("echo", |args| Ok(args.clone()));
        let out = gw
            .call(&ToolCall {
                capability: "echo".into(),
                args: json!({"x": 1}),
                operation_key: "run/echo/0".into(),
            })
            .unwrap();
        assert_eq!(out, json!({"x": 1}));
    }

    #[test]
    fn unknown_capability_errors() {
        let mut gw = NativeToolGateway::new();
        let err = gw.call(&ToolCall {
            capability: "nope".into(),
            args: json!(null),
            operation_key: "k".into(),
        });
        assert!(matches!(err, Err(ToolError::UnknownCapability(_))));
    }
}
