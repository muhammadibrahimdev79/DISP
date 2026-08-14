use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy)]
pub struct RuntimeFeatures {
    pub networking: bool,
    pub http: bool,
    pub database: bool,
}

pub fn compile_and_link(
    c_source: &Path,
    object: &Path,
    executable: &Path,
    optimized: bool,
    features: RuntimeFeatures,
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
    if features.networking {
        compile.push("-DDISP_NETWORKING".into());
    }
    if features.http {
        compile.push("-DDISP_HTTP".into());
    }
    if features.database {
        compile.push("-DDISP_DATABASE".into());
    }
    compile.extend([
        "-c".into(),
        path(c_source)?.into(),
        "-o".into(),
        path(object)?.into(),
    ]);
    run_native_cc(&compile, "C compilation")?;
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
        if features.networking {
            link.push("-lws2_32".into());
            link.push("-lsecur32".into());
            link.push("-lcrypt32".into());
            if features.http {
                link.push("-lwinhttp".into());
            }
        }
        if features.database {
            link.push(windows_sqlite_library()?);
        }
    }
    if features.database && !cfg!(windows) {
        link.push("-lsqlite3".into());
    }
    for library in libraries {
        link.push(format!("-l{library}"));
    }
    run_native_cc(&link, "native linking")
}

fn windows_sqlite_library() -> Result<String, Diagnostic> {
    let windows = std::env::var_os("WINDIR")
        .ok_or_else(|| error("WINDIR is unavailable for Windows SQLite linking"))?;
    let library = Path::new(&windows).join("System32").join("winsqlite3.dll");
    if !library.is_file() {
        return Err(error(&format!(
            "Windows system SQLite is unavailable at `{}`",
            library.display()
        )));
    }
    path(&library).map(str::to_owned)
}

fn path(path: &Path) -> Result<&str, Diagnostic> {
    path.to_str()
        .ok_or_else(|| error("native toolchain cannot represent a non-UTF-8 path"))
}
fn bundled_zig() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let directory = executable.parent()?;
    let candidate = directory
        .join("toolchain")
        .join(if cfg!(windows) { "zig.exe" } else { "zig" });
    candidate.is_file().then_some(candidate)
}

fn native_cc() -> (OsString, Vec<OsString>, &'static str) {
    if let Some(path) = env::var_os("DISP_ZIG").map(PathBuf::from)
        && path.is_file()
    {
        return (path.into_os_string(), vec!["cc".into()], "bundled Zig");
    }
    if let Some(path) = bundled_zig() {
        return (path.into_os_string(), vec!["cc".into()], "bundled Zig");
    }
    if let Some(command) = env::var_os("DISP_CC") {
        return (command, Vec::new(), "configured C compiler");
    }
    ("gcc".into(), Vec::new(), "GCC")
}

fn run_native_cc(arguments: &[String], phase: &str) -> Result<(), Diagnostic> {
    let (program, prefix, description) = native_cc();
    let output = Command::new(&program)
        .args(prefix)
        .args(arguments)
        .output()
        .map_err(|cause| {
            error(&format!(
                "{description} is unavailable for {phase}: {cause}"
            ))
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(error(&format!(
        "{phase} failed with {description}:\n{}",
        stderr.trim()
    )))
}
fn error(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}
