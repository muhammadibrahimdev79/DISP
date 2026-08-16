# Out-of-process component protocol

Status: Pass 018 implemented transport foundation. This is not the future Page component syntax
and does not turn arbitrary foreign code into safe in-process code.

DISP uses `disp.component.v1` when foreign code is not trusted to share the compiler or runtime
address space. The host starts a canonical executable through the common OS process-tree sandbox,
clears its ambient environment, adds `DISP_COMPONENT_PROTOCOL=disp.component.v1`, sends one bounded
request frame on standard input, and accepts exactly one bounded response frame on standard output.
Windows additionally retains only `LOCALAPPDATA`, `SYSTEMROOT`, and `USERPROFILE`, which its
AppContainer process bootstrap requires; `PATH` and all other ambient values remain absent. Standard
error is diagnostic output and shares the aggregate output ceiling.

## Wire format

Every request and response is one exact binary frame:

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 8 | ASCII magic `DISPCMP1` |
| 8 | 8 | unsigned 64-bit payload length in network byte order |
| 16 | declared length | uninterpreted payload bytes |

The maximum payload is 8 MiB. A truncated header or body, wrong magic, unrepresentable or oversized
length, and every trailing byte are protocol failures. No decoder guesses encoding, ignores suffix
data, or accepts multiple frames. Higher-level typed schemas may be layered over the payload only
when their own version and bounds are explicit.

## Component resource profile

| Control | Default |
|---|---:|
| `DISP_COMPONENT_MAX_MEMORY_BYTES` | 256 MiB |
| `DISP_COMPONENT_MAX_CPU_MILLIS` | 10,000 ms |
| `DISP_COMPONENT_MAX_PROCESSES` | 8 |
| `DISP_COMPONENT_MAX_WALL_MILLIS` | 30,000 ms |
| `DISP_COMPONENT_MAX_OUTPUT_BYTES` | 16 MiB aggregate |

Controls are parsed in the trusted parent before environment clearing. They must be positive decimal
integers; invalid values fail before executable resolution or process creation. Deadline and output
violations terminate the complete component process tree. Windows uses the shared atomic Job Object
launcher. Linux uses the verified hard cgroup helper when installed or the explicitly documented
resource-contained fallback according to `DISP_LINUX_HARD_SANDBOX`.

On Linux, the component profile is additionally **networkless**. Both the direct fallback and the
verified cgroup helper install a component-only seccomp filter that returns `EPERM` for socket
creation and use, legacy `socketcall`, and `io_uring_setup`. All non-standard inherited file
descriptors are closed before the component image executes, so an undeclared inherited socket is
not an alternate path. The hard helper accepts the internal `--component-networkless` marker; an
older helper treats it as an invalid non-absolute target and fails before component execution.

On Windows, every canonical component path receives a stable, 192-bit-hash-separated AppContainer
profile. Profile creation is serialized inside the host; an existing profile is reopened by deriving
its SID. The launcher grants **zero capability SIDs** and supplies that package SID through
`PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` in the same `CreateProcessW` call that installs the
exact standard-handle allowlist and Job Object. The same attribute list opts out of
`ALL_APPLICATION_PACKAGES`, making the child a Less-Privileged AppContainer (LPAC). A setup failure
is fatal and has no unrestricted fallback. The Job Object also denies access to USER handles outside
the job, clipboard reads/writes,
desktop changes, global atoms, display/system-parameter changes, and window-session exit operations.
A real child probe verifies AppContainer identity, Low integrity, the enabled-privilege ceiling,
absence of enabled `ALL_APPLICATION_PACKAGES` membership, denial of both reads and writes to a
parent-created host file, and network unavailability. Depending on Windows facilities available to
LPAC, Winsock is either unavailable at initialization or an outbound connection fails with
`WSAEACCES`.

## Security claim

The Windows profile is an LPAC with no declared capabilities, while the Linux profile is
networkless and resource-contained. AppContainer blocks ordinary host user files, credentials,
devices, processes, windows, and network access unless Windows grants the package identity access;
regular AppContainers still see selected system resources and objects ACLed for `ALL APPLICATION
PACKAGES`; LPAC deliberately removes that ambient membership. Candidate 1 has no user-facing
filesystem-grant or broker model, and Linux does not yet have mount-namespace filesystem isolation.
Claims therefore stay platform-specific. Trusted `extern C` remains an explicit `unsafe uses
Foreign` boundary; untrusted native code belongs here or behind the future WASI component profile.

The executable and arguments are passed directly without a shell. A successful exit is not enough:
the response must also satisfy the exact frame contract. Nonzero exit, malformed framing, output
flooding, deadline expiration, and invalid policy remain distinct controlled failures.
