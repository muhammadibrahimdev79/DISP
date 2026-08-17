use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    fs::OpenOptions,
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
    let lock_path = directory.join(format!(".{FILE_NAME}.lock"));
    let staging_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|cause| {
            error(format!(
                "could not open native cryptography runtime staging lock `{}`: {cause}",
                lock_path.display()
            ))
        })?;
    staging_lock.lock().map_err(|cause| {
        error(format!(
            "could not lock native cryptography runtime staging lock `{}`: {cause}",
            lock_path.display()
        ))
    })?;
    let destination = directory.join(FILE_NAME);
    let source_digest = digest(&source)?;
    if destination.is_file() && source_digest == digest(&destination)? {
        return Ok(destination);
    }
    let temporary = directory.join(format!(
        ".{FILE_NAME}.stage-{}-{}",
        std::process::id(),
        NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(cause) = fs::copy(&source, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error(format!(
            "could not stage native cryptography runtime from `{}` to `{}`: {cause}",
            source.display(),
            temporary.display()
        )));
    }
    let temporary_digest = match digest(&temporary) {
        Ok(value) => value,
        Err(diagnostic) => {
            let _ = fs::remove_file(&temporary);
            return Err(diagnostic);
        }
    };
    if source_digest != temporary_digest {
        let _ = fs::remove_file(&temporary);
        return Err(error(
            "staged native cryptography runtime failed its SHA-256 integrity check",
        ));
    }
    if destination.exists() {
        if destination.is_file() && source_digest == digest(&destination)? {
            let _ = fs::remove_file(&temporary);
            return Ok(destination);
        }
        fs::remove_file(&destination).map_err(|cause| {
            error(format!(
                "could not replace native cryptography runtime `{}`: {cause}; stop programs using this build directory or build into an isolated directory",
                destination.display(),
            ))
        })?;
    }
    match fs::rename(&temporary, &destination) {
        Ok(()) => Ok(destination),
        Err(cause) => {
            let destination_matches = destination.is_file()
                && digest(&destination).is_ok_and(|value| value == source_digest);
            let _ = fs::remove_file(&temporary);
            if destination_matches {
                Ok(destination)
            } else {
                Err(error(format!(
                    "could not commit native cryptography runtime `{}`: {cause}",
                    destination.display()
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        sync::{Arc, Barrier},
        time::Duration,
    };

    const CHILD_EXECUTABLE: &str = "DISP_CRYPTO_STAGE_TEST_EXECUTABLE";
    const CHILD_GATE: &str = "DISP_CRYPTO_STAGE_TEST_GATE";

    #[test]
    fn staging_process_helper() {
        let Some(executable) = env::var_os(CHILD_EXECUTABLE) else {
            return;
        };
        let gate = PathBuf::from(env::var_os(CHILD_GATE).unwrap());
        for _ in 0..10_000 {
            if gate.exists() {
                stage_for(Path::new(&executable)).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for the native cryptography staging test gate");
    }

    #[test]
    fn concurrent_processes_reuse_one_verified_runtime() {
        let directory = env::temp_dir().join(format!(
            "disp-crypto-stage-test-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("program.exe");
        let gate = directory.join("start");
        let test_binary = env::current_exe().unwrap();
        let children = (0..8)
            .map(|_| {
                Command::new(&test_binary)
                    .arg("--exact")
                    .arg("backend::crypto_runtime::tests::staging_process_helper")
                    .arg("--nocapture")
                    .env(CHILD_EXECUTABLE, &executable)
                    .env(CHILD_GATE, &gate)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        fs::write(&gate, b"start").unwrap();

        let expected = directory.join(FILE_NAME);
        for child in children {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "staging child failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(
            digest(&locate().unwrap()).unwrap(),
            digest(&expected).unwrap()
        );
        assert!(
            fs::read_dir(&directory).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".stage-")
            }),
            "native cryptography staging left temporary files behind"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_threads_reuse_one_verified_runtime() {
        let directory = env::temp_dir().join(format!(
            "disp-crypto-stage-thread-test-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let executable = Arc::new(directory.join("program.exe"));
        let barrier = Arc::new(Barrier::new(8));
        let workers = (0..8)
            .map(|_| {
                let executable = Arc::clone(&executable);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    stage_for(&executable)
                })
            })
            .collect::<Vec<_>>();

        let expected = directory.join(FILE_NAME);
        for worker in workers {
            assert_eq!(worker.join().unwrap().unwrap(), expected);
        }
        assert_eq!(
            digest(&locate().unwrap()).unwrap(),
            digest(&expected).unwrap()
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
