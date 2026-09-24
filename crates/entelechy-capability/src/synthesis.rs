//! Tool synthesis governance (PRD 13.2, TS-3, TS-4).
//!
//! Generated tool code requires tests, property tests, static/dependency scans and
//! approval before it may be used for consequential effects (TS-3); tools are
//! versioned with provenance and rollback (TS-4). This module models that
//! governance — the code generation itself (TS-1, from OpenAPI/spec/examples) is a
//! model-in-the-loop step that produces a [`GeneratedTool`] this gate then admits
//! or rejects.

use serde::{Deserialize, Serialize};

use entelechy_ir::EffectClass;

/// The verification checks a generated tool has passed (TS-3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolChecks {
    /// Unit tests generated and passing.
    pub unit_tests: bool,
    /// Property tests generated and passing.
    pub property_tests: bool,
    /// Static analysis clean.
    pub static_scan: bool,
    /// Dependency/supply-chain scan clean.
    pub dependency_scan: bool,
    /// Human approval for consequential effects (TS-3).
    pub approved_for_consequential: bool,
}

/// A generated tool version with provenance (TS-1, TS-4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedTool {
    /// Tool name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Provenance (source spec / generator identity) for TS-4.
    pub provenance: String,
    /// Verification checks passed.
    pub checks: ToolChecks,
}

impl GeneratedTool {
    /// Whether the tool is eligible to run for the given effect class (TS-2/TS-3).
    /// Read-only effects require at least passing unit tests; consequential
    /// effects require the full check set plus approval. Returns the list of
    /// missing checks, or `Ok(())` when eligible.
    pub fn eligible_for(&self, class: EffectClass) -> Result<(), Vec<&'static str>> {
        let mut missing = Vec::new();
        if !self.checks.unit_tests {
            missing.push("unit_tests");
        }
        if class.is_consequential() {
            if !self.checks.property_tests {
                missing.push("property_tests");
            }
            if !self.checks.static_scan {
                missing.push("static_scan");
            }
            if !self.checks.dependency_scan {
                missing.push("dependency_scan");
            }
            if !self.checks.approved_for_consequential {
                missing.push("approval");
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(missing)
        }
    }
}

/// A versioned tool with rollback (TS-4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolVersions {
    /// All published versions, oldest first.
    versions: Vec<GeneratedTool>,
    /// Index of the currently active version.
    current: usize,
}

/// Errors from tool version management.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VersionError {
    /// No such version exists.
    #[error("version '{0}' not found")]
    NotFound(String),
}

impl ToolVersions {
    /// Create a version history from an initial tool.
    pub fn new(initial: GeneratedTool) -> Self {
        Self {
            versions: vec![initial],
            current: 0,
        }
    }

    /// Publish a new version, making it current (TS-4).
    pub fn publish(&mut self, tool: GeneratedTool) {
        self.versions.push(tool);
        self.current = self.versions.len() - 1;
    }

    /// The currently active version.
    pub fn current(&self) -> &GeneratedTool {
        &self.versions[self.current]
    }

    /// Roll back to a previous version by version string (TS-4). The rolled-back
    /// version becomes current; history is preserved.
    pub fn rollback_to(&mut self, version: &str) -> Result<&GeneratedTool, VersionError> {
        match self.versions.iter().position(|v| v.version == version) {
            Some(idx) => {
                self.current = idx;
                Ok(&self.versions[idx])
            }
            None => Err(VersionError::NotFound(version.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(version: &str, checks: ToolChecks) -> GeneratedTool {
        GeneratedTool {
            name: "invoice_api".into(),
            version: version.into(),
            provenance: "openapi:invoices.yaml".into(),
            checks,
        }
    }

    #[test]
    fn read_only_needs_only_unit_tests() {
        let t = tool("1.0.0", ToolChecks { unit_tests: true, ..Default::default() });
        assert!(t.eligible_for(EffectClass::Read).is_ok());
        // But a consequential effect is missing the rest (TS-3).
        assert!(t.eligible_for(EffectClass::Write).is_err());
    }

    #[test]
    fn consequential_needs_full_checks_and_approval() {
        let full = ToolChecks {
            unit_tests: true,
            property_tests: true,
            static_scan: true,
            dependency_scan: true,
            approved_for_consequential: true,
        };
        assert!(tool("1.0.0", full).eligible_for(EffectClass::Irreversible).is_ok());

        let no_approval = ToolChecks { approved_for_consequential: false, ..full };
        let missing = tool("1.0.0", no_approval).eligible_for(EffectClass::Write).unwrap_err();
        assert!(missing.contains(&"approval"));
    }

    #[test]
    fn versioning_and_rollback() {
        let full = ToolChecks {
            unit_tests: true,
            property_tests: true,
            static_scan: true,
            dependency_scan: true,
            approved_for_consequential: true,
        };
        let mut v = ToolVersions::new(tool("1.0.0", full));
        v.publish(tool("1.1.0", full));
        assert_eq!(v.current().version, "1.1.0");
        // Roll back to the prior version (TS-4).
        assert_eq!(v.rollback_to("1.0.0").unwrap().version, "1.0.0");
        assert_eq!(v.current().version, "1.0.0");
        assert!(v.rollback_to("9.9.9").is_err());
    }
}
