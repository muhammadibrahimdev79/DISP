# Resource limits and exhaustion behavior

Status: implemented Pass 017 Candidate 1 contract.

DISP treats resource exhaustion as a controlled security boundary. Compiler input and generated
work are bounded before later stages run. Runtime meters reject the operation that would cross a
quota; they do not wrap counters, continue with partial output, or silently disable the limit.

The canonical policy values live in `compiler/src/limits.rs`. Enforcement stays beside the
resource so bypasses are visible in review. Native code receives the same runtime defaults in a
generated prelude rather than maintaining handwritten duplicate values.

## Runtime quotas

| Resource | Candidate 1 default | Native enforcement | Interpreter enforcement | Operator control |
|---|---:|---|---|---|
| execution work | 100,000,000 charged MIR blocks / interpreter operations | process-wide atomic counter shared by threads and async work | shared atomic counter across the root interpreter and spawned threads | `DISP_MAX_STEPS` |
| printed output | 16 MiB including newlines | charged before `stdout` is changed | charged before the output line is committed | `DISP_MAX_OUTPUT_BYTES` |
| call depth | 32 synchronous calls per thread | thread-local entry/return accounting | function and closure call accounting | `DISP_MAX_CALL_DEPTH` |
| live tasks | 4,096 | process-wide task-state counter released after final handle/executor cleanup | root-shared counter released with the final task state | `DISP_MAX_TASKS` |
| live runtime threads | 256 | process-wide counter covering user and runtime helper threads | root-shared counter covering spawned DISP threads | `DISP_MAX_THREADS` |
| child-process launch attempts | 256 | process-wide monotonic counter charged before each run/start | root-shared monotonic counter charged before each run/start | `DISP_MAX_PROCESS_STARTS` |
| live resource handles | 4,096 | shared counter for channels, TCP/TLS/UDP state, database/DataStore state, regular `FILE` opens, active HTTP/process operations, and started children | root-shared counter for channels, sockets, databases/DataStores, sync/async filesystem and HTTP operations, process runs, and started children | `DISP_MAX_HANDLES` |
| one file write/copy | 64 MiB final content | common sync/async/append/copy transaction stages, syncs, and replaces in one commit | the same same-directory transaction and final-size validation | `DISP_MAX_FILE_WRITE_BYTES` |
| managed memory | 256 MiB live requested bytes | process-wide atomic `disp_alloc`/`disp_realloc`/`disp_dealloc` accounting | root-shared explicit `Memory` plus retained object-graph accounting across frames, nested values, and shared runtime state | `DISP_MAX_MEMORY_BYTES` |

Every `DISP_MAX_*` value is a positive base-10 integer. Missing values select the documented
default. Zero, non-decimal text, invalid encoding in the interpreter, and host-size overflow fail
closed. Native quota exhaustion writes one `DISP runtime resource limit exceeded` message to
standard error and exits with status 101. The interpreter returns a runtime diagnostic.

Execution work is deterministic fuel, not elapsed wall-clock time. Blocking network and process
operations use explicit deadlines/timeouts. CPU and memory consumed inside a foreign library or
child process are outside the managed runtime meter and require the Pass 018 sandbox boundary.

## Compiler quotas

| Area | Bound |
|---|---:|
| one source file | 16 MiB |
| one project | 1,024 modules, 64 MiB source, import depth 128 |
| package manifest | 64 KiB |
| package graph | 512 packages, dependency depth 128 |
| package sources | 16,384 files, 256 MiB |
| lockfile | 4 MiB |
| expression nesting | 32 parser recursion levels |
| operator / call chain | 256 operations |
| structured generation | depth 64, 4,096 repeats/alternatives, 65,536 generated nodes |
| compile-time evaluation | 100,000 steps, depth 128, 65,536 value nodes, 1 MiB strings |
| native specialization | 16,384 monomorphized functions |
| generated native C | 256 MiB |

Compiler limits produce source-spanned diagnostics whenever a source location exists. Generated C
is size-checked before it is written or passed to an external compiler.

## Domain-specific bounds

| Boundary | Bound |
|---|---:|
| URL | 8,192 bytes |
| JSON | 16 MiB, nesting 128, 4,096 object keys |
| HTTP | 64 KiB headers, 16 MiB body, 100 request headers, 10 redirects |
| database | 1 MiB SQL, 100,000 returned rows, 4,096 columns, 16 MiB JSON result |
| DISP Data | 64 MiB snapshot, 4,096 tables/fields, 100,000 rows/table, 16 MiB row |
| child process | 4,096 arguments/environment overrides, 1 MiB arguments, 16 MiB input/output stream |
| compiler tool process | 2 GiB memory, 300 s CPU, 256 processes, 600 s wall time, 16 MiB aggregate diagnostics |
| TCP/TLS read | 16 MiB per requested read |
| UDP | 65,535-byte receive request, 65,507-byte send payload |

These protocol and storage limits are enforced in both execution paths where the facility is
available. API-requested smaller limits remain authoritative.

## Exhaustion invariants

- Counter addition is overflow-checked.
- A failed output charge emits none of the rejected print.
- Explicit memory and requested collection-capacity failures occur before calling the platform
  allocator. Retained interpreter graphs are reconciled after each statement and call-frame
  transition, and independently valid objects still share one aggregate ceiling.
- An oversized file overwrite, append, async write, or copy fails before opening or truncating the
  destination. Append is bounded by the resulting content, not only the appended fragment.
- Accepted writes and copies use a create-new staging file in the destination directory. The
  runtime writes the complete content, flushes and synchronizes it, preserves applicable file
  permissions, and only then atomically replaces the destination. Every preparation or commit
  failure removes the staging file and leaves the prior destination bytes unchanged.
- Reallocation charges only live-size growth and releases shrinkage; it does not double-charge the
  old and replacement blocks.
- Spawned interpreter work and native threads cannot evade process/root execution and output
  counters.
- Live task and thread permits are released exactly once when their final state or OS work exits.
- Handle permits are process/root-wide. Terminal `close` releases reusable channel, socket, and
  database slots; child/process/HTTP/file slots release after final cleanup or operation return.
- Invalid quota configuration never falls back to an unlimited value.
- Runtime quota controls do not grant capabilities and cannot widen an `unsafe uses` contract.

The interpreter counts retained String/CString/Path/URL/JSON and collection storage recursively,
including arrays, slices, maps, sets, structs, enums, closures, futures, tasks, channels, mutex
payloads, HTTP/process buffers, and in-memory DISP Data tables. Shared `Arc` states are counted once
per interpreter graph traversal; independently running interpreter workers contribute through the
same root budget. Explicit `Memory` payloads retain their own exact live permits and are not
double-counted by graph traversal.

## Child sandbox boundary

Pass 018 now enforces child address-space/committed-memory, CPU-time, and process-count controls at
the OS boundary. Windows uses Job Objects and tree termination. The Linux fallback applies
`RLIMIT_AS`, `RLIMIT_CPU`, `RLIMIT_NPROC` defense in depth, a seccomp-locked process group,
no-new-privs, and descriptor sanitization before `exec`. A verified fixed-identity helper upgrades
this to root-owned cgroup v2 aggregate memory, PID, CPU, and wall enforcement. Set
`DISP_LINUX_HARD_SANDBOX=required` when deployment policy must reject the fallback; `auto` is the
default and `off` explicitly selects it. Compiler/linker tools use a separate, larger bounded
profile and the same process-tree launcher. See [SANDBOX.md](SANDBOX.md) for guarantees,
installation status, and explicit non-guarantees. Future macro/build-script hosts remain disabled
until their capability profile is implemented rather than being treated as covered by the managed-
runtime meter.
