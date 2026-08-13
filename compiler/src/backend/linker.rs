use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::{path::Path, process::Command};

pub fn compile_and_link(
    c_source: &Path,
    object: &Path,
    executable: &Path,
    optimized: bool,
    networking: bool,
    http: bool,
    libraries: &[String],
) -> Result<(), Diagnostic> {
    let mut compile = vec![
        "-std=c11".to_string(),
        if optimized { "-O2" } else { "-O0" }.to_string(),
        "-g".to_string(),
        "-ffunction-sections".to_string(),
        "-fdata-sections".to_string(),
    ];
    if !cfg!(windows) {
        compile.push("-pthread".into());
    }
    if networking {
        compile.push("-DDISP_NETWORKING".into());
    }
    if http {
        compile.push("-DDISP_HTTP".into());
    }
    compile.extend([
        "-c".into(),
        path(c_source)?.into(),
        "-o".into(),
        path(object)?.into(),
    ]);
    run_gcc(&compile, "C compilation")?;
    let mut link = vec![
        path(object)?.into(),
        "-o".into(),
        path(executable)?.into(),
        "-Wl,--gc-sections".into(),
        "-lm".into(),
    ];
    if !cfg!(windows) {
        link.push("-pthread".into());
    } else {
        link.push("-lshell32".into());
        if networking {
            link.push("-lws2_32".into());
            link.push("-lsecur32".into());
            link.push("-lcrypt32".into());
            if http {
                link.push("-lwinhttp".into());
            }
        }
    }
    for library in libraries {
        link.push(format!("-l{library}"));
    }
    run_gcc(&link, "native linking")
}

fn path(path: &Path) -> Result<&str, Diagnostic> {
    path.to_str()
        .ok_or_else(|| error("native toolchain cannot represent a non-UTF-8 path"))
}
fn run_gcc(arguments: &[String], phase: &str) -> Result<(), Diagnostic> {
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
