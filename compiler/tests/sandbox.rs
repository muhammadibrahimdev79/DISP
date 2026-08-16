use disp::{
    backend, lower_source,
    process_sandbox::{SandboxProfile, SandboxedCommand},
    run_source,
};
use std::{fs, path::PathBuf, process::Command};
#[cfg(windows)]
use std::{thread, time::Duration};

const INNER_PROBE: &str = "DISP_SANDBOX_PROBE_INNER";

#[test]
fn generated_runtime_contains_fixed_linux_hard_boundary_contract() {
    let source = "fn main() { print(1) }";
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox-tests")
        .join(format!("generated-contract-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("contract.disp");
    fs::write(&source_path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(
        &hir,
        &mir,
        &source_path,
        backend::BuildOptions {
            emit_c: true,
            ..backend::BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifact.backend_ir.unwrap()).unwrap();
    for invariant in [
        "/usr/libexec/disp-cgroup-launch",
        "DISP_LINUX_HARD_SANDBOX",
        "disp_process_helper_trusted",
        "disp_process_sandbox_exec",
        "DISP_DEFAULT_CHILD_WALL_MILLIS",
    ] {
        assert!(
            generated.contains(invariant),
            "generated runtime lost Linux hard-boundary invariant: {invariant}"
        );
    }
}

fn run_native(name: &str, source: &str) -> std::process::Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox-tests")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join(format!("{name}.disp"));
    fs::write(&source_path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let launch_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let mut blocked = None;
    for (variant, options) in [
        backend::BuildOptions::default(),
        backend::BuildOptions {
            optimized: true,
            ..backend::BuildOptions::default()
        },
    ]
    .into_iter()
    .enumerate()
    {
        let artifact = backend::build(&hir, &mir, &source_path, options).unwrap();
        for attempt in 0..10 {
            let executable = if attempt == 0 {
                artifact.executable.clone()
            } else {
                let alternate = launch_root.join(format!(
                    "disp-sandbox-probe-{}-{name}-{variant}-{attempt}.exe",
                    std::process::id()
                ));
                fs::copy(&artifact.executable, &alternate).unwrap();
                alternate
            };
            match Command::new(&executable).output() {
                Ok(output) => return output,
                Err(error) if error.raw_os_error() == Some(4551) => blocked = Some(error),
                Err(error) => panic!("sandbox probe native launch failed: {error}"),
            }
        }
    }
    panic!(
        "Windows application policy blocked every sandbox probe artifact: {}",
        blocked.unwrap()
    )
}

fn differential_probe(name: &str, source: &str, limits: &[(&str, &str)], test_name: &str) {
    if std::env::var(INNER_PROBE).as_deref() == Ok(name) {
        assert_eq!(run_source(source).unwrap(), vec!["true"]);
        let output = run_native(name, source);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .replace("\r\n", "\n"),
            "true\n"
        );
        return;
    }

    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", test_name, "--nocapture"]);
    child.env(INNER_PROBE, name);
    child.envs(limits.iter().copied());
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "sandbox probe subprocess failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(any(target_os = "linux", windows))]
fn enter_isolated_probe(name: &str, limits: &[(&str, &str)], test_name: &str) -> bool {
    if std::env::var(INNER_PROBE).as_deref() == Ok(name) {
        return true;
    }
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args(["--exact", test_name, "--nocapture"]);
    child.env(INNER_PROBE, name);
    child.envs(limits.iter().copied());
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "isolated sandbox probe failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[cfg(target_os = "linux")]
fn compile_linux_c_probe(name: &str, body: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sandbox-tests")
        .join(format!("linux-{}-{name}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("escape.c");
    let executable = root.join("escape");
    fs::write(&source, body).unwrap();
    let output = Command::new("cc")
        .args(["-std=c11", "-O2", "-o"])
        .arg(&executable)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "could not compile Linux sandbox probe: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

#[cfg(target_os = "linux")]
fn compile_linux_escape_probe(name: &str) -> PathBuf {
    compile_linux_c_probe(
        name,
        r#"#define _GNU_SOURCE
#include <errno.h>
#include <sched.h>
#include <sys/prctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
int main(void) {
    errno = 0;
    if (setpgid(0, 0) != -1 || errno != EPERM) return 10;
    errno = 0;
    if (unshare(0) != -1 || errno != EPERM) return 11;
    errno = 0;
    if (setsid() != -1 || errno != EPERM) return 12;
    if (prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 1) return 13;
    pid_t child = fork();
    if (child < 0) return errno == EAGAIN ? 0 : 14;
    if (child == 0) _exit(0);
    int status = 0;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 15;
    return 0;
}
"#,
    )
}

#[cfg(target_os = "linux")]
#[test]
fn shared_launcher_denies_linux_group_and_namespace_escape() {
    let probe = compile_linux_escape_probe("shared");
    let command = SandboxedCommand::new(&probe);
    let output = command.output(SandboxProfile::Toolchain).unwrap();
    assert!(
        output.status.success(),
        "Linux escape probe exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn shared_launcher_closes_unrelated_inheritable_linux_fds() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let probe = compile_linux_c_probe(
        "fd",
        r#"#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    if (argc != 2) return 10;
    int fd = atoi(argv[1]);
    errno = 0;
    return fcntl(fd, F_GETFD) == -1 && errno == EBADF ? 0 : 11;
}
"#,
    );
    let mut raw = [0; 2];
    // SAFETY: `raw` has space for both descriptors returned by pipe.
    assert_eq!(unsafe { libc::pipe(raw.as_mut_ptr()) }, 0);
    // SAFETY: ownership of both newly-created descriptors transfers here.
    let read = unsafe { OwnedFd::from_raw_fd(raw[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(raw[1]) };
    // Keep the sentinel well above descriptors normally allocated by the
    // dynamic loader so a closed number is not accidentally reused.
    // SAFETY: `read` owns a valid descriptor and F_DUPFD returns a new one.
    let sentinel = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_DUPFD, 512) };
    assert!(
        sentinel >= 512,
        "could not allocate a high descriptor sentinel"
    );
    // SAFETY: ownership of the duplicate transfers here.
    let sentinel = unsafe { OwnedFd::from_raw_fd(sentinel) };
    drop(read);
    // Deliberately make the sentinel inheritable; the launcher must override it.
    // SAFETY: `sentinel` owns a valid descriptor and F_SETFD accepts zero flags.
    assert_eq!(
        unsafe { libc::fcntl(sentinel.as_raw_fd(), libc::F_SETFD, 0) },
        0
    );

    let mut command = SandboxedCommand::new(&probe);
    command.arg(sentinel.as_raw_fd().to_string());
    let output = command.output(SandboxProfile::Toolchain).unwrap();
    drop(write);
    drop(sentinel);
    assert!(
        output.status.success(),
        "Linux descriptor probe exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_engines_inherit_linux_escape_filter() {
    let probe = compile_linux_escape_probe("runtime");
    let source = format!(
        r#"
fn contained() -> Result<bool, IoError> {{
    var arguments: List<String> = List.new()
    output = Process.run(Path("{}"), arguments)?
    return Ok(output.success())
}}
fn main() {{ print(match contained() {{ Ok(value) => value, Err(error) => false }}) }}
"#,
        probe.display()
    );
    differential_probe(
        "linux-escape",
        &source,
        &[],
        "runtime_engines_inherit_linux_escape_filter",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn child_address_space_is_kernel_bounded_in_both_linux_engines() {
    let probe = compile_linux_c_probe(
        "memory",
        r#"#include <stddef.h>
#include <stdlib.h>
int main(void) {
    const size_t bytes = 256u * 1024u * 1024u;
    volatile unsigned char *memory = malloc(bytes);
    if (!memory) return 42;
    for (size_t offset = 0; offset < bytes; offset += 4096) memory[offset] = 1;
    free((void *)memory);
    return 0;
}
"#,
    );
    let source = format!(
        r#"
fn contained() -> Result<bool, IoError> {{
    var arguments: List<String> = List.new()
    output = Process.run(Path("{}"), arguments)?
    return Ok(!output.success())
}}
fn main() {{ print(match contained() {{ Ok(value) => value, Err(error) => false }}) }}
"#,
        probe.display()
    );
    differential_probe(
        "linux-memory",
        &source,
        &[("DISP_CHILD_MAX_MEMORY_BYTES", "67108864")],
        "child_address_space_is_kernel_bounded_in_both_linux_engines",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn child_fork_is_bounded_by_linux_nproc_defense_in_depth() {
    let probe = compile_linux_c_probe(
        "nproc",
        r#"#include <errno.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
int main(void) {
    pid_t child = fork();
    if (child < 0) return errno == EAGAIN ? 42 : 43;
    if (child == 0) _exit(0);
    int status = 0;
    return waitpid(child, &status, 0) == child ? 0 : 44;
}
"#,
    );
    let source = format!(
        r#"
fn contained() -> Result<bool, IoError> {{
    var arguments: List<String> = List.new()
    output = Process.run(Path("{}"), arguments)?
    return Ok(!output.success())
}}
fn main() {{ print(match contained() {{ Ok(value) => value, Err(error) => false }}) }}
"#,
        probe.display()
    );
    differential_probe(
        "linux-nproc",
        &source,
        &[("DISP_CHILD_MAX_PROCESSES", "1")],
        "child_fork_is_bounded_by_linux_nproc_defense_in_depth",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_hard_profile_configuration_fails_closed() {
    if !enter_isolated_probe(
        "linux-hard-invalid",
        &[("DISP_LINUX_HARD_SANDBOX", "unknown")],
        "linux_hard_profile_configuration_fails_closed",
    ) {
        return;
    }
    let command = SandboxedCommand::new("/bin/true");
    let error = command
        .output(SandboxProfile::Runtime)
        .expect_err("unknown hard-sandbox modes must fail before creation");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.to_string().contains("auto, required, or off"),
        "{error}"
    );
}

#[test]
fn child_cpu_time_is_kernel_bounded_in_both_engines() {
    #[cfg(windows)]
    let source = r#"
fn contained() -> Result<bool, IoError> {
    output = Process.run(
        Path("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"),
        List.of("-NoProfile", "-NonInteractive", "-Command", "while ($true) {}")
    )?
    return Ok(!output.success())
}
fn main() { print(match contained() { Ok(value) => value, Err(error) => false }) }
"#;
    #[cfg(not(windows))]
    let source = r#"
fn contained() -> Result<bool, IoError> {
    output = Process.run(Path("/bin/sh"), List.of("-c", "while :; do :; done"))?
    return Ok(!output.success())
}
fn main() { print(match contained() { Ok(value) => value, Err(error) => false }) }
"#;
    differential_probe(
        "cpu",
        source,
        &[("DISP_CHILD_MAX_CPU_MILLIS", "100")],
        "child_cpu_time_is_kernel_bounded_in_both_engines",
    );
}

#[cfg(windows)]
#[test]
fn child_committed_memory_is_kernel_bounded_in_both_engines() {
    let source = r#"
fn contained() -> Result<bool, IoError> {
    output = Process.run(
        Path("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"),
        List.of("-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; $bytes=New-Object byte[] 536870912; $bytes.Length")
    )?
    return Ok(!output.success())
}
fn main() { print(match contained() { Ok(value) => value, Err(error) => false }) }
"#;
    differential_probe(
        "memory",
        source,
        &[("DISP_CHILD_MAX_MEMORY_BYTES", "268435456")],
        "child_committed_memory_is_kernel_bounded_in_both_engines",
    );
}

#[cfg(windows)]
#[test]
fn child_tree_process_count_is_kernel_bounded_in_both_engines() {
    let source = r#"
fn contained() -> Result<bool, IoError> {
    output = Process.run(
        Path("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"),
        List.of("-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; Start-Process -Wait -FilePath 'C:/Windows/System32/cmd.exe' -ArgumentList '/c','exit 0'")
    )?
    return Ok(!output.success())
}
fn main() { print(match contained() { Ok(value) => value, Err(error) => false }) }
"#;
    differential_probe(
        "processes",
        source,
        &[("DISP_CHILD_MAX_PROCESSES", "1")],
        "child_tree_process_count_is_kernel_bounded_in_both_engines",
    );
}

#[cfg(windows)]
#[test]
fn shared_launcher_bounds_output_and_hides_internal_state() {
    if !enter_isolated_probe(
        "tool-output",
        &[("DISP_TOOL_MAX_OUTPUT_BYTES", "1024")],
        "shared_launcher_bounds_output_and_hides_internal_state",
    ) {
        return;
    }
    let powershell = "C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe";
    let mut gate = SandboxedCommand::new(powershell);
    gate.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "[Console]::Out.Write([Environment]::GetEnvironmentVariable('DISP_INTERNAL_SANDBOX_GATE') -eq $null)",
    ]);
    let gate = gate.output(SandboxProfile::Toolchain).unwrap();
    assert!(gate.status.success());
    assert_eq!(String::from_utf8(gate.stdout).unwrap(), "True");

    let mut excessive = SandboxedCommand::new(powershell);
    excessive.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "[Console]::Out.Write('x' * 4096)",
    ]);
    let error = excessive
        .output(SandboxProfile::Toolchain)
        .expect_err("tool output beyond its profile must terminate the tree");
    assert!(error.to_string().contains("output exceeds"), "{error}");
}

#[cfg(windows)]
#[test]
fn shared_launcher_rejects_invalid_policy_and_program_injection() {
    if !enter_isolated_probe(
        "tool-invalid",
        &[("DISP_TOOL_MAX_MEMORY_BYTES", "0")],
        "shared_launcher_rejects_invalid_policy_and_program_injection",
    ) {
        return;
    }
    let command = SandboxedCommand::new("C:/Windows/System32/where.exe");
    let error = command
        .output(SandboxProfile::Toolchain)
        .expect_err("zero tool memory policy must fail before launch");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("greater than zero"), "{error}");

    let injected = SandboxedCommand::new("where.exe cmd.exe");
    let error = injected
        .output(SandboxProfile::Runtime)
        .expect_err("program text must never be interpreted as a shell command");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[cfg(windows)]
#[test]
fn shared_launcher_wall_deadline_kills_grandchildren() {
    if !enter_isolated_probe(
        "tool-wall",
        &[("DISP_TOOL_MAX_WALL_MILLIS", "100")],
        "shared_launcher_wall_deadline_kills_grandchildren",
    ) {
        return;
    }
    let marker = std::env::temp_dir().join(format!(
        "disp-tool-tree-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_file(&marker);
    let script = format!(
        "Start-Process -WindowStyle Hidden powershell.exe -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Milliseconds 500; [IO.File]::WriteAllText(''{}'',''bad'')'; Start-Sleep -Seconds 5",
        marker.to_string_lossy().replace('\\', "/")
    );
    let mut command =
        SandboxedCommand::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let error = command
        .output(SandboxProfile::Toolchain)
        .expect_err("tool tree must exceed its wall deadline");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    thread::sleep(Duration::from_millis(800));
    assert!(!marker.exists(), "grandchild escaped the toolchain job");
}

#[cfg(windows)]
#[test]
fn shared_launcher_denies_windows_breakaway() {
    let source = r#"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class BreakawayProbe {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO { public int cb; public string reserved; public string desktop; public string title; public int x; public int y; public int xSize; public int ySize; public int xChars; public int yChars; public int fill; public int flags; public short show; public short reserved2; public IntPtr reservedPtr; public IntPtr input; public IntPtr output; public IntPtr error; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr process; public IntPtr thread; public int processId; public int threadId; }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcess(string application, StringBuilder command, IntPtr processAttributes, IntPtr threadAttributes, bool inherit, uint flags, IntPtr environment, string directory, ref STARTUPINFO startup, out PROCESS_INFORMATION information);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    public static int Run() {
        var startup = new STARTUPINFO(); startup.cb = Marshal.SizeOf(startup);
        PROCESS_INFORMATION information;
        bool created = CreateProcess(null, new StringBuilder("C:\\Windows\\System32\\cmd.exe /c exit 0"), IntPtr.Zero, IntPtr.Zero, false, 0x01000000, IntPtr.Zero, null, ref startup, out information);
        if (!created) return Marshal.GetLastWin32Error() == 5 ? 42 : 43;
        TerminateProcess(information.process, 125); CloseHandle(information.thread); CloseHandle(information.process); return 0;
    }
}
"#;
    let script = format!(
        "$source=@'\n{source}\n'@; Add-Type -TypeDefinition $source; [Console]::Out.Write([BreakawayProbe]::Run())"
    );
    let mut command =
        SandboxedCommand::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let output = command.output(SandboxProfile::Toolchain).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "42");
}

#[cfg(windows)]
#[test]
fn shared_launcher_inherits_only_declared_standard_handles() {
    use std::{
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use windows_sys::Win32::{Security::SECURITY_ATTRIBUTES, System::Threading::CreateEventW};

    let create_event = |inheritable| {
        let security = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap(),
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: i32::from(inheritable),
        };
        // SAFETY: attributes are valid and an unnamed event needs no name buffer.
        let raw = unsafe { CreateEventW(&raw const security, 1, 0, ptr::null()) };
        assert!(!raw.is_null());
        // SAFETY: ownership of the new event transfers to `OwnedHandle`.
        unsafe { OwnedHandle::from_raw_handle(raw.cast()) }
    };
    // Push the sentinel beyond handles normally allocated during PowerShell
    // startup, making accidental same-value reuse in the child implausible.
    let padding = (0..512).map(|_| create_event(false)).collect::<Vec<_>>();
    let sentinel = create_event(true);
    let handle = sentinel.as_raw_handle() as usize;
    let source = r#"
using System;
using System.Runtime.InteropServices;
public static class HandleProbe {
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetHandleInformation(IntPtr handle, out uint flags);
    public static bool IsValid(long value) { uint flags; return GetHandleInformation(new IntPtr(value), out flags); }
}
"#;
    let script = format!(
        "$source=@'\n{source}\n'@; Add-Type -TypeDefinition $source; [Console]::Out.Write([HandleProbe]::IsValid({handle}))"
    );
    let mut command =
        SandboxedCommand::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let output = command.output(SandboxProfile::Toolchain).unwrap();
    drop(padding);
    drop(sentinel);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "False");
}
