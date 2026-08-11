use super::{abi::AbiProgram, mono::MonoProgram, typed_codegen};
use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    mir,
};

pub struct GeneratedC {
    pub source: String,
}

pub fn generate(
    program: &mir::Program,
    mono_program: &MonoProgram,
    abi: &AbiProgram,
    native_types: &str,
) -> Result<GeneratedC, Diagnostic> {
    let source =
        typed_codegen::generate(program, mono_program, abi, native_types)?.ok_or_else(|| {
            Diagnostic::new(
                DiagnosticKind::Backend,
                "MIR contains a type that has no concrete native representation",
                Span::point(1, 1),
            )
        })?;
    Ok(GeneratedC { source })
}
