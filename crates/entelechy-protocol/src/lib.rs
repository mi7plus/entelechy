//! Protocol version negotiation and plugin compatibility.
//!
//! PRD v11 references: section 16.6 (API/protocol/plugin compatibility), PC-1
//! (negotiation fails closed), PC-2 (no silent downgrade), PC-3 (plugin manifests
//! fail closed), Q29 (compatibility window).
//!
//! Externally consumed APIs use explicit semantic versions; server and client
//! exchange supported protocol ranges and unsupported combinations fail with a
//! machine-readable upgrade-required error (16.6). Negotiation never selects a
//! version that lacks a security-relevant guarantee unless the operator explicitly
//! opted in, and the downgrade is recorded (PC-2).
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A protocol version (major.minor). Breaking changes bump major (16.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    /// Major version; a change is breaking.
    pub major: u32,
    /// Minor version; backward-compatible additions.
    pub minor: u32,
}

impl ProtocolVersion {
    /// Construct a version.
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl std::fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// An inclusive supported version range advertised during connection (16.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRange {
    /// Minimum supported version (inclusive).
    pub min: ProtocolVersion,
    /// Maximum supported version (inclusive).
    pub max: ProtocolVersion,
}

impl VersionRange {
    /// Construct a range.
    pub const fn new(min: ProtocolVersion, max: ProtocolVersion) -> Self {
        Self { min, max }
    }
    /// Whether `v` is within the range.
    pub fn contains(&self, v: ProtocolVersion) -> bool {
        self.min <= v && v <= self.max
    }
}

/// A security-relevant guarantee a protocol version may provide (PC-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecurityGuarantee {
    /// Authenticated principal/service identity.
    Authentication,
    /// Approval binding to artifact hashes.
    ApprovalBinding,
    /// Authorization checks on privileged operations.
    Authorization,
}

/// Errors from negotiation (machine-readable — 16.6).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NegotiationError {
    /// No overlapping version: upgrade required (PC-1).
    #[error("no compatible protocol version (server {server_min}..={server_max}, client {client_min}..={client_max}); upgrade required")]
    Incompatible {
        /// Server min.
        server_min: ProtocolVersion,
        /// Server max.
        server_max: ProtocolVersion,
        /// Client min.
        client_min: ProtocolVersion,
        /// Client max.
        client_max: ProtocolVersion,
    },
    /// The only common version drops a required security guarantee (PC-2).
    #[error("negotiation would silently downgrade security guarantee {0:?}; refused")]
    SecurityDowngrade(SecurityGuarantee),
}

/// The result of a successful negotiation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Negotiated {
    /// The selected version (highest common — 16.6).
    pub version: ProtocolVersion,
    /// Whether an operator-approved security downgrade was recorded (PC-2).
    pub downgrade_recorded: bool,
}

/// Negotiate the highest common protocol version, failing closed on no overlap
/// (PC-1). Security guarantees are not considered here — see
/// [`negotiate_secure`] for the PC-2 path.
pub fn negotiate(
    server: VersionRange,
    client: VersionRange,
) -> Result<ProtocolVersion, NegotiationError> {
    let max = server.max.min(client.max);
    let min = server.min.max(client.min);
    if min > max {
        return Err(NegotiationError::Incompatible {
            server_min: server.min,
            server_max: server.max,
            client_min: client.min,
            client_max: client.max,
        });
    }
    // Highest common version.
    Ok(max)
}

/// Negotiate while enforcing security guarantees (PC-2). `guarantees_of` reports
/// the guarantees a given version provides. If the selected version lacks a
/// required guarantee, the negotiation is refused unless `allow_downgrade` is set
/// (an explicit operator opt-in), in which case the downgrade is recorded.
pub fn negotiate_secure(
    server: VersionRange,
    client: VersionRange,
    required: &BTreeSet<SecurityGuarantee>,
    guarantees_of: impl Fn(ProtocolVersion) -> BTreeSet<SecurityGuarantee>,
    allow_downgrade: bool,
) -> Result<Negotiated, NegotiationError> {
    let version = negotiate(server, client)?;
    let provided = guarantees_of(version);
    let mut downgrade_recorded = false;
    for g in required {
        if !provided.contains(g) {
            if !allow_downgrade {
                return Err(NegotiationError::SecurityDowngrade(*g));
            }
            downgrade_recorded = true;
        }
    }
    Ok(Negotiated {
        version,
        downgrade_recorded,
    })
}

/// A plugin manifest (PC-3, 16.6): declares host API range, artifact schema range
/// and required capabilities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name.
    pub name: String,
    /// Host API versions the plugin supports.
    pub host_api_range: VersionRange,
    /// Artifact schema versions the plugin supports.
    pub artifact_schema_range: VersionRange,
    /// Capabilities the plugin requires (it never receives ambient authority).
    pub required_capabilities: Vec<String>,
}

/// The host environment a plugin loads into.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostEnvironment {
    /// The host's API version.
    pub host_api: ProtocolVersion,
    /// The host's artifact schema version.
    pub artifact_schema: ProtocolVersion,
    /// Capabilities the host can grant.
    pub available_capabilities: BTreeSet<String>,
}

/// Why a plugin failed to load (fails closed — PC-3).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LoadError {
    /// The host API version is outside the plugin's supported range.
    #[error("host API {0} outside plugin range")]
    HostApiIncompatible(ProtocolVersion),
    /// The artifact schema version is outside the plugin's supported range.
    #[error("artifact schema {0} outside plugin range")]
    SchemaIncompatible(ProtocolVersion),
    /// A required capability is unavailable in the host.
    #[error("required capability '{0}' unavailable")]
    MissingCapability(String),
}

impl PluginManifest {
    /// Attempt to load into a host, failing closed when compatibility cannot be
    /// established (PC-3).
    pub fn check_load(&self, host: &HostEnvironment) -> Result<(), LoadError> {
        if !self.host_api_range.contains(host.host_api) {
            return Err(LoadError::HostApiIncompatible(host.host_api));
        }
        if !self.artifact_schema_range.contains(host.artifact_schema) {
            return Err(LoadError::SchemaIncompatible(host.artifact_schema));
        }
        for cap in &self.required_capabilities {
            if !host.available_capabilities.contains(cap) {
                return Err(LoadError::MissingCapability(cap.clone()));
            }
        }
        Ok(())
    }
}

/// A deprecation record for a stable interface (PRD Q29, 16.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deprecation {
    /// The interface being deprecated.
    pub interface: String,
    /// The version in which it was first deprecated.
    pub first_deprecated: ProtocolVersion,
    /// The earliest version it may be removed in.
    pub earliest_removal: ProtocolVersion,
    /// When it was first deprecated (unix seconds).
    pub first_deprecated_at: u64,
    /// The replacement interface, if any.
    pub replacement: Option<String>,
}

/// Six months in seconds (PRD Q29 minimum window component).
pub const SIX_MONTHS_SECS: u64 = 182 * 24 * 60 * 60;

impl Deprecation {
    /// Whether removal at `candidate_version`/`now` honors the Q29 window: at
    /// least two minor releases *and* six months after first deprecation (a
    /// security emergency uses a separate documented exception, not this check).
    pub fn removal_allowed(&self, candidate_version: ProtocolVersion, now: u64) -> bool {
        let two_minors = candidate_version.major > self.first_deprecated.major
            || candidate_version.minor >= self.first_deprecated.minor + 2;
        let six_months = now.saturating_sub(self.first_deprecated_at) >= SIX_MONTHS_SECS;
        candidate_version >= self.earliest_removal && two_minors && six_months
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32) -> ProtocolVersion {
        ProtocolVersion::new(major, minor)
    }

    #[test]
    fn negotiate_picks_highest_common() {
        let server = VersionRange::new(v(1, 0), v(1, 5));
        let client = VersionRange::new(v(1, 2), v(2, 0));
        assert_eq!(negotiate(server, client).unwrap(), v(1, 5));
    }

    #[test]
    fn disjoint_ranges_fail_closed() {
        let server = VersionRange::new(v(1, 0), v(1, 2));
        let client = VersionRange::new(v(2, 0), v(2, 3));
        assert!(matches!(
            negotiate(server, client),
            Err(NegotiationError::Incompatible { .. })
        ));
    }

    #[test]
    fn no_silent_security_downgrade() {
        let server = VersionRange::new(v(1, 0), v(1, 3));
        let client = VersionRange::new(v(1, 0), v(1, 3));
        let required = BTreeSet::from([SecurityGuarantee::Authentication]);
        // v1.3 lacks authentication in this model.
        let guarantees = |ver: ProtocolVersion| {
            if ver.minor >= 2 {
                BTreeSet::new()
            } else {
                BTreeSet::from([SecurityGuarantee::Authentication])
            }
        };
        // Refused without opt-in (PC-2).
        assert_eq!(
            negotiate_secure(server, client, &required, guarantees, false),
            Err(NegotiationError::SecurityDowngrade(
                SecurityGuarantee::Authentication
            ))
        );
        // Allowed with recorded downgrade.
        let n = negotiate_secure(server, client, &required, guarantees, true).unwrap();
        assert!(n.downgrade_recorded);
    }

    #[test]
    fn plugin_load_fails_closed() {
        let manifest = PluginManifest {
            name: "retriever".into(),
            host_api_range: VersionRange::new(v(1, 0), v(1, 4)),
            artifact_schema_range: VersionRange::new(v(1, 0), v(1, 0)),
            required_capabilities: vec!["read_kb".into()],
        };
        let mut host = HostEnvironment {
            host_api: v(1, 2),
            artifact_schema: v(1, 0),
            available_capabilities: BTreeSet::from(["read_kb".to_string()]),
        };
        assert!(manifest.check_load(&host).is_ok());
        // Host API too new.
        host.host_api = v(2, 0);
        assert!(matches!(
            manifest.check_load(&host),
            Err(LoadError::HostApiIncompatible(_))
        ));
        // Missing capability.
        host.host_api = v(1, 2);
        host.available_capabilities.clear();
        assert!(matches!(
            manifest.check_load(&host),
            Err(LoadError::MissingCapability(_))
        ));
    }

    #[test]
    fn deprecation_window_enforced() {
        let dep = Deprecation {
            interface: "old_endpoint".into(),
            first_deprecated: v(1, 2),
            earliest_removal: v(1, 4),
            first_deprecated_at: 0,
            replacement: Some("new_endpoint".into()),
        };
        // Too soon: only one minor later and before six months.
        assert!(!dep.removal_allowed(v(1, 3), 1000));
        // Two minors later and past six months.
        assert!(dep.removal_allowed(v(1, 4), SIX_MONTHS_SECS + 1));
    }
}
