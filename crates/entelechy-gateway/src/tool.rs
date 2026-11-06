//! Tool gateway: capability invocation, effect journaling boundary (PRD 8.3).
//!
//! The ToolGateway performs capability checks, approval gates, egress policy and
//! effect journaling. This slice provides the trait plus a native in-process
//! registry sufficient for the Phase 0 simulated helpdesk (PRD 21).

use std::collections::HashMap;

/// Whether a *failed* tool call could have left a side effect behind (PRD 8.5, 8.6).
///
/// The runtime maps this onto a terminal state: a definitive rejection that
/// committed nothing yields a clean `Failed`, while a lost or indeterminate
/// acknowledgement — the effect may have taken hold — forces
/// `ReconciliationRequired`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectOutcome {
    /// Nothing was committed: the call was rejected or never dispatched, so there
    /// is no dangling effect to reconcile.
    NoEffect,
    /// The outcome is unknown: an acknowledgement was lost, or an app-level error
    /// may follow a partial effect, so the effect must be reconciled.
    Indeterminate,
}

/// Errors from a tool gateway.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// No such capability is registered.
    #[error("unknown capability: {0}")]
    UnknownCapability(String),
    /// A guarded tool validated the call and refused it *before* any side effect,
    /// so nothing was committed (PRD 8.5).
    #[error("tool '{0}' rejected the call: {1}")]
    Rejected(String, String),
    /// The tool returned an application-level error.
    #[error("tool '{0}' failed: {1}")]
    Failed(String, String),
}

impl ToolError {
    /// Classify a failure by whether a side effect could remain (PRD 8.5, 8.6).
    ///
    /// `UnknownCapability` and `Rejected` committed nothing (`NoEffect`); `Failed`
    /// is the conservative default — an app-level error may follow a partial
    /// effect, so it is `Indeterminate`.
    pub fn effect_outcome(&self) -> EffectOutcome {
        match self {
            ToolError::UnknownCapability(_) | ToolError::Rejected(..) => EffectOutcome::NoEffect,
            ToolError::Failed(..) => EffectOutcome::Indeterminate,
        }
    }
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
///
/// Each entry carries a `guarded` flag (the `bool`): a guarded tool validates the
/// call and may refuse it *before* any side effect, so its `Err` is a definitive
/// [`ToolError::Rejected`] (`NoEffect`); a plain tool's `Err` is
/// [`ToolError::Failed`] (`Indeterminate` — a failure may follow a partial effect).
#[derive(Default)]
pub struct NativeToolGateway {
    tools: HashMap<String, (NativeTool, bool)>,
}

impl NativeToolGateway {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plain native tool under a capability name. Its `Err` maps to
    /// [`ToolError::Failed`] (`Indeterminate`): the failure may follow a partial
    /// effect.
    pub fn register(
        &mut self,
        capability: impl Into<String>,
        tool: impl FnMut(&serde_json::Value) -> Result<serde_json::Value, String> + Send + 'static,
    ) {
        self.tools
            .insert(capability.into(), (Box::new(tool), false));
    }

    /// Register a *guarded* native tool: one that validates the call and may refuse
    /// it before any side effect. Its `Err` maps to [`ToolError::Rejected`]
    /// (`NoEffect`), so a refusal fails the run cleanly rather than forcing
    /// reconciliation (PRD 8.5).
    pub fn register_guarded(
        &mut self,
        capability: impl Into<String>,
        tool: impl FnMut(&serde_json::Value) -> Result<serde_json::Value, String> + Send + 'static,
    ) {
        self.tools.insert(capability.into(), (Box::new(tool), true));
    }
}

impl ToolGateway for NativeToolGateway {
    fn call(&mut self, call: &ToolCall) -> Result<serde_json::Value, ToolError> {
        let entry = self
            .tools
            .get_mut(&call.capability)
            .ok_or_else(|| ToolError::UnknownCapability(call.capability.clone()))?;
        let guarded = entry.1;
        let tool = &mut entry.0;
        tool(&call.args).map_err(|e| {
            if guarded {
                ToolError::Rejected(call.capability.clone(), e)
            } else {
                ToolError::Failed(call.capability.clone(), e)
            }
        })
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

    #[test]
    fn guarded_tool_rejection_is_no_effect() {
        let mut gw = NativeToolGateway::new();
        gw.register_guarded("validate", |_| Err("out of range".into()));
        let err = gw
            .call(&ToolCall {
                capability: "validate".into(),
                args: json!(null),
                operation_key: "k".into(),
            })
            .unwrap_err();
        assert!(matches!(err, ToolError::Rejected(..)));
        assert_eq!(err.effect_outcome(), EffectOutcome::NoEffect);
    }

    #[test]
    fn plain_tool_error_is_indeterminate() {
        let mut gw = NativeToolGateway::new();
        gw.register("charge", |_| Err("acknowledgement lost".into()));
        let err = gw
            .call(&ToolCall {
                capability: "charge".into(),
                args: json!(null),
                operation_key: "k".into(),
            })
            .unwrap_err();
        assert!(matches!(err, ToolError::Failed(..)));
        assert_eq!(err.effect_outcome(), EffectOutcome::Indeterminate);
    }

    #[test]
    fn unknown_capability_is_no_effect() {
        let mut gw = NativeToolGateway::new();
        let err = gw
            .call(&ToolCall {
                capability: "nope".into(),
                args: json!(null),
                operation_key: "k".into(),
            })
            .unwrap_err();
        assert!(matches!(err, ToolError::UnknownCapability(_)));
        assert_eq!(err.effect_outcome(), EffectOutcome::NoEffect);
    }
}
