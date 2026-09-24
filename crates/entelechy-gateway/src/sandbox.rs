//! Plugin host and sandbox authority model (PRD 16.3, 8.3, TS-2, TS-3).
//!
//! WASM component plugins declare required capabilities and never receive ambient
//! authority (PRD 16.3). Generated tools/code run only in declared sandbox scopes
//! (TS-2), isolated by risk class (PRD 8.3: WASM, container or microVM). This
//! module enforces that contract: a plugin is granted exactly the capabilities
//! its manifest declares *and* an operator has approved, and any call outside
//! that scope is denied — even if the underlying [`ToolGateway`] has the
//! capability registered.
//!
//! The actual execution engine (e.g. a `wasmtime` backend) plugs in behind this
//! boundary without changing the authority model; this crate provides the
//! dependency-free authority and scope enforcement.

use std::collections::BTreeSet;

use entelechy_protocol::PluginManifest;

use crate::tool::{ToolCall, ToolError, ToolGateway};

/// The isolation mechanism for a sandbox, chosen by risk class (PRD 8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    /// WASM isolation (lowest overhead).
    Wasm,
    /// Container isolation.
    Container,
    /// microVM isolation (strongest).
    MicroVm,
}

/// The risk class of a generated tool / plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskLevel {
    /// Low risk.
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    High,
}

/// Select the isolation mechanism for a risk class (PRD 8.3: isolation chosen by
/// risk class).
pub fn isolation_for(risk: RiskLevel) -> Isolation {
    match risk {
        RiskLevel::Low => Isolation::Wasm,
        RiskLevel::Medium => Isolation::Container,
        RiskLevel::High => Isolation::MicroVm,
    }
}

/// The declared scope a plugin runs within (TS-2). Default-deny: only the listed
/// capabilities are reachable, and network/filesystem are off unless granted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxScope {
    /// Capabilities the plugin may invoke (no ambient authority beyond these).
    pub capabilities: BTreeSet<String>,
    /// Isolation mechanism.
    pub isolation: Isolation,
    /// Whether the plugin may perform network egress.
    pub allow_network: bool,
}

/// Errors from loading a plugin (fail closed — TS-3, 16.3).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HostError {
    /// The plugin requires a capability the operator has not granted (no ambient
    /// authority — PRD 16.3).
    #[error("plugin '{plugin}' requires ungranted capability '{capability}'")]
    CapabilityNotGranted {
        /// Plugin name.
        plugin: String,
        /// The ungranted capability.
        capability: String,
    },
}

/// Errors from invoking a plugin capability or executing sandboxed code.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The capability is outside the plugin's declared scope (PRD 16.3).
    #[error("capability '{0}' is outside the plugin's sandbox scope")]
    CapabilityDenied(String),
    /// The underlying tool failed.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// The sandboxed code failed to load, instantiate or run.
    #[error("sandbox execution error: {0}")]
    Engine(String),
    /// The sandboxed code exhausted its resource bound (e.g. fuel — bounds like
    /// IR-I4/RK apply inside the sandbox too).
    #[error("sandbox resource limit exceeded")]
    ResourceExhausted,
}

/// A loaded plugin bound to its sandbox scope (PRD 16.3).
#[derive(Clone, Debug)]
pub struct LoadedPlugin {
    /// Plugin name.
    pub name: String,
    /// The sandbox scope it runs within.
    pub scope: SandboxScope,
}

impl LoadedPlugin {
    /// Whether the plugin may invoke a capability (only its declared scope — no
    /// ambient authority, PRD 16.3).
    pub fn may_invoke(&self, capability: &str) -> bool {
        self.scope.capabilities.contains(capability)
    }

    /// Invoke a capability through a [`ToolGateway`], enforcing the sandbox scope
    /// first (PRD 16.3): a call outside scope is denied even if the gateway has
    /// the capability registered.
    pub fn invoke(
        &self,
        gateway: &mut dyn ToolGateway,
        call: &ToolCall,
    ) -> Result<serde_json::Value, SandboxError> {
        if !self.may_invoke(&call.capability) {
            return Err(SandboxError::CapabilityDenied(call.capability.clone()));
        }
        Ok(gateway.call(call)?)
    }
}

/// A sandbox execution engine (PRD 8.3): runs generated tool/plugin code inside a
/// declared scope. The authority model is enforced by the caller/scope; an engine
/// implementation (e.g. WASM via `wasmtime`, container, microVM) supplies the
/// actual isolated execution and must not grant any authority beyond the scope.
pub trait SandboxEngine {
    /// Execute `code` within `scope`, passing a single integer argument and
    /// returning a single integer result. This intentionally minimal contract is
    /// enough to prove isolated, no-ambient-authority execution; richer byte/JSON
    /// ABIs layer on top.
    fn execute(&mut self, code: &[u8], scope: &SandboxScope, arg: i64)
        -> Result<i64, SandboxError>;
}

/// The plugin host: loads plugins under the no-ambient-authority contract (PRD
/// 16.3).
pub struct PluginHost;

impl PluginHost {
    /// Load a plugin, granting exactly the capabilities its manifest declares —
    /// and only if every one is in the operator-approved `granted` set (PRD 16.3,
    /// TS-3). Fails closed on any ungranted capability.
    pub fn load(
        manifest: &PluginManifest,
        granted: &BTreeSet<String>,
        risk: RiskLevel,
    ) -> Result<LoadedPlugin, HostError> {
        for cap in &manifest.required_capabilities {
            if !granted.contains(cap) {
                return Err(HostError::CapabilityNotGranted {
                    plugin: manifest.name.clone(),
                    capability: cap.clone(),
                });
            }
        }
        Ok(LoadedPlugin {
            name: manifest.name.clone(),
            scope: SandboxScope {
                capabilities: manifest.required_capabilities.iter().cloned().collect(),
                isolation: isolation_for(risk),
                allow_network: false,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::NativeToolGateway;
    use entelechy_protocol::{ProtocolVersion, VersionRange};

    fn manifest(caps: &[&str]) -> PluginManifest {
        PluginManifest {
            name: "retriever".into(),
            host_api_range: VersionRange::new(
                ProtocolVersion::new(1, 0),
                ProtocolVersion::new(1, 9),
            ),
            artifact_schema_range: VersionRange::new(
                ProtocolVersion::new(1, 0),
                ProtocolVersion::new(1, 0),
            ),
            required_capabilities: caps.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn granted(caps: &[&str]) -> BTreeSet<String> {
        caps.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ungranted_capability_fails_closed() {
        let err = PluginHost::load(
            &manifest(&["read_kb", "write_kb"]),
            &granted(&["read_kb"]),
            RiskLevel::Low,
        )
        .unwrap_err();
        assert!(matches!(err, HostError::CapabilityNotGranted { .. }));
    }

    #[test]
    fn loads_with_declared_scope_and_isolation() {
        let p = PluginHost::load(
            &manifest(&["read_kb"]),
            &granted(&["read_kb", "extra"]),
            RiskLevel::High,
        )
        .unwrap();
        assert!(p.may_invoke("read_kb"));
        // No ambient authority: a capability the operator granted but the plugin
        // did not declare is still not reachable.
        assert!(!p.may_invoke("extra"));
        assert_eq!(p.scope.isolation, Isolation::MicroVm);
    }

    #[test]
    fn invoke_denies_out_of_scope_even_if_gateway_has_it() {
        let mut gw = NativeToolGateway::new();
        gw.register("read_kb", |_| Ok(serde_json::json!({ "ok": true })));
        gw.register("secret_exfil", |_| {
            Ok(serde_json::json!({ "leaked": true }))
        });

        let plugin = PluginHost::load(
            &manifest(&["read_kb"]),
            &granted(&["read_kb"]),
            RiskLevel::Low,
        )
        .unwrap();

        // In-scope call works.
        let ok = plugin.invoke(
            &mut gw,
            &ToolCall {
                capability: "read_kb".into(),
                args: serde_json::Value::Null,
                operation_key: "k".into(),
            },
        );
        assert!(ok.is_ok());

        // Out-of-scope call is denied even though the gateway has it (no ambient
        // authority — PRD 16.3).
        let denied = plugin.invoke(
            &mut gw,
            &ToolCall {
                capability: "secret_exfil".into(),
                args: serde_json::Value::Null,
                operation_key: "k".into(),
            },
        );
        assert!(matches!(denied, Err(SandboxError::CapabilityDenied(_))));
    }

    #[test]
    fn isolation_by_risk_class() {
        assert_eq!(isolation_for(RiskLevel::Low), Isolation::Wasm);
        assert_eq!(isolation_for(RiskLevel::Medium), Isolation::Container);
        assert_eq!(isolation_for(RiskLevel::High), Isolation::MicroVm);
    }
}
