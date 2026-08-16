//! Shared OS process-tree boundary used by compiler tools and `disp run`.
//!
//! This module is public only so the `disp` binary can share the library's
//! launcher. It is not a stable language-level API.

use crate::limits;
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{ExitStatus, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::process::{Child, Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxProfile {
    Runtime,
    Toolchain,
    Component,
}

#[derive(Clone, Copy)]
struct SandboxLimits {
    memory_bytes: usize,
    cpu_millis: usize,
    processes: usize,
    wall_millis: usize,
    output_bytes: usize,
}

impl SandboxLimits {
    fn load(profile: SandboxProfile) -> io::Result<Self> {
        match profile {
            SandboxProfile::Runtime => Ok(Self {
                memory_bytes: positive_limit(
                    "DISP_CHILD_MAX_MEMORY_BYTES",
                    limits::DEFAULT_CHILD_MEMORY_BYTES,
                )?,
                cpu_millis: positive_limit(
                    "DISP_CHILD_MAX_CPU_MILLIS",
                    limits::DEFAULT_CHILD_CPU_MILLIS,
                )?,
                processes: positive_limit(
                    "DISP_CHILD_MAX_PROCESSES",
                    limits::DEFAULT_CHILD_PROCESSES,
                )?,
                wall_millis: positive_limit(
                    "DISP_CHILD_MAX_WALL_MILLIS",
                    limits::DEFAULT_CHILD_WALL_MILLIS,
                )?,
                output_bytes: limits::PROCESS_STREAM_BYTES,
            }),
            SandboxProfile::Toolchain => Ok(Self {
                memory_bytes: positive_limit(
                    "DISP_TOOL_MAX_MEMORY_BYTES",
                    limits::DEFAULT_TOOL_MEMORY_BYTES,
                )?,
                cpu_millis: positive_limit(
                    "DISP_TOOL_MAX_CPU_MILLIS",
                    limits::DEFAULT_TOOL_CPU_MILLIS,
                )?,
                processes: positive_limit(
                    "DISP_TOOL_MAX_PROCESSES",
                    limits::DEFAULT_TOOL_PROCESSES,
                )?,
                wall_millis: positive_limit(
                    "DISP_TOOL_MAX_WALL_MILLIS",
                    limits::DEFAULT_TOOL_WALL_MILLIS,
                )?,
                output_bytes: positive_limit(
                    "DISP_TOOL_MAX_OUTPUT_BYTES",
                    limits::DEFAULT_TOOL_OUTPUT_BYTES,
                )?,
            }),
            SandboxProfile::Component => Ok(Self {
                memory_bytes: positive_limit(
                    "DISP_COMPONENT_MAX_MEMORY_BYTES",
                    limits::DEFAULT_COMPONENT_MEMORY_BYTES,
                )?,
                cpu_millis: positive_limit(
                    "DISP_COMPONENT_MAX_CPU_MILLIS",
                    limits::DEFAULT_COMPONENT_CPU_MILLIS,
                )?,
                processes: positive_limit(
                    "DISP_COMPONENT_MAX_PROCESSES",
                    limits::DEFAULT_COMPONENT_PROCESSES,
                )?,
                wall_millis: positive_limit(
                    "DISP_COMPONENT_MAX_WALL_MILLIS",
                    limits::DEFAULT_COMPONENT_WALL_MILLIS,
                )?,
                output_bytes: positive_limit(
                    "DISP_COMPONENT_MAX_OUTPUT_BYTES",
                    limits::DEFAULT_COMPONENT_OUTPUT_BYTES,
                )?,
            }),
        }
    }
}

fn positive_limit(name: &str, fallback: usize) -> io::Result<usize> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(fallback);
    };
    let text = raw.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a positive decimal integer"),
        )
    })?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a positive decimal integer"),
        ));
    }
    let value = text.parse::<usize>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} exceeds the platform range"),
        )
    })?;
    if value == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be greater than zero"),
        ));
    }
    Ok(value)
}

pub struct SandboxedCommand {
    program: OsString,
    arguments: Vec<OsString>,
    directory: Option<PathBuf>,
    environment: Vec<(OsString, Option<OsString>)>,
    clear_environment: bool,
}

impl SandboxedCommand {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            arguments: Vec::new(),
            directory: None,
            environment: Vec::new(),
            clear_environment: false,
        }
    }

    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments
            .extend(arguments.into_iter().map(|value| value.as_ref().to_owned()));
        self
    }

    pub fn current_dir(&mut self, directory: impl AsRef<Path>) -> &mut Self {
        self.directory = Some(directory.as_ref().to_owned());
        self
    }

    pub fn env(&mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.environment
            .push((name.as_ref().to_owned(), Some(value.as_ref().to_owned())));
        self
    }

    pub fn env_remove(&mut self, name: impl AsRef<OsStr>) -> &mut Self {
        self.environment.push((name.as_ref().to_owned(), None));
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.clear_environment = true;
        self
    }

    pub fn output(&self, profile: SandboxProfile) -> io::Result<Output> {
        let limits = SandboxLimits::load(profile)?;
        let mut child = self.spawn(profile, IoMode::Piped, limits)?;
        child.wait_with_output(limits)
    }

    pub fn output_with_input(&self, profile: SandboxProfile, input: &[u8]) -> io::Result<Output> {
        let limits = SandboxLimits::load(profile)?;
        let mut child = self.spawn(profile, IoMode::Streaming, limits)?;
        let mut stdin = child
            .take_stdin()
            .expect("streaming sandbox stdin is piped");
        let input = input.to_vec();
        let writer = thread::spawn(move || {
            stdin.write_all(&input)?;
            stdin.flush()
        });
        let output = child.wait_with_output(limits);
        let written = writer
            .join()
            .map_err(|_| io::Error::other("sandbox stdin writer panicked"))?;
        match output {
            Ok(output) => {
                written?;
                Ok(output)
            }
            Err(error) => Err(error),
        }
    }

    pub fn status(&self, profile: SandboxProfile) -> io::Result<ExitStatus> {
        let limits = SandboxLimits::load(profile)?;
        let mut child = self.spawn(profile, IoMode::Inherited, limits)?;
        child.wait_status(limits)
    }

    pub fn spawn_streaming(&self, profile: SandboxProfile) -> io::Result<SandboxedProcess> {
        let limits = SandboxLimits::load(profile)?;
        self.spawn(profile, IoMode::Streaming, limits)
    }

    fn spawn(
        &self,
        profile: SandboxProfile,
        io_mode: IoMode,
        limits: SandboxLimits,
    ) -> io::Result<SandboxedProcess> {
        validate_command(self)?;
        #[cfg(windows)]
        let _ = profile;
        let target = resolve_program(&self.program)?;
        let directory = self
            .directory
            .as_ref()
            .map(|directory| canonical_directory(directory))
            .transpose()?;
        #[cfg(windows)]
        {
            spawn_windows(
                self,
                &target,
                directory.as_deref(),
                io_mode,
                limits,
                profile,
            )
        }
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            let (launch_target, launch_arguments, hard_boundary) =
                linux_launch_command(&target, &self.arguments, limits, profile)?;
            #[cfg(not(target_os = "linux"))]
            let (launch_target, launch_arguments, hard_boundary) =
                (target.clone(), self.arguments.clone(), false);
            let mut command = Command::new(&launch_target);
            command.args(&launch_arguments);
            if let Some(directory) = directory {
                command.current_dir(directory);
            }
            if self.clear_environment {
                command.env_clear();
            }
            for (name, value) in &self.environment {
                match value {
                    Some(value) => {
                        command.env(name, value);
                    }
                    None => {
                        command.env_remove(name);
                    }
                }
            }
            match io_mode {
                IoMode::Piped => {
                    command
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                }
                IoMode::Streaming => {
                    command
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());
                }
                IoMode::Inherited => {
                    command
                        .stdin(Stdio::inherit())
                        .stdout(Stdio::inherit())
                        .stderr(Stdio::inherit());
                }
            }
            configure_unix(&mut command, limits, hard_boundary, profile)?;
            let child = command.spawn()?;
            let guard = SandboxGuard::Unix {
                process_group: child.id(),
            };
            Ok(SandboxedProcess {
                child: PlatformChild::Standard(child),
                guard,
            })
        }
    }
}

#[derive(Clone, Copy)]
enum IoMode {
    Piped,
    Streaming,
    Inherited,
}

fn validate_command(command: &SandboxedCommand) -> io::Result<()> {
    if command.program.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox program path cannot be empty",
        ));
    }
    if command.arguments.len() > limits::PROCESS_ARGUMENTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox argument count exceeds 4096",
        ));
    }
    let bytes = command.arguments.iter().try_fold(0usize, |total, value| {
        total.checked_add(value.as_encoded_bytes().len())
    });
    if bytes.is_none_or(|bytes| bytes > limits::PROCESS_ARGUMENT_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox argument bytes exceed 1 MiB",
        ));
    }
    Ok(())
}

fn canonical_directory(directory: &Path) -> io::Result<PathBuf> {
    let directory = canonical_path(directory)?;
    if !directory.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox working directory is not a directory",
        ));
    }
    Ok(directory)
}

fn resolve_program(program: &OsStr) -> io::Result<PathBuf> {
    let requested = Path::new(program);
    if requested.is_absolute() || requested.components().count() > 1 {
        return canonical_executable(requested);
    }
    let path = std::env::var_os("PATH").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "PATH is unavailable while resolving `{}`",
                requested.display()
            ),
        )
    })?;
    #[cfg(windows)]
    let extensions = executable_extensions(requested);
    #[cfg(not(windows))]
    let extensions = vec![OsString::new()];
    for directory in std::env::split_paths(&path) {
        for extension in &extensions {
            let mut candidate = directory.join(requested);
            if !extension.is_empty() {
                candidate.set_extension(extension.to_string_lossy().trim_start_matches('.'));
            }
            if let Ok(candidate) = canonical_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "sandbox executable `{}` was not found on PATH",
            requested.display()
        ),
    ))
}

#[cfg(windows)]
fn executable_extensions(program: &Path) -> Vec<OsString> {
    if program.extension().is_some() {
        return vec![OsString::new()];
    }
    std::env::var_os("PATHEXT")
        .and_then(|value| value.to_str().map(str::to_owned))
        .map(|value| {
            value
                .split(';')
                .filter(|value| !value.is_empty())
                .map(OsString::from)
                .collect()
        })
        .filter(|values: &Vec<_>| !values.is_empty())
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()])
}

fn canonical_executable(path: &Path) -> io::Result<PathBuf> {
    let path = canonical_path(path)?;
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("sandbox executable `{}` is not a file", path.display()),
        ));
    }
    Ok(path)
}

fn canonical_path(path: &Path) -> io::Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        const VERBATIM: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
        const VERBATIM_UNC: &[u16] = &[
            b'\\' as u16,
            b'\\' as u16,
            b'?' as u16,
            b'\\' as u16,
            b'U' as u16,
            b'N' as u16,
            b'C' as u16,
            b'\\' as u16,
        ];
        if wide.starts_with(VERBATIM_UNC) {
            let mut normalized = vec![b'\\' as u16, b'\\' as u16];
            normalized.extend_from_slice(&wide[VERBATIM_UNC.len()..]);
            return Ok(PathBuf::from(OsString::from_wide(&normalized)));
        }
        if wide.starts_with(VERBATIM) {
            return Ok(PathBuf::from(OsString::from_wide(&wide[VERBATIM.len()..])));
        }
    }
    Ok(path)
}

#[cfg(unix)]
fn configure_unix(
    command: &mut Command,
    limits: SandboxLimits,
    hard_boundary: bool,
    profile: SandboxProfile,
) -> io::Result<()> {
    #[cfg(not(target_os = "linux"))]
    let _ = (hard_boundary, profile);
    use std::os::unix::process::CommandExt;
    let memory = libc::rlim_t::try_from(limits.memory_bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox memory limit is too large",
        )
    })?;
    let cpu = libc::rlim_t::try_from(limits.cpu_millis.div_ceil(1_000)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox CPU limit is too large",
        )
    })?;
    let processes = libc::rlim_t::try_from(limits.processes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox process limit is too large",
        )
    })?;
    #[cfg(target_os = "linux")]
    let escape_filter = linux_escape_filter(profile == SandboxProfile::Component);
    #[cfg(target_os = "linux")]
    let maximum_fd = linux_maximum_fd()?;
    // SAFETY: the callback uses async-signal-safe syscalls only.
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            if hard_boundary {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                mark_linux_fds_close_on_exec(maximum_fd)?;
                return Ok(());
            }
            let memory = libc::rlimit {
                rlim_cur: memory,
                rlim_max: memory,
            };
            let cpu = libc::rlimit {
                rlim_cur: cpu,
                rlim_max: cpu,
            };
            let processes = libc::rlimit {
                rlim_cur: processes,
                rlim_max: processes,
            };
            if libc::setpgid(0, 0) != 0
                || libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                || libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0
                || libc::setrlimit(libc::RLIMIT_NPROC, &processes) != 0
            {
                return Err(io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                mark_linux_fds_close_on_exec(maximum_fd)?;
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                install_linux_escape_filter(&escape_filter)?;
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_launch_command(
    target: &Path,
    arguments: &[OsString],
    limits: SandboxLimits,
    profile: SandboxProfile,
) -> io::Result<(PathBuf, Vec<OsString>, bool)> {
    const HELPER: &str = "/usr/libexec/disp-cgroup-launch";
    let configured = std::env::var_os("DISP_LINUX_HARD_SANDBOX");
    let mode = configured
        .as_deref()
        .unwrap_or_else(|| OsStr::new("auto"))
        .to_str()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "DISP_LINUX_HARD_SANDBOX must be valid Unicode",
            )
        })?;
    if !matches!(mode, "auto" | "required" | "off") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "DISP_LINUX_HARD_SANDBOX must be auto, required, or off",
        ));
    }
    if mode == "off" {
        return Ok((target.to_owned(), arguments.to_vec(), false));
    }
    let helper = match canonical_path(Path::new(HELPER)) {
        Ok(helper) => helper,
        Err(error) if mode == "auto" && error.kind() == io::ErrorKind::NotFound => {
            return Ok((target.to_owned(), arguments.to_vec(), false));
        }
        Err(error) if mode == "auto" && error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok((target.to_owned(), arguments.to_vec(), false));
        }
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("required Linux cgroup helper is unavailable: {error}"),
            ));
        }
    };
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(&helper)?;
    let trusted = metadata.is_file()
        && metadata.uid() == 0
        && metadata.gid() == 0
        && metadata.mode() & 0o6_000 == 0o6_000
        && metadata.mode() & 0o022 == 0;
    if !trusted {
        if mode == "auto" {
            return Ok((target.to_owned(), arguments.to_vec(), false));
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "required Linux cgroup helper must be root-owned, setuid/setgid, and not group/other writable",
        ));
    }
    let mut helper_arguments = vec![
        limits.memory_bytes.to_string().into(),
        limits.cpu_millis.to_string().into(),
        limits.processes.to_string().into(),
        limits.wall_millis.to_string().into(),
    ];
    if profile == SandboxProfile::Component {
        helper_arguments.push("--component-networkless".into());
    }
    helper_arguments.push(target.as_os_str().to_owned());
    helper_arguments.extend_from_slice(arguments);
    Ok((helper, helper_arguments, true))
}

#[cfg(target_os = "linux")]
fn linux_maximum_fd() -> io::Result<libc::c_int> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a writable out-parameter of the documented type.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let maximum = limit.rlim_cur.min(libc::c_int::MAX as libc::rlim_t);
    libc::c_int::try_from(maximum).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox file-descriptor limit exceeds the platform range",
        )
    })
}

#[cfg(target_os = "linux")]
fn mark_linux_fds_close_on_exec(maximum_fd: libc::c_int) -> io::Result<()> {
    const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
    // SAFETY: the numeric range is valid and the flag only changes descriptor
    // inheritance; descriptors remain usable by the pre-exec error path.
    if unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3 as libc::c_uint,
            libc::c_uint::MAX,
            CLOSE_RANGE_CLOEXEC,
        )
    } == 0
    {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        return Err(error);
    }
    for fd in 3..maximum_fd {
        // SAFETY: `fcntl` accepts every integer descriptor and reports EBADF
        // for unused slots; setting FD_CLOEXEC does not close a live handle.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EBADF) {
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_escape_filter(deny_network: bool) -> Vec<libc::sock_filter> {
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_ALU_AND_K: u16 = 0x54;
    const BPF_RET_K: u16 = 0x06;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    const SECCOMP_DATA_NR: u32 = 0;
    const SECCOMP_DATA_ARCH: u32 = 4;
    const NAMESPACE_FLAGS: u32 = libc::CLONE_NEWCGROUP as u32
        | libc::CLONE_NEWIPC as u32
        | libc::CLONE_NEWNET as u32
        | libc::CLONE_NEWNS as u32
        | libc::CLONE_NEWPID as u32
        | libc::CLONE_NEWTIME as u32
        | libc::CLONE_NEWUSER as u32
        | libc::CLONE_NEWUTS as u32;

    let statement = |code, k| libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    };
    let jump = |k, jt, jf| libc::sock_filter {
        code: BPF_JMP_JEQ_K,
        jt,
        jf,
        k,
    };
    let denied = SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let unsupported = SECCOMP_RET_ERRNO | libc::ENOSYS as u32;
    let mut filter = vec![
        statement(BPF_LD_W_ABS, SECCOMP_DATA_ARCH),
        jump(linux_audit_arch(), 1, 0),
        statement(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        statement(BPF_LD_W_ABS, SECCOMP_DATA_NR),
    ];
    let mut denied_syscalls = vec![
        libc::SYS_setpgid,
        libc::SYS_setsid,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_getfd,
    ];
    if deny_network {
        denied_syscalls.extend([
            libc::SYS_socket,
            libc::SYS_socketpair,
            libc::SYS_connect,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
            libc::SYS_sendto,
            libc::SYS_recvfrom,
            libc::SYS_sendmsg,
            libc::SYS_recvmsg,
            libc::SYS_sendmmsg,
            libc::SYS_recvmmsg,
            libc::SYS_shutdown,
            libc::SYS_getsockname,
            libc::SYS_getpeername,
            libc::SYS_setsockopt,
            libc::SYS_getsockopt,
            libc::SYS_io_uring_setup,
        ]);
        #[cfg(target_arch = "x86")]
        denied_syscalls.push(libc::SYS_socketcall);
    }
    for syscall in denied_syscalls {
        filter.push(jump(syscall as u32, 0, 1));
        filter.push(statement(BPF_RET_K, denied));
    }
    // Returning ENOSYS preserves libc's clone fallback while preventing
    // clone3 from hiding namespace and cgroup-placement flags in a pointer.
    filter.push(jump(libc::SYS_clone3 as u32, 0, 1));
    filter.push(statement(BPF_RET_K, unsupported));
    // Ordinary clone/fork/thread creation remains available, but namespace
    // creation cannot be used to acquire a new authority boundary.
    filter.push(jump(libc::SYS_clone as u32, 0, 4));
    filter.push(statement(BPF_LD_W_ABS, linux_clone_flags_offset()));
    filter.push(statement(BPF_ALU_AND_K, NAMESPACE_FLAGS));
    filter.push(jump(0, 1, 0));
    filter.push(statement(BPF_RET_K, denied));
    filter.push(statement(BPF_RET_K, SECCOMP_RET_ALLOW));
    filter
}

#[cfg(target_os = "linux")]
fn install_linux_escape_filter(filter: &[libc::sock_filter]) -> io::Result<()> {
    let len = u16::try_from(filter.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seccomp filter is too large"))?;
    let program = libc::sock_fprog {
        len,
        filter: filter.as_ptr().cast_mut(),
    };
    const SECCOMP_MODE_FILTER: libc::c_ulong = 2;
    // SAFETY: `program` and its instruction slice remain live for the syscall;
    // no allocation or non-async-signal-safe library operation occurs here.
    if unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            &raw const program,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const fn linux_audit_arch() -> u32 {
    0xc000_003e
}

#[cfg(all(target_os = "linux", target_arch = "x86"))]
const fn linux_audit_arch() -> u32 {
    0x4000_0003
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const fn linux_audit_arch() -> u32 {
    0xc000_00b7
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
const fn linux_audit_arch() -> u32 {
    0x4000_0028
}

#[cfg(all(target_os = "linux", target_arch = "riscv64"))]
const fn linux_audit_arch() -> u32 {
    0xc000_00f3
}

#[cfg(all(target_os = "linux", target_arch = "s390x"))]
const fn linux_audit_arch() -> u32 {
    0x8000_0016
}

#[cfg(all(target_os = "linux", target_arch = "powerpc64", target_endian = "big"))]
const fn linux_audit_arch() -> u32 {
    0x8000_0015
}

#[cfg(all(
    target_os = "linux",
    target_arch = "powerpc64",
    target_endian = "little"
))]
const fn linux_audit_arch() -> u32 {
    0xc000_0015
}

#[cfg(all(target_os = "linux", target_arch = "s390x"))]
const fn linux_clone_flags_offset() -> u32 {
    // Linux s390 passes clone's stack pointer before its flags.
    24
}

#[cfg(all(target_os = "linux", not(target_arch = "s390x")))]
const fn linux_clone_flags_offset() -> u32 {
    16
}

enum SandboxGuard {
    #[cfg(unix)]
    Unix { process_group: u32 },
    #[cfg(windows)]
    Windows {
        job: std::os::windows::io::OwnedHandle,
    },
}

impl SandboxGuard {
    fn terminate(&self) -> io::Result<()> {
        #[cfg(unix)]
        let Self::Unix { process_group } = self;
        #[cfg(unix)]
        {
            let process_group = i32::try_from(*process_group).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "sandbox process id is too large",
                )
            })?;
            // SAFETY: the negative PID targets only the dedicated child group.
            if unsafe { libc::kill(-process_group, libc::SIGKILL) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        #[cfg(windows)]
        let Self::Windows { job, .. } = self;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            // SAFETY: the guard owns a live Job Object handle.
            if unsafe { TerminateJobObject(job.as_raw_handle().cast(), 124) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[cfg(windows)]
fn create_windows_job(
    limits: SandboxLimits,
    profile: SandboxProfile,
) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
        JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
        JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
        JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
        JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicUIRestrictions,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    let processes = u32::try_from(limits.processes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox process limit is too large",
        )
    })?;
    let cpu_ticks = i64::try_from(limits.cpu_millis)
        .ok()
        .and_then(|value| value.checked_mul(10_000))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "sandbox CPU limit is too large",
            )
        })?;
    // SAFETY: null attributes and name create an unnamed, non-inheritable job.
    let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the new handle transfers to `OwnedHandle`.
    let job = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw.cast()) };
    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_JOB_TIME
        | JOB_OBJECT_LIMIT_JOB_MEMORY;
    information.BasicLimitInformation.ActiveProcessLimit = processes;
    information.BasicLimitInformation.PerJobUserTimeLimit = cpu_ticks;
    information.JobMemoryLimit = limits.memory_bytes;
    // SAFETY: the pointer and byte count describe the exact information structure.
    if unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            (&raw const information).cast(),
            u32::try_from(std::mem::size_of_val(&information)).expect("job structure size"),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if profile == SandboxProfile::Component {
        let restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_EXITWINDOWS,
        };
        // SAFETY: the pointer and byte count describe the exact UI structure.
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectBasicUIRestrictions,
                (&raw const restrictions).cast(),
                u32::try_from(std::mem::size_of_val(&restrictions)).expect("job UI structure size"),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(job)
}

#[cfg(windows)]
struct WindowsSid(windows_sys::Win32::Security::PSID);

#[cfg(windows)]
impl Drop for WindowsSid {
    fn drop(&mut self) {
        // SAFETY: AppContainer profile APIs allocate returned SIDs with the
        // allocator paired with FreeSid, and this wrapper owns exactly one SID.
        unsafe { windows_sys::Win32::Security::FreeSid(self.0) };
    }
}

#[cfg(windows)]
struct WindowsMutexGuard(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl Drop for WindowsMutexGuard {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: this guard is constructed only after the caller owns the mutex.
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0.as_raw_handle().cast())
        };
    }
}

#[cfg(windows)]
fn lock_windows_appcontainer_profile(identity: &str) -> io::Result<WindowsMutexGuard> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{CreateMutexW, WaitForSingleObject},
    };

    let mut name = format!("Local\\DISP.AppContainer.Profile.{identity}")
        .encode_utf16()
        .collect::<Vec<_>>();
    name.push(0);
    // SAFETY: the name is terminated and the null security pointer is permitted.
    let handle = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the new/opened mutex handle transfers immediately.
    let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.cast()) };
    use std::os::windows::io::AsRawHandle;
    // Profile creation should be brief. A stuck or malicious peer causes a
    // controlled launch failure rather than a weaker unsynchronized path.
    let waited = unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 30_000) };
    match waited {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(WindowsMutexGuard(handle)),
        WAIT_TIMEOUT => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out serializing Windows AppContainer profile creation",
        )),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(windows)]
fn create_windows_component_appcontainer(target: &Path) -> io::Result<WindowsSid> {
    use sha2::{Digest, Sha256};
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::ERROR_ALREADY_EXISTS,
        Security::{
            Isolation::{CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName},
            PSID,
        },
    };

    let mut digest = Sha256::new();
    for unit in target.as_os_str().encode_wide() {
        digest.update(unit.to_le_bytes());
    }
    let fingerprint = format!("{:x}", digest.finalize());
    // CreateAppContainerProfile limits names to 64 characters. The prefix plus
    // 48 hexadecimal digest characters is 63 and still provides 192 identity bits.
    let identity = format!("DISP.Component.{}", &fingerprint[..48]);
    let _profile_lock = lock_windows_appcontainer_profile(&identity)?;
    let mut identity = identity.encode_utf16().collect::<Vec<_>>();
    identity.push(0);
    let mut display = "DISP foreign component".encode_utf16().collect::<Vec<_>>();
    display.push(0);
    let mut description = "Networkless DISP component sandbox"
        .encode_utf16()
        .collect::<Vec<_>>();
    description.push(0);
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: all strings are terminated and live for the call; a zero
    // capability count permits a null capability pointer.
    let created = unsafe {
        CreateAppContainerProfile(
            identity.as_ptr(),
            display.as_ptr(),
            description.as_ptr(),
            ptr::null(),
            0,
            &mut sid,
        )
    };
    const HRESULT_ALREADY_EXISTS: i32 = 0x8007_00B7u32 as i32;
    if created == HRESULT_ALREADY_EXISTS || created == ERROR_ALREADY_EXISTS as i32 {
        // SAFETY: the identity is terminated and the output pointer is writable.
        let derived =
            unsafe { DeriveAppContainerSidFromAppContainerName(identity.as_ptr(), &mut sid) };
        if derived < 0 {
            let code = (derived as u32 & 0xffff) as i32;
            return Err(io::Error::other(format!(
                "could not derive the Windows component AppContainer SID (error {code}): {}",
                io::Error::from_raw_os_error(code)
            )));
        }
    } else if created < 0 {
        let code = (created as u32 & 0xffff) as i32;
        return Err(io::Error::other(format!(
            "could not create the Windows component AppContainer profile (error {code}): {}",
            io::Error::from_raw_os_error(code)
        )));
    }
    if sid.is_null() {
        return Err(io::Error::other(
            "Windows AppContainer profile returned a null SID",
        ));
    }
    Ok(WindowsSid(sid))
}

#[cfg(windows)]
fn spawn_windows(
    command: &SandboxedCommand,
    target: &Path,
    directory: Option<&Path>,
    io_mode: IoMode,
    limits: SandboxLimits,
    profile: SandboxProfile,
) -> io::Result<SandboxedProcess> {
    use std::{
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError},
        Security::SECURITY_CAPABILITIES,
        System::Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, UpdateProcThreadAttribute,
        },
        System::WindowsProgramming::PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT,
    };

    let job = create_windows_job(limits, profile)?;
    let component_container = (profile == SandboxProfile::Component)
        .then(|| create_windows_component_appcontainer(target))
        .transpose()?;
    let security_capabilities =
        component_container
            .as_ref()
            .map(|container| SECURITY_CAPABILITIES {
                AppContainerSid: container.0,
                Capabilities: ptr::null_mut(),
                CapabilityCount: 0,
                Reserved: 0,
            });
    let all_application_packages_policy = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let stdio = WindowsStdio::new(io_mode)?;
    let inherited = stdio.child_handles();
    let jobs: [windows_sys::Win32::Foundation::HANDLE; 1] = [job.as_raw_handle().cast()];
    let mut attribute_bytes = 0usize;
    // SAFETY: the documented sizing call writes only the required byte count.
    let attribute_count = if security_capabilities.is_some() {
        4
    } else {
        2
    };
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), attribute_count, 0, &mut attribute_bytes)
    };
    if attribute_bytes == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attribute_storage =
        vec![0usize; attribute_bytes.div_ceil(std::mem::size_of::<usize>())];
    let attribute_list = attribute_storage.as_mut_ptr().cast();
    // SAFETY: storage is aligned and large enough for the requested attribute list.
    if unsafe {
        InitializeProcThreadAttributeList(attribute_list, attribute_count, 0, &mut attribute_bytes)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let handles_ok = unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherited.as_ptr().cast(),
            inherited.len() * std::mem::size_of_val(&inherited[0]),
            ptr::null_mut(),
            ptr::null(),
        )
    } != 0;
    let job_ok = unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            jobs.as_ptr().cast(),
            std::mem::size_of_val(&jobs),
            ptr::null_mut(),
            ptr::null(),
        )
    } != 0;
    let security_ok = security_capabilities
        .as_ref()
        .is_none_or(|capabilities| unsafe {
            UpdateProcThreadAttribute(
                attribute_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (capabilities as *const SECURITY_CAPABILITIES).cast(),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                ptr::null_mut(),
                ptr::null(),
            ) != 0
        });
    let lpac_ok = security_capabilities.as_ref().is_none_or(|_| unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
            (&raw const all_application_packages_policy).cast(),
            std::mem::size_of_val(&all_application_packages_policy),
            ptr::null_mut(),
            ptr::null(),
        ) != 0
    });
    if !handles_ok || !job_ok || !security_ok || !lpac_ok {
        let error = io::Error::last_os_error();
        // SAFETY: initialization succeeded, so deletion is required exactly once.
        unsafe { DeleteProcThreadAttributeList(attribute_list) };
        return Err(error);
    }

    let mut application = target.as_os_str().encode_wide().collect::<Vec<_>>();
    application.push(0);
    let mut command_line = windows_command_line(target, &command.arguments)?;
    let environment = windows_environment(command, profile == SandboxProfile::Component)?;
    let wide_directory = directory.map(|directory| {
        let mut value = directory.as_os_str().encode_wide().collect::<Vec<_>>();
        value.push(0);
        value
    });
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb =
        u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).expect("startup information size");
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = inherited[0];
    startup.StartupInfo.hStdOutput = inherited[1];
    startup.StartupInfo.hStdError = inherited[2];
    startup.lpAttributeList = attribute_list;
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: all pointers reference live, terminated buffers; the handle and
    // job attributes remain live until CreateProcess returns.
    let directory_pointer = wide_directory
        .as_ref()
        .map_or(ptr::null(), |value| value.as_ptr());
    let flags = CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            flags,
            environment.as_ptr().cast(),
            directory_pointer,
            &raw const startup.StartupInfo,
            &mut process,
        )
    };
    let creation_error = if created == 0 {
        Some(unsafe { GetLastError() })
    } else {
        None
    };
    // SAFETY: initialization succeeded and CreateProcess has returned.
    unsafe { DeleteProcThreadAttributeList(attribute_list) };
    if let Some(code) = creation_error {
        return Err(io::Error::other(format!(
            "could not create the Windows sandbox process (error {code}): {}",
            io::Error::from_raw_os_error(code as i32)
        )));
    }
    // SAFETY: the primary thread handle is no longer needed; the process was
    // associated with the job atomically during creation.
    unsafe { CloseHandle(process.hThread) };
    // SAFETY: ownership of the returned process handle transfers here.
    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess.cast()) };
    let (stdin, stdout, stderr) = stdio.into_parent_streams();
    Ok(SandboxedProcess {
        child: PlatformChild::Windows(WindowsChild {
            process: process_handle,
            stdin,
            stdout,
            stderr,
            status: None,
        }),
        guard: SandboxGuard::Windows { job },
    })
}

#[cfg(windows)]
struct WindowsStdio {
    input: std::os::windows::io::OwnedHandle,
    output: std::os::windows::io::OwnedHandle,
    error: std::os::windows::io::OwnedHandle,
    parent_input: Option<std::os::windows::io::OwnedHandle>,
    parent_output: Option<std::os::windows::io::OwnedHandle>,
    parent_error: Option<std::os::windows::io::OwnedHandle>,
}

#[cfg(windows)]
impl WindowsStdio {
    fn new(mode: IoMode) -> io::Result<Self> {
        match mode {
            IoMode::Piped => {
                let input = windows_null_handle(true)?;
                let (parent_output, output) = windows_output_pipe()?;
                let (parent_error, error) = windows_output_pipe()?;
                Ok(Self {
                    input,
                    output,
                    error,
                    parent_input: None,
                    parent_output: Some(parent_output),
                    parent_error: Some(parent_error),
                })
            }
            IoMode::Streaming => {
                let (input, parent_input) = windows_input_pipe()?;
                let (parent_output, output) = windows_output_pipe()?;
                let (parent_error, error) = windows_output_pipe()?;
                Ok(Self {
                    input,
                    output,
                    error,
                    parent_input: Some(parent_input),
                    parent_output: Some(parent_output),
                    parent_error: Some(parent_error),
                })
            }
            IoMode::Inherited => Ok(Self {
                input: windows_duplicate_standard(true, -10i32 as u32)?,
                output: windows_duplicate_standard(false, -11i32 as u32)?,
                error: windows_duplicate_standard(false, -12i32 as u32)?,
                parent_input: None,
                parent_output: None,
                parent_error: None,
            }),
        }
    }

    fn child_handles(&self) -> [windows_sys::Win32::Foundation::HANDLE; 3] {
        use std::os::windows::io::AsRawHandle;
        [
            self.input.as_raw_handle().cast(),
            self.output.as_raw_handle().cast(),
            self.error.as_raw_handle().cast(),
        ]
    }

    fn into_parent_streams(self) -> (Option<fs::File>, Option<fs::File>, Option<fs::File>) {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        let stdin = self.parent_input.map(|handle| {
            // SAFETY: ownership moves from `OwnedHandle` into `File`.
            unsafe { fs::File::from_raw_handle(handle.into_raw_handle()) }
        });
        let stdout = self.parent_output.map(|handle| {
            // SAFETY: ownership moves from `OwnedHandle` into `File`.
            unsafe { fs::File::from_raw_handle(handle.into_raw_handle()) }
        });
        let stderr = self.parent_error.map(|handle| {
            // SAFETY: ownership moves from `OwnedHandle` into `File`.
            unsafe { fs::File::from_raw_handle(handle.into_raw_handle()) }
        });
        (stdin, stdout, stderr)
    }
}

#[cfg(windows)]
fn windows_input_pipe() -> io::Result<(
    std::os::windows::io::OwnedHandle,
    std::os::windows::io::OwnedHandle,
)> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
        Security::SECURITY_ATTRIBUTES,
        System::Pipes::CreatePipe,
    };
    let security = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
            .expect("security attributes size"),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    // SAFETY: both out-pointers and attributes are valid.
    if unsafe { CreatePipe(&mut read, &mut write, &raw const security, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of both new handles transfers immediately.
    let read = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(read.cast()) };
    let write = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(write.cast()) };
    use std::os::windows::io::AsRawHandle;
    // SAFETY: the parent write end is valid and must not cross into the child.
    if unsafe { SetHandleInformation(write.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((read, write))
}

#[cfg(windows)]
fn windows_output_pipe() -> io::Result<(
    std::os::windows::io::OwnedHandle,
    std::os::windows::io::OwnedHandle,
)> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
        Security::SECURITY_ATTRIBUTES,
        System::Pipes::CreatePipe,
    };
    let security = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
            .expect("security attributes size"),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    // SAFETY: both out-pointers and attributes are valid.
    if unsafe { CreatePipe(&mut read, &mut write, &raw const security, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of both new handles transfers immediately.
    let read = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(read.cast()) };
    let write = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(write.cast()) };
    use std::os::windows::io::AsRawHandle;
    // SAFETY: the parent read end is valid and must not cross into the child.
    if unsafe { SetHandleInformation(read.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((read, write))
}

#[cfg(windows)]
fn windows_null_handle(read: bool) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    };
    let security = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
            .expect("security attributes size"),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let name = [b'N' as u16, b'U' as u16, b'L' as u16, 0];
    // SAFETY: the name is terminated and all arguments are valid constants.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            if read { GENERIC_READ } else { GENERIC_WRITE },
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &raw const security,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the newly opened handle transfers here.
    Ok(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.cast()) })
}

#[cfg(windows)]
fn windows_duplicate_standard(
    read: bool,
    which: u32,
) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, INVALID_HANDLE_VALUE},
        System::{Console::GetStdHandle, Threading::GetCurrentProcess},
    };
    // SAFETY: requesting a process standard handle has no pointer preconditions.
    let source = unsafe { GetStdHandle(which) };
    if source.is_null() || source == INVALID_HANDLE_VALUE {
        return windows_null_handle(read);
    }
    let current = unsafe { GetCurrentProcess() };
    let mut duplicate = ptr::null_mut();
    // SAFETY: source/current are valid and the out-pointer is writable.
    if unsafe {
        DuplicateHandle(
            current,
            source,
            current,
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the duplicate transfers here.
    Ok(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(duplicate.cast()) })
}

#[cfg(windows)]
fn windows_command_line(target: &Path, arguments: &[OsString]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    windows_quote_argument(&mut line, target.as_os_str())?;
    for argument in arguments {
        line.push(b' ' as u16);
        windows_quote_argument(&mut line, argument)?;
    }
    line.push(0);
    Ok(line)
}

#[cfg(windows)]
fn windows_quote_argument(line: &mut Vec<u16>, argument: &OsStr) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let value = argument.encode_wide().collect::<Vec<_>>();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox argument contains NUL",
        ));
    }
    let quoted = value.is_empty()
        || value
            .iter()
            .any(|value| matches!(*value, 9 | 10 | 11 | 12 | 13 | 32 | 34));
    if !quoted {
        line.extend_from_slice(&value);
        return Ok(());
    }
    line.push(b'"' as u16);
    let mut slashes = 0usize;
    for value in value {
        if value == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if value == b'"' as u16 {
            line.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            line.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        line.push(value);
    }
    line.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    line.push(b'"' as u16);
    Ok(())
}

#[cfg(windows)]
fn windows_environment(
    command: &SandboxedCommand,
    appcontainer_component: bool,
) -> io::Result<Vec<u16>> {
    use std::{collections::BTreeMap, os::windows::ffi::OsStrExt};
    let mut values = BTreeMap::<String, (OsString, OsString)>::new();
    if !command.clear_environment {
        for (name, value) in std::env::vars_os() {
            values.insert(name.to_string_lossy().to_uppercase(), (name, value));
        }
    }
    for (name, value) in &command.environment {
        let key = name.to_string_lossy().to_uppercase();
        match value {
            Some(value) => {
                values.insert(key, (name.clone(), value.clone()));
            }
            None => {
                values.remove(&key);
            }
        }
    }
    if appcontainer_component {
        for name in ["LOCALAPPDATA", "SYSTEMROOT", "USERPROFILE"] {
            let value = std::env::var_os(name).ok_or_else(|| {
                io::Error::other(format!(
                    "Windows AppContainer launch requires the host {name} environment value"
                ))
            })?;
            values.insert(name.to_owned(), (name.into(), value));
        }
    }
    let mut block = Vec::new();
    for (_, (name, value)) in values {
        let name = name.encode_wide().collect::<Vec<_>>();
        let value = value.encode_wide().collect::<Vec<_>>();
        if name.is_empty()
            || name.contains(&0)
            || name.contains(&(b'=' as u16))
            || value.contains(&0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sandbox environment contains an invalid name or value",
            ));
        }
        block.extend_from_slice(&name);
        block.push(b'=' as u16);
        block.extend_from_slice(&value);
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

enum PlatformChild {
    #[cfg(unix)]
    Standard(Child),
    #[cfg(windows)]
    Windows(WindowsChild),
}

#[cfg(windows)]
struct WindowsChild {
    process: std::os::windows::io::OwnedHandle,
    stdin: Option<fs::File>,
    stdout: Option<fs::File>,
    stderr: Option<fs::File>,
    status: Option<ExitStatus>,
}

impl PlatformChild {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        #[cfg(unix)]
        let Self::Standard(child) = self;
        #[cfg(unix)]
        return child.stdin.take().map(|writer| Box::new(writer) as _);
        #[cfg(windows)]
        let Self::Windows(child) = self;
        #[cfg(windows)]
        return child.stdin.take().map(|writer| Box::new(writer) as _);
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        #[cfg(unix)]
        let Self::Standard(child) = self;
        #[cfg(unix)]
        return child.try_wait();
        #[cfg(windows)]
        let Self::Windows(child) = self;
        #[cfg(windows)]
        return child.try_wait();
    }

    fn kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        let Self::Standard(child) = self;
        #[cfg(unix)]
        return child.kill();
        #[cfg(windows)]
        let Self::Windows(child) = self;
        #[cfg(windows)]
        return child.kill();
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        let Self::Standard(child) = self;
        #[cfg(unix)]
        return child.wait();
        #[cfg(windows)]
        let Self::Windows(child) = self;
        #[cfg(windows)]
        return child.wait();
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        #[cfg(unix)]
        let Self::Standard(child) = self;
        #[cfg(unix)]
        return child.stdout.take().map(|reader| Box::new(reader) as _);
        #[cfg(windows)]
        let Self::Windows(child) = self;
        #[cfg(windows)]
        return child.stdout.take().map(|reader| Box::new(reader) as _);
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        #[cfg(unix)]
        let Self::Standard(child) = self;
        #[cfg(unix)]
        return child.stderr.take().map(|reader| Box::new(reader) as _);
        #[cfg(windows)]
        let Self::Windows(child) = self;
        #[cfg(windows)]
        return child.stderr.take().map(|reader| Box::new(reader) as _);
    }
}

#[cfg(windows)]
impl WindowsChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        use std::os::windows::{io::AsRawHandle, process::ExitStatusExt};
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        };
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        if unsafe { WaitForSingleObject(self.process.as_raw_handle().cast(), 0) } != WAIT_OBJECT_0 {
            return Ok(None);
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle().cast(), &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let status = ExitStatus::from_raw(code);
        self.status = Some(status);
        Ok(Some(status))
    }

    fn kill(&mut self) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Threading::TerminateProcess;
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { TerminateProcess(self.process.as_raw_handle().cast(), 124) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{INFINITE, WaitForSingleObject},
        };
        if let Some(status) = self.status {
            return Ok(status);
        }
        if unsafe { WaitForSingleObject(self.process.as_raw_handle().cast(), INFINITE) }
            != WAIT_OBJECT_0
        {
            return Err(io::Error::last_os_error());
        }
        self.try_wait()?
            .ok_or_else(|| io::Error::other("sandbox process wait returned no status"))
    }
}

pub struct SandboxedProcess {
    child: PlatformChild,
    guard: SandboxGuard,
}

impl SandboxedProcess {
    pub fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.child.take_stdin()
    }

    pub fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child.take_stdout()
    }

    pub fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child.take_stderr()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.guard.terminate()?;
        }
        Ok(status)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait()?;
        self.guard.terminate()?;
        Ok(status)
    }

    pub fn kill_tree(&mut self) -> io::Result<ExitStatus> {
        self.guard.terminate()?;
        let _ = self.child.kill();
        self.child.wait()
    }

    fn wait_status(&mut self, limits: SandboxLimits) -> io::Result<ExitStatus> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            if started.elapsed() >= Duration::from_millis(limits.wall_millis as u64) {
                self.terminate_and_wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "sandbox process tree exceeded its wall deadline",
                ));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_with_output(&mut self, limits: SandboxLimits) -> io::Result<Output> {
        let stdout = self.child.take_stdout().expect("sandbox stdout is piped");
        let stderr = self.child.take_stderr().expect("sandbox stderr is piped");
        let overflow = Arc::new(AtomicBool::new(false));
        let budget = Arc::new(Mutex::new(0usize));
        let stdout_overflow = Arc::clone(&overflow);
        let stderr_overflow = Arc::clone(&overflow);
        let stdout_budget = Arc::clone(&budget);
        let stderr_budget = Arc::clone(&budget);
        let limit = limits.output_bytes;
        let stdout =
            thread::spawn(move || bounded_read(stdout, limit, stdout_budget, stdout_overflow));
        let stderr =
            thread::spawn(move || bounded_read(stderr, limit, stderr_budget, stderr_overflow));
        let started = Instant::now();
        let status = loop {
            if overflow.load(Ordering::Acquire) {
                self.terminate_and_wait();
                break Err(io::Error::other(
                    "sandbox process output exceeds the configured byte limit",
                ));
            }
            if let Some(status) = self.try_wait()? {
                break Ok(status);
            }
            if started.elapsed() >= Duration::from_millis(limits.wall_millis as u64) {
                self.terminate_and_wait();
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "sandbox process tree exceeded its wall deadline",
                ));
            }
            thread::sleep(Duration::from_millis(2));
        };
        let stdout = stdout
            .join()
            .map_err(|_| io::Error::other("sandbox stdout reader panicked"))??;
        let stderr = stderr
            .join()
            .map_err(|_| io::Error::other("sandbox stderr reader panicked"))??;
        Ok(Output {
            status: status?,
            stdout,
            stderr,
        })
    }

    fn terminate_and_wait(&mut self) {
        let _ = self.guard.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for SandboxedProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.terminate_and_wait();
        } else {
            let _ = self.guard.terminate();
        }
    }
}

fn bounded_read(
    mut reader: impl Read,
    limit: usize,
    budget: Arc<Mutex<usize>>,
    overflow: Arc<AtomicBool>,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let count = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let mut used = budget
            .lock()
            .map_err(|_| io::Error::other("sandbox output budget is poisoned"))?;
        let available = limit.saturating_sub(*used);
        let retained = available.min(count);
        *used += retained;
        drop(used);
        bytes.extend_from_slice(&chunk[..retained]);
        if retained != count {
            overflow.store(true, Ordering::Release);
        }
    }
    Ok(bytes)
}
