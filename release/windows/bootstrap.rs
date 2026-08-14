use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

const COMPILER: &[u8] = include_bytes!(env!("DISP_COMPILER_EXE"));
const ZIG_ARCHIVE: &[u8] = include_bytes!(env!("DISP_ZIG_ARCHIVE"));
const INSTALL_SCRIPT: &[u8] = include_bytes!(env!("DISP_INSTALL_SCRIPT"));
const RELEASE_NOTES: &[u8] = include_bytes!(env!("DISP_RELEASE_NOTES"));

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run() -> Result<(), String> {
    let mut install_directory = None;
    let mut skip_path = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--install-dir" => {
                install_directory = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--install-dir requires a path".to_owned())?,
                );
            }
            "--skip-path" => skip_path = true,
            _ => return Err(format!("unknown installer option `{argument}`")),
        }
    }

    println!("DISP 0.1.0 Developer Preview installer");
    println!("Installing for the current Windows user...");

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("could not read the system clock: {error}"))?
        .as_nanos();
    let directory = env::temp_dir().join(format!("disp-0.1-installer-{}-{nonce}", process::id()));
    fs::create_dir(&directory)
        .map_err(|error| format!("could not create the temporary installer directory: {error}"))?;
    let temporary = TemporaryDirectory(directory);

    let compiler = temporary.0.join("disp.exe");
    let zig = temporary.0.join("zig-x86_64-windows-0.16.0.zip");
    let installer = temporary.0.join("install.ps1");
    let notes = temporary.0.join("RELEASE_NOTES_0.1.md");
    fs::write(&compiler, COMPILER)
        .map_err(|error| format!("could not unpack the DISP compiler: {error}"))?;
    fs::write(&zig, ZIG_ARCHIVE)
        .map_err(|error| format!("could not unpack the native toolchain: {error}"))?;
    fs::write(&installer, INSTALL_SCRIPT)
        .map_err(|error| format!("could not unpack the installer: {error}"))?;
    fs::write(&notes, RELEASE_NOTES)
        .map_err(|error| format!("could not unpack the release notes: {error}"))?;

    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&installer);
    if let Some(directory) = install_directory {
        command.args(["-InstallDirectory", &directory]);
    }
    if skip_path {
        command.arg("-SkipPath");
    }
    let status = command
        .status()
        .map_err(|error| format!("could not start the Windows installer: {error}"))?;
    if !status.success() {
        return Err(format!("installation failed with {status}"));
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
