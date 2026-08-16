use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
pub const FILE_NAME: &str = "disp_crypto_native.dll";
#[cfg(target_os = "linux")]
pub const FILE_NAME: &str = "libdisp_crypto_native.so";
#[cfg(target_os = "macos")]
pub const FILE_NAME: &str = "libdisp_crypto_native.dylib";
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub const FILE_NAME: &str = "disp_crypto_native.unsupported";

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}

fn resolve(path: &Path) -> Result<PathBuf, Diagnostic> {
    let resolved = fs::canonicalize(path).map_err(|cause| {
        error(format!(
            "could not resolve native cryptography runtime `{}`: {cause}",
            path.display()
        ))
    })?;
    #[cfg(windows)]
    if let Some(ordinary) = resolved
        .to_str()
        .and_then(|value| value.strip_prefix(r"\\?\"))
    {
        return Ok(PathBuf::from(ordinary));
    }
    Ok(resolved)
}

pub fn locate() -> Result<PathBuf, Diagnostic> {
    let executable = env::current_exe().map_err(|cause| {
        error(format!(
            "could not locate the running DISP compiler: {cause}"
        ))
    })?;
    let directory = executable
        .parent()
        .ok_or_else(|| error("the running DISP compiler has no parent directory"))?;
    let mut candidates = vec![directory.join(FILE_NAME)];
    if directory.file_name().is_some_and(|name| name == "deps")
        && let Some(profile) = directory.parent()
    {
        candidates.push(profile.join(FILE_NAME));
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return resolve(candidate);
        }
    }
    Err(error(format!(
        "native cryptography runtime `{FILE_NAME}` was not found beside the DISP compiler; checked {}",
        candidates
            .iter()
            .map(|path| format!("`{}`", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

fn digest(path: &Path) -> Result<[u8; 32], Diagnostic> {
    let bytes = fs::read(path).map_err(|cause| {
        error(format!(
            "could not read native cryptography runtime `{}`: {cause}",
            path.display()
        ))
    })?;
    Ok(Sha256::digest(bytes).into())
}

pub fn stage_for(executable: &Path) -> Result<PathBuf, Diagnostic> {
    let source = locate()?;
    let directory = executable.parent().ok_or_else(|| {
        error(format!(
            "generated executable `{}` has no parent directory",
            executable.display()
        ))
    })?;
    fs::create_dir_all(directory).map_err(|cause| {
        error(format!(
            "could not create generated executable directory `{}`: {cause}",
            directory.display()
        ))
    })?;
    let destination = directory.join(FILE_NAME);
    if destination.is_file() && digest(&source)? == digest(&destination)? {
        return Ok(destination);
    }
    let temporary = directory.join(format!(
        ".{FILE_NAME}.stage-{}-{}",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::copy(&source, &temporary).map_err(|cause| {
        error(format!(
            "could not stage native cryptography runtime from `{}` to `{}`: {cause}",
            source.display(),
            temporary.display()
        ))
    })?;
    if digest(&source)? != digest(&temporary)? {
        let _ = fs::remove_file(&temporary);
        return Err(error(
            "staged native cryptography runtime failed its SHA-256 integrity check",
        ));
    }
    if destination.exists() {
        fs::remove_file(&destination).map_err(|cause| {
            error(format!(
                "could not replace native cryptography runtime `{}`: {cause}",
                destination.display()
            ))
        })?;
    }
    fs::rename(&temporary, &destination).map_err(|cause| {
        let _ = fs::remove_file(&temporary);
        error(format!(
            "could not commit native cryptography runtime `{}`: {cause}",
            destination.display()
        ))
    })?;
    Ok(destination)
}
