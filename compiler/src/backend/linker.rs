use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    process_sandbox::{SandboxProfile, SandboxedCommand},
};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct RuntimeFeatures {
    pub networking: bool,
    pub http: bool,
    pub database: bool,
    pub data: bool,
    pub native_crypto_library: Option<PathBuf>,
    pub shared: bool,
}

pub fn compile_and_link(
    c_source: &Path,
    object: &Path,
    executable: &Path,
    optimized: bool,
    sanitizers: bool,
    features: RuntimeFeatures,
    libraries: &[String],
) -> Result<(), Diagnostic> {
    let msvc_driver = cfg!(windows) && native_cc_targets_msvc();
    let mut compile = vec![
        "-std=c11".to_string(),
        if optimized { "-O2" } else { "-O0" }.to_string(),
        "-g".to_string(),
        "-ffunction-sections".to_string(),
        "-fdata-sections".to_string(),
    ];
    if !cfg!(windows) {
        compile.push("-pthread".into());
        if features.shared {
            compile.push("-fPIC".into());
        }
    }
    if sanitizers {
        compile.push("-fsanitize=address,undefined".into());
        compile.push("-fno-omit-frame-pointer".into());
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
    if features.data {
        compile.push("-DDISP_DATA".into());
    }
    if features.native_crypto_library.is_some() {
        compile.push("-DDISP_CRYPTO_NATIVE".into());
    }
    compile.extend([
        "-c".into(),
        path(c_source)?.into(),
        "-o".into(),
        path(object)?.into(),
    ]);
    run_native_cc(&compile, "C compilation")?;
    let mut link = vec![path(object)?.into(), "-o".into(), path(executable)?.into()];
    if features.shared {
        link.push(if cfg!(target_os = "macos") {
            "-dynamiclib".into()
        } else {
            "-shared".into()
        });
    }
    if !msvc_driver {
        link.push("-Wl,--gc-sections".into());
        link.push("-lm".into());
    }
    if !cfg!(windows) {
        link.push("-pthread".into());
        if features.http {
            link.push("-lcurl".into());
        }
    } else {
        link.push("-lshell32".into());
        link.push("-lbcrypt".into());
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
    if sanitizers {
        link.push("-fsanitize=address,undefined".into());
    }
    if features.database && !cfg!(windows) {
        link.push("-lsqlite3".into());
    }
    if let Some(runtime) = features.native_crypto_library.as_deref() {
        if cfg!(windows) {
            let import_library = runtime.with_extension("dll.lib");
            if !import_library.is_file() {
                return Err(error(&format!(
                    "native cryptography import library `{}` is missing",
                    import_library.display()
                )));
            }
            link.push(path(&import_library)?.into());
        } else {
            link.push(path(runtime)?.into());
            if cfg!(target_os = "linux") {
                link.push("-Wl,-rpath,$ORIGIN".into());
            } else if cfg!(target_os = "macos") {
                link.push("-Wl,-rpath,@loader_path".into());
            }
        }
    }
    for library in libraries {
        link.push(format!("-l{library}"));
    }
    run_native_cc(&link, "native linking")?;
    if sanitizers && msvc_driver {
        stage_msvc_sanitizer_runtime(executable)?;
    }
    Ok(())
}

fn stage_msvc_sanitizer_runtime(executable: &Path) -> Result<(), Diagnostic> {
    let (program, prefix, description) = native_cc();
    let mut command = SandboxedCommand::new(&program);
    command.args(prefix).arg("--print-runtime-dir");
    let output = command.output(SandboxProfile::Toolchain).map_err(|cause| {
        error(&format!(
            "{description} could not locate its sanitizer runtime: {cause}"
        ))
    })?;
    if !output.status.success() {
        return Err(error(&format!(
            "{description} could not locate its sanitizer runtime:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let runtime_directory = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let runtime = fs::read_dir(&runtime_directory)
        .map_err(|cause| {
            error(&format!(
                "could not inspect sanitizer runtime directory `{}`: {cause}",
                runtime_directory.display()
            ))
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("clang_rt.asan_dynamic-") && name.ends_with(".dll")
                })
        })
        .ok_or_else(|| {
            error(&format!(
                "{description} has no dynamic ASan runtime in `{}`",
                runtime_directory.display()
            ))
        })?;
    let destination = executable
        .parent()
        .ok_or_else(|| error("native executable has no output directory"))?
        .join(runtime.file_name().unwrap());
    fs::copy(&runtime, &destination).map_err(|cause| {
        error(&format!(
            "could not stage sanitizer runtime `{}` beside `{}`: {cause}",
            runtime.display(),
            executable.display()
        ))
    })?;
    Ok(())
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

fn native_cc_targets_msvc() -> bool {
    let (program, prefix, _) = native_cc();
    let mut command = SandboxedCommand::new(program);
    command.args(prefix).arg("-dumpmachine");
    command
        .output(SandboxProfile::Toolchain)
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .ends_with("windows-msvc")
        })
}

fn run_native_cc(arguments: &[String], phase: &str) -> Result<(), Diagnostic> {
    let (program, prefix, description) = native_cc();
    let mut command = SandboxedCommand::new(&program);
    command.args(prefix).args(arguments);
    let output = command.output(SandboxProfile::Toolchain).map_err(|cause| {
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
