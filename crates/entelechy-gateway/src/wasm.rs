//! A real WASM sandbox engine backed by `wasmtime` (PRD 8.3, 16.3, TS-2).
//!
//! This is the concrete execution backend behind [`crate::sandbox::SandboxEngine`].
//! It enforces the no-ambient-authority contract structurally: the module is
//! instantiated with an **empty linker**, so a guest that imports any host
//! function fails to instantiate — a WASM plugin can compute but cannot touch the
//! outside world except through capabilities the host explicitly links (none by
//! default). Execution is bounded by fuel, so a runaway or infinite-loop guest is
//! trapped rather than hanging (a resource bound in the spirit of IR-I4/RK).
//!
//! Enabled with the `wasm` feature. Guest ABI: export `fn run(i64) -> i64`.

use wasmtime::{Config, Engine, Linker, Module, Store};

use crate::sandbox::{SandboxEngine, SandboxError, SandboxScope};

/// Default fuel budget for a sandboxed call (bounds execution — PRD 8.3).
pub const DEFAULT_FUEL: u64 = 10_000_000;

/// A WASM execution sandbox (PRD 8.3). Accepts `.wat` or `.wasm` bytes.
pub struct WasmSandbox {
    engine: Engine,
    fuel: u64,
}

impl WasmSandbox {
    /// Create a sandbox with the default fuel budget.
    pub fn new() -> Result<Self, SandboxError> {
        Self::with_fuel(DEFAULT_FUEL)
    }

    /// Create a sandbox with an explicit fuel budget.
    pub fn with_fuel(fuel: u64) -> Result<Self, SandboxError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        // No WASI, no ambient imports: a pure compute sandbox (no ambient
        // authority — PRD 16.3).
        let engine = Engine::new(&config).map_err(|e| SandboxError::Engine(e.to_string()))?;
        Ok(Self { engine, fuel })
    }
}

impl SandboxEngine for WasmSandbox {
    fn execute(
        &mut self,
        code: &[u8],
        _scope: &SandboxScope,
        arg: i64,
    ) -> Result<i64, SandboxError> {
        // `Module::new` accepts both binary `.wasm` and text `.wat` (the `wat`
        // feature). Compilation failure is an engine error.
        let module =
            Module::new(&self.engine, code).map_err(|e| SandboxError::Engine(e.to_string()))?;

        let mut store = Store::new(&self.engine, ());
        store
            .set_fuel(self.fuel)
            .map_err(|e| SandboxError::Engine(e.to_string()))?;

        // Empty linker: any host import the guest declares is unsatisfiable, so a
        // guest cannot obtain ambient authority (PRD 16.3).
        let linker: Linker<()> = Linker::new(&self.engine);
        let instance = linker.instantiate(&mut store, &module).map_err(|e| {
            SandboxError::Engine(format!(
                "instantiate failed (ambient imports are refused): {e}"
            ))
        })?;

        let run = instance
            .get_typed_func::<i64, i64>(&mut store, "run")
            .map_err(|e| SandboxError::Engine(format!("guest must export `run(i64)->i64`: {e}")))?;

        match run.call(&mut store, arg) {
            Ok(result) => Ok(result),
            Err(trap) => {
                // Distinguish fuel exhaustion (a resource bound) from other traps.
                if store.get_fuel().map(|f| f == 0).unwrap_or(false) {
                    Err(SandboxError::ResourceExhausted)
                } else {
                    Err(SandboxError::Engine(format!("guest trapped: {trap}")))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn scope() -> SandboxScope {
        SandboxScope {
            capabilities: BTreeSet::new(),
            isolation: crate::sandbox::Isolation::Wasm,
            allow_network: false,
        }
    }

    #[test]
    fn runs_a_pure_wasm_function() {
        // A guest that doubles its input.
        let wat = r#"
            (module
              (func (export "run") (param i64) (result i64)
                local.get 0
                i64.const 2
                i64.mul))
        "#;
        let mut sandbox = WasmSandbox::new().unwrap();
        let out = sandbox.execute(wat.as_bytes(), &scope(), 21).unwrap();
        assert_eq!(out, 42);
    }

    #[test]
    fn a_guest_importing_host_functions_cannot_instantiate() {
        // The guest tries to import a host function; the empty linker refuses it,
        // so it never runs — no ambient authority (PRD 16.3).
        let wat = r#"
            (module
              (import "env" "exfiltrate" (func $exfil (param i64)))
              (func (export "run") (param i64) (result i64)
                local.get 0
                call $exfil
                local.get 0))
        "#;
        let mut sandbox = WasmSandbox::new().unwrap();
        let err = sandbox.execute(wat.as_bytes(), &scope(), 1).unwrap_err();
        assert!(matches!(err, SandboxError::Engine(_)), "{err:?}");
    }

    #[test]
    fn an_infinite_loop_is_bounded_by_fuel() {
        // A guest that loops forever is trapped once fuel runs out.
        let wat = r#"
            (module
              (func (export "run") (param i64) (result i64)
                (loop $l (br $l))
                local.get 0))
        "#;
        let mut sandbox = WasmSandbox::with_fuel(100_000).unwrap();
        let err = sandbox.execute(wat.as_bytes(), &scope(), 1).unwrap_err();
        assert_eq!(
            format!("{err:?}"),
            format!("{:?}", SandboxError::ResourceExhausted)
        );
    }
}
