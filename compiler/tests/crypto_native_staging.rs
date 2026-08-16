use disp::backend::crypto_runtime;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn temporary_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-crypto-native-stage-{}-{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn compiler_locates_the_versioned_native_crypto_companion() {
    let runtime = crypto_runtime::locate().unwrap();
    assert!(runtime.is_absolute());
    assert!(runtime.is_file());
    assert_eq!(runtime.file_name().unwrap(), crypto_runtime::FILE_NAME);
    assert!(fs::metadata(runtime).unwrap().len() > 32 * 1024);
}

#[test]
fn staging_is_content_verified_and_idempotent() {
    let directory = temporary_directory();
    fs::create_dir_all(&directory).unwrap();
    let executable = directory.join(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });
    fs::write(&executable, b"placeholder").unwrap();
    let source = crypto_runtime::locate().unwrap();
    let staged = crypto_runtime::stage_for(&executable).unwrap();
    assert_eq!(staged.parent(), Some(directory.as_path()));
    assert_eq!(staged.file_name().unwrap(), crypto_runtime::FILE_NAME);
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(fs::read(source).unwrap())),
        <[u8; 32]>::from(Sha256::digest(fs::read(&staged).unwrap()))
    );
    assert_eq!(crypto_runtime::stage_for(&executable).unwrap(), staged);
    fs::remove_dir_all(directory).unwrap();
}
