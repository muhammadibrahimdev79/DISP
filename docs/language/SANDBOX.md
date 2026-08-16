# Sandbox boundary

Status: Pass 018 active. This document is the Candidate 1 threat model. Windows runtime and
compiler-driver process-tree enforcement is implemented. Linux process-group escape prevention and
the trusted cgroup v2 launcher are implemented and cross-compiled, but privileged installation,
hostile helper execution, and the complete non-Windows matrix remain release gates. The overall
sandbox must not yet be described as complete.

DISP capability checking answers which effects a program is allowed to request. A sandbox answers
what an executed component can actually do after it becomes hostile or compromised. Both layers
are required: a `Process` capability authorizes a launch, but never authorizes escape from the
selected containment profile.

## Trust zones

| Zone | Examples | Candidate 1 treatment |
|---|---|---|
| managed DISP code | interpreter and generated native runtime | language ownership/effect rules plus Pass 017 quotas |
| launched application tree | `Process.run`, `ProcessCommand.start`, `disp run` | OS process-tree containment, CPU/memory/process-count ceilings, deadline termination, bounded I/O, no breakaway |
| compiler toolchain | configured or bundled C compiler and linker | canonical executable identity, argument-only invocation, bounded output/time/tree, explicit generated inputs/outputs |
| package build extension | future build scripts and procedural macros | disabled until an out-of-process capability profile exists; never loaded into the compiler process |
| foreign component | `external "C"` library | trusted in-process only; an untrusted component must use an out-of-process/WASI boundary |

The filesystem, registry, network, GUI, device, and credential restrictions are separate from
resource containment. A platform profile that only applies CPU and memory limits must report
itself as `resource-contained`, not `isolated`.

## Mandatory invariants

- The child cannot execute user code before it belongs to the containment object. The shared
  Windows launcher supplies the Job Object atomically in the process-creation attribute list;
  generated native programs create suspended, assign the job, and resume only after association.
- Descendants inherit the same containment tree. No breakaway flag is enabled; Linux descendants
  cannot change session/process group or create/join namespaces after the launch boundary.
- Closing or dropping the final parent-side containment handle terminates every remaining process
  in the tree.
- Timeout and cancellation terminate the tree, not only its root process.
- CPU time, committed/address-space memory, and simultaneous process count are bounded by validated
  positive decimal controls. Invalid configuration fails before launch.
- Only explicitly selected standard handles are inherited. Sandbox/job/cgroup handles are not
  inherited. Linux marks every descriptor above standard input/output/error close-on-exec before
  installing the final filter.
- A containment setup failure is a launch failure; execution never falls back to an unrestricted
  child.
- Runtime/toolchain resource containment alone does not claim filesystem or network isolation;
  component profiles report their separately tested platform authority restrictions.

## Candidate 1 controls

| Control | Default | Windows enforcement | Linux enforcement |
|---|---:|---|---|
| `DISP_CHILD_MAX_MEMORY_BYTES` | 512 MiB | Job Object aggregate committed-memory limit | `RLIMIT_AS` before `execve` |
| `DISP_CHILD_MAX_CPU_MILLIS` | 60,000 ms | Job Object aggregate user-time limit | `RLIMIT_CPU` plus parent deadline |
| `DISP_CHILD_MAX_PROCESSES` | 64 | Job Object active-process limit | verified hard helper: cgroup v2 `pids.max`; fallback: seccomp-locked group plus `RLIMIT_NPROC` |

On Windows, Job Objects manage a process tree as a unit; descendants are associated by default,
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates the tree when the last job handle closes, and
memory/time/process limits are enforced by the kernel. The shared Rust launcher passes the job in
`PROC_THREAD_ATTRIBUTE_JOB_LIST`, making association part of creation. Its
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` contains exactly the three selected standard-stream handles.

Generated native programs create targets suspended, assign the Job Object, and only then resume
the primary thread. The interpreter, compiler/linker, and `disp run` use the same direct shared
launcher without an intermediate helper process. The process and job handles remain owned by the
child state, and timeout, kill, normal root exit, or final drop terminates remaining descendants
before pipe readers are joined.

On Linux, `RLIMIT_AS` bounds address space and `RLIMIT_CPU` bounds CPU seconds. Every launch first
creates a dedicated process group, sets `PR_SET_NO_NEW_PRIVS`, and installs an inherited seccomp
filter before `execve`. The filter denies `setpgid`, `setsid`, `unshare`, `setns`, `ptrace`, cross-
process memory access, and PID-file-descriptor duplication. It reports `clone3` as unavailable so
libc can fall back safely, while ordinary `clone` remains available only when no namespace flags
are present. An audit-architecture mismatch terminates the process instead of interpreting syscall
numbers under the wrong ABI.

Before installing that filter, Linux calls `close_range(3, UINT_MAX, CLOSE_RANGE_CLOEXEC)`. Kernels
without that operation use a bounded `fcntl(FD_CLOEXEC)` sweep derived from `RLIMIT_NOFILE`; errors
other than unused descriptor slots abort the launch. Descriptors remain available to the trusted
pre-exec error path but cannot cross into the requested image.

This makes the process group a kernel-enforced cleanup boundary: descendants cannot move out of it
before timeout/cancellation sends `SIGKILL` to the group. `RLIMIT_NPROC` remains per real user ID,
counts threads, and is not enforced for privileged identities, so it is defense in depth rather
than a per-sandbox aggregate count. Only the verified hard helper upgrades this to a per-sandbox
cgroup v2 `pids.max` guarantee; the fallback intentionally makes no aggregate-count claim.

## Linux hard cgroup profile

Both the shared Rust launcher and generated native runtime inspect only the fixed system identity
`/usr/libexec/disp-cgroup-launch`. It is eligible only when it is a regular file owned by UID/GID 0,
has the set-user-ID and set-group-ID bits, has no group/other write bit, and is executable. It is
never resolved from `PATH` and no environment variable can substitute another helper.

`DISP_LINUX_HARD_SANDBOX` accepts exactly:

- `auto` (default): use the verified helper when installed; otherwise use the documented resource-
  contained fallback;
- `required`: fail closed before target execution unless the helper identity is verified; or
- `off`: explicitly select the resource-contained fallback.

The minimal helper lives outside the compiler in `support/linux/disp-cgroup-launch.c`. Its root
supervisor creates a root-owned leaf, applies `memory.max`, `memory.oom.group`, and `pids.max`, and
monitors aggregate `cpu.stat` plus the wall deadline. The worker enters the leaf while privileged,
restores the invoking user's real/effective/saved UID and GID, verifies the drop, installs
no-new-privs and the escape filter, closes private descriptors, then executes the canonical target.
On completion or violation the supervisor uses `cgroup.kill` and removes the leaf. The helper and a
complete generated runtime both cross-compile for x86-64 Linux; setuid/setgid installation and hostile
runtime execution remain mandatory evidence before this profile is called complete.

The current differential lifecycle probe starts a real grandchild which would write a delayed
sentinel after its parent disappears. Both the interpreter and generated native runtime remove the
entire tree: the sentinel is never created. Eight isolated sandbox probes demonstrate CPU,
committed-memory, active-process and aggregate-output limits, invalid-policy and command-text
rejection, wall-deadline descendant cleanup, real Windows breakaway denial, and exact inherited-
handle filtering. The focused process suite passes 12/12 and the Windows sandbox suite passes 9/9,
including a generated-runtime hard-profile contract gate. The post-extension-gate complete
all-target matrix passes 490/490 tests across 55 harnesses, with strict Clippy and formatting gates
also clean on Windows. Three host-policy-blocked harness identities (`http`, `sandbox`, and
`tcp_server`) executed all 27 affected assertions through controlled distinct-metadata reroutes.
The Linux Rust launcher
cross-compiles for `x86_64-unknown-linux-gnu`, and a complete generated native C translation unit
cross-compiles for
`x86_64-linux-gnu`. Six Linux-only executable probes cover direct launcher escape attempts,
filter inheritance through both runtime engines, address-space exhaustion, the `RLIMIT_NPROC`
defense-in-depth boundary, rejection of a real unrelated inheritable descriptor, and fail-closed
hard-profile configuration. The cross-platform compiler workflow runs them on Ubuntu; its first
remote execution remains a release gate.

## Compiler and driver profile

Edition 1 manifests reject build-script fields and build-script, macro, procedural-macro, and
plugin sections before loading package source. The diagnostic identifies the exact manifest line
and directs authors to ordinary DISP source or an explicitly sandboxed out-of-process tool. This
is a security boundary, not an unimplemented syntax accident: package-provided code is never
loaded into or executed by the compiler process. `DISP-CORE-0035` and its manifest regression test
make that boundary normative. A future extension host must first provide explicit capability,
resource, input, output, and determinism contracts.

## Foreign component profile

The implemented `disp.component.v1` host provides the first out-of-process foreign-code boundary.
It clears the component environment, supplies the protocol-version marker (plus the three required
Windows AppContainer bootstrap values on Windows), and exchanges one exact length-prefixed binary
request and response with an 8 MiB payload ceiling. Wrong magic,
truncation, oversized lengths, trailing bytes, nonzero exit, deadline expiration, and aggregate
output flooding fail distinctly. Dedicated `DISP_COMPONENT_MAX_*` controls default to 256 MiB
memory, 10 seconds CPU, 8 processes, 30 seconds wall time, and 16 MiB aggregate output. Invalid
controls fail before executable resolution. Four integration tests use a real native component to
verify binary exactness, environment clearing, pre-launch request rejection, hostile framing,
failure diagnostics, output termination, and wall-deadline termination.

The latest complete Windows assertion matrix passes 525/525 tests across 60 harnesses, including
the post-LPAC sandbox probes and Pass 019 cryptographic-foundation tests.
Host-policy-blocked harness identities were executed through controlled metadata, release-profile,
or exact-test reroutes; no assertion was skipped. The component host together with its Linux
sandbox launcher independently cross-compiles to Rust metadata for `x86_64-unknown-linux-gnu`.

This transport has platform-specific authority profiles. The complete wire contract and explicit
non-guarantees are in `COMPONENTS.md`.

Linux strengthens this profile by stacking a component-only seccomp filter that denies socket
creation and operations, legacy `socketcall`, and `io_uring_setup`. The direct fallback installs
it before `execve`; the verified cgroup helper installs the same network-denial layer after dropping
privilege and before executing the target. CI exercises both paths, and the privileged hostile
helper probe requires `socket(AF_INET, SOCK_STREAM, 0)` to fail with `EPERM`. Windows components now
execute in path-separated LPAC profiles with no capability SIDs. AppContainer identity, Low
integrity, absence of enabled `ALL_APPLICATION_PACKAGES` membership, the privilege ceiling, denial
of host-file reads and writes, and network unavailability are verified inside a real child. The
AppContainer SID, LPAC opt-out, exact handles, and Job Object/UI restrictions are all
process-creation attributes, with no weaker fallback.

Every external compiler-driver operation now uses the shared sandbox launcher: target discovery,
C compilation, native linking, sanitizer-runtime discovery, and `disp run`. A program name is
resolved through `PATH` once and canonicalized to an absolute file before creation; program text is
never parsed by a shell. The launcher rejects more than 4,096 arguments or 1 MiB of argument data,
captures a bounded aggregate of standard output plus standard error, and kills the complete tree
on overflow or wall deadline.

| Toolchain control | Default |
|---|---:|
| `DISP_TOOL_MAX_MEMORY_BYTES` | 2 GiB |
| `DISP_TOOL_MAX_CPU_MILLIS` | 300,000 ms |
| `DISP_TOOL_MAX_PROCESSES` | 256 |
| `DISP_TOOL_MAX_WALL_MILLIS` | 600,000 ms |
| `DISP_TOOL_MAX_OUTPUT_BYTES` | 16 MiB aggregate |

All controls are validated positive decimal integers. Invalid policy fails before executable
resolution or creation. Focused evidence verifies output termination, wall-deadline grandchild
cleanup, absence of an intermediate launch helper, command-text injection rejection, denial of a
real `CREATE_BREAKAWAY_FROM_JOB` attempt, and rejection of an unrelated inheritable sentinel handle.

## Failure and evidence matrix

Pass 018 is complete only when automated probes demonstrate:

1. the root and a spawned grandchild die on timeout, cancellation, and parent cleanup;
2. CPU, memory, and process-count violations fail predictably without harming the DISP parent;
3. an attempted Windows breakaway remains in the job or fails;
4. Linux descendants are killed as one process group and cannot gain privilege through `execve`;
5. inherited-handle inspection finds only the declared standard streams;
6. compiler-driver and runtime launch paths cannot bypass the common policy;
7. unsupported platform guarantees fail closed and report the missing boundary precisely.

## Explicit non-guarantees while Pass 018 is active

- Candidate 1's Windows LPAC does not yet expose audited, user-selectable filesystem/capability
  grants or brokered resource access.
- Candidate 1's Linux seccomp policy prevents containment escape; it is not yet a general syscall
  allowlist or a complete mount/network namespace isolation profile.
- In-process C ABI calls share the DISP process authority and address space.
- Build scripts and procedural macros are not an accepted extension mechanism yet.
- The Linux code path and hard helper are implemented and cross-compiled but still require the
  privileged installer, hostile helper tests, and first non-Windows CI execution.

These are release gates, not optional future polish.
