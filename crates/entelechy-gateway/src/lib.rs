//! Gateways: model, tool and (later) secret/policy boundaries (PRD 8.3).
//!
//! PRD v11 references: sections 8.3 (gateways), 5.10 (provider identity), 7.5
//! (model calls are egress), 17.4 (degraded mode & fallbacks).
//!
//! Gateways are the single choke points where the runtime meets the external
//! capability plane. Every model call is an egress effect; every tool call is
//! journaled and capability-checked.
#![forbid(unsafe_code)]

pub mod model;
pub mod sandbox;
pub mod tool;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use model::{MockModel, ModelError, ModelGateway, ModelRequest, ModelResponse, ProviderIdentity};
pub use sandbox::{
    isolation_for, HostError, Isolation, LoadedPlugin, PluginHost, RiskLevel, SandboxEngine,
    SandboxError, SandboxScope,
};
pub use tool::{NativeToolGateway, ToolCall, ToolError, ToolGateway};

#[cfg(feature = "wasm")]
pub use wasm::WasmSandbox;
