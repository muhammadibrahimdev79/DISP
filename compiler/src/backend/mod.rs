pub mod abi;
pub mod allocator;
pub mod codegen;
pub mod layout;
pub mod linker;
pub mod mono;
pub mod native_types;
pub mod runtime;
pub mod target;
pub mod typed_codegen;

use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    hir, mir,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    pub optimized: bool,
    pub emit_c: bool,
    pub emit_object: bool,
}
#[derive(Debug, Clone)]
pub struct BuildArtifacts {
    pub executable: PathBuf,
    pub object: Option<PathBuf>,
    pub backend_ir: Option<PathBuf>,
}

pub fn build(
    hir: &hir::Program,
    mir: &mir::Program,
    source_path: &Path,
    options: BuildOptions,
) -> Result<BuildArtifacts, Diagnostic> {
    mir::validate(mir)?;
    let target = target::Target::host()?;
    let mono = mono::collect(mir)?;
    validate_layouts(hir, mir, &mono, target)?;
    let abi = abi::lower(hir, mir, &mono, target)?;
    let native_types = native_types::generate(hir, &mono, target)?;
    let generated = codegen::generate(mir, &mono, &abi, &native_types)?;
    let stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| error("source file has no valid output name"))?;
    let safe_stem = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let build_dir = parent.join("build").join(&safe_stem);
    fs::create_dir_all(&build_dir)
        .map_err(|cause| error(&format!("could not create native build directory: {cause}")))?;
    let c_path = build_dir.join(format!("{safe_stem}.backend.c"));
    let object_path = build_dir.join(format!("{safe_stem}.o"));
    let executable_path = parent.join("build").join(format!("{safe_stem}.exe"));
    fs::write(&c_path, generated.source)
        .map_err(|cause| error(&format!("could not write backend C: {cause}")))?;
    linker::compile_and_link(&c_path, &object_path, &executable_path, options.optimized)?;
    if !options.emit_c {
        let _ = fs::remove_file(&c_path);
    }
    if !options.emit_object {
        let _ = fs::remove_file(&object_path);
    }
    Ok(BuildArtifacts {
        executable: executable_path,
        object: options.emit_object.then_some(object_path),
        backend_ir: options.emit_c.then_some(c_path),
    })
}

fn validate_layouts(
    program: &hir::Program,
    mir: &mir::Program,
    mono: &mono::MonoProgram,
    target: target::Target,
) -> Result<(), Diagnostic> {
    let mut engine = layout::LayoutEngine::new(target, program);
    for instance in &mono.instances {
        let function = &mir.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        for local in &function.locals {
            engine.layout(&layout::substitute(&local.ty, &substitutions))?;
        }
    }
    Ok(())
}
fn error(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}
