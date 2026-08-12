use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::{path::Path, process::Command};

pub fn compile_and_link(
    c_source: &Path,
    object: &Path,
    executable: &Path,
    optimized: bool,
) -> Result<(), Diagnostic> {
    let mut compile = vec![
        "-std=c11",
        if optimized { "-O2" } else { "-O0" },
        "-g",
        "-ffunction-sections",
        "-fdata-sections",
    ];
    if !cfg!(windows) {
        compile.push("-pthread");
    }
    compile.extend(["-c", path(c_source)?, "-o", path(object)?]);
    run_gcc(&compile, "C compilation")?;
    let mut link = vec![
        path(object)?,
        "-o",
        path(executable)?,
        "-Wl,--gc-sections",
        "-lm",
    ];
    if !cfg!(windows) {
        link.push("-pthread");
    }
    run_gcc(&link, "native linking")
}

fn path(path: &Path) -> Result<&str, Diagnostic> {
    path.to_str()
        .ok_or_else(|| error("native toolchain cannot represent a non-UTF-8 path"))
}
fn run_gcc(arguments: &[&str], phase: &str) -> Result<(), Diagnostic> {
    let output = Command::new("gcc")
        .args(arguments)
        .output()
        .map_err(|cause| error(&format!("GCC is unavailable for {phase}: {cause}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(error(&format!("{phase} failed:\n{}", stderr.trim())))
}
fn error(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}
