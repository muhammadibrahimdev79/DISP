# DISP 1.0 — 100-pass execution ledger

This is the live engineering ledger for taking DISP from the current developer preview to
a credible 1.0 language and computing platform. The objective remains one coherent, safe,
high-performance language spanning Data, Intelligence, System, and Page.

## Pass contract

A pass is `complete` only when its scoped behavior is implemented, its positive and negative
tests pass, its public behavior is documented, and its security/performance claims have direct
evidence. Existing preview behavior is baseline evidence, not permission to mark a future pass
complete. A pass may be split internally, but the ledger always contains exactly 100 release
passes. Regressions reopen the responsible pass.

Status values are `planned`, `active`, `complete`, and `blocked`. The authoritative evidence is
the referenced source, test, benchmark, audit, or release artifact in the Evidence column.

## Audited starting point

The August 15, 2026 audit found a working Rust bootstrap compiler with a lexer, parser, resolver,
static type checker, ownership analysis, HIR/MIR, interpreter, Windows x64 native C/object
backend, packages with locked local dependencies, concurrency/async, networking/TLS/HTTP,
JSON, SQLite compatibility, and the DISP-owned DataStore. The complete Rust library baseline
passed 31 unit tests. The wider integration run reached `tests/async.rs` and was stopped by
Windows application-control policy error 4551, not a reported DISP test assertion. Intelligence,
Page, non-Windows targets, the language server, debugger, self-hosting, and stable 1.0 guarantees
were explicitly incomplete at audit time.

## Passes 001–010 — language foundation and specification

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 001 | complete | Baseline audit, canonical source formatter, project formatting, and `fmt --check` | 34 library tests, two formatter CLI tests, clippy, all-target build, this ledger |
| 002 | complete | Reconcile implementation into a normative DISP 1.0 core specification | Candidate 1 grammar, 30 normative rules, three passing specification/traceability tests, clippy |
| 003 | complete | Public parser and semantic conformance suite | 31 portable source/project fixtures covering all 30 Candidate 1 rules; static/interpreter/native-required tests pass |
| 004 | complete | Stable diagnostic taxonomy and machine-readable diagnostics | Seven stable stage codes plus driver code, v1 JSON schema, 3 diagnostic/14 CLI/19-rule conformance tests |
| 005 | complete | Effect and capability type model for filesystem, network, process, FFI, GPU, and UI authority | Checked explicit/inferred contracts, transitive propagation, fail-closed callable erasure, six integration tests, 20-case conformance, threat model, all-target regression, clippy |
| 006 | complete | Compile-time evaluation, hygienic metaprogramming, and bounded code generation | Deterministic constant folding; authority-free evaluator budgets; hygienic `Meta.map`; bounded AST generation; 5 constant/5 expansion/17 CLI tests; 21-case native conformance; all-target regression; clippy |
| 007 | complete | Stable generics, traits, associated types, constraints, and coherence | Exact method/capability contracts; `Self.Name` projection; order-independent cycle-safe selection; universal generic Copy validation; 21 generics/13 ownership tests; 22-case conformance; all-target regression |
| 008 | complete | Complete pattern language, exhaustiveness, destructuring, and guards | Recursive usefulness/exhaustiveness matrix; ordered MIR dispatch; struct destructuring; bounded typed `|` alternatives; typed read-only guards; negative literals; 13 pattern tests; 23-case static/interpreter/native conformance; all-target regression; clippy |
| 009 | complete | Structured errors, typed exceptions/effects, propagation, and cleanup semantics | Explicit `Result`/`Option` failure model; exact single-evaluation `?`; no hidden exceptions; reverse lexical and partial-move cleanup; MIR carrier-before-drop proof; closure/async interpreter-native parity; 6 error tests; 25-case conformance; all-target regression; clippy |
| 010 | complete | Editions, feature gates, deprecation, and source compatibility policy | Edition-1 legacy equivalence; explicit bounded fail-closed feature sets; isolated dependency editions; lock invalidation; idempotent `disp migrate` and non-mutating `--check`; 4 compatibility/18 CLI tests; 27 source/project conformance fixtures; all-target regression; clippy |

## Passes 011–020 — safety and secure unsafe execution

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 011 | complete | Formal ownership, borrowing, lifetime, move, and destruction model | Operational `S = (Γ, I, L, O)` model; place-sensitive NLL and join/loop rules; generic/nominal aggregate origins; exact structured destruction; 7 model/13 ownership/9 FFI tests; 28-case conformance; all assertions pass; clippy (Windows policy denied two debug harness artifacts, whose release suites passed 10/10 and 6/6) |
| 012 | complete | Safe initialization, unions, pinning, interior mutability, and aliasing rules | Explicit bounded safety model; 5 memory-safety/19 CLI/14 backend tests; real official LLVM ASan/UBSan execution; GNU/MSVC Clang driver separation; staged sanitizer runtime and cache completeness; 29-case conformance; all-target assertion coverage (HTTP 11/11 independently linked after Windows denied one harness hash); specification; clippy |
| 013 | complete | Safe Unsafe Execution: bounded unsafe regions with explicit capabilities | `unsafe uses RawMemory, Foreign` grammar; lexical non-widening contracts retained in HIR; strict raw/FFI capability checks; transitive call-chain effects; 7 containment tests; positive and negative 30th-rule conformance with native execution; real LLVM ASan/UBSan gate; complete all-target regression; clippy |
| 014 | complete | Checked raw pointers, provenance, alignment, bounds, and lifetime tokens | Distinct `MemoryPtr<T>` / `MemoryMutPtr<T>` fat pointers retain base, extent, element-size/alignment, and source-allocation loans; native helpers reject invalid offset/access before C pointer operations; lifetime origins propagate through offsets, aggregates, assignments, and direct calls; owner conflicts, escape, thread/ABI transfer, and unchecked dereference fail closed; 9 focused tests and two DISP-CORE-0031 cases (33 total fixtures) pass in interpreter/native execution; all-target assertions, specification, clippy, and sanitizer-request fail-closed gate pass (current GCC lacks `libasan`/`libubsan`) |
| 015 | complete | Race-safe atomics, locks, channels, structured threads, and memory ordering | Explicit operation-valid atomic orderings map to Rust/C11; structured threads retain deterministic join/drop cleanup; mutexes are recursively owned with interpreter, Windows, and POSIX parity; bounded generic MPMC `Channel<T>` provides recoverable capacity validation, owned sends, FIFO receive, blocking backpressure, close wakeups/drain semantics, pointer layout, and deterministic queued-message cleanup; 15 focused concurrency tests include capacity-one and four-producer/1,000-message differential stress; DISP-CORE-0032 and 35 conformance fixtures pass in static/interpreter/native modes; full assertion matrix, formatting, all-target checks, clippy, and diff hygiene pass (Windows Application Control-blocked harness hashes were rerun under alternate hashes) |
| 016 | complete | Structured async, cancellation, deadlines, backpressure, and deterministic resource release | Lazy linear futures; scope-bound tasks; consuming `Task.cancel()` with cleanup-before-return; non-consuming `Task.is_finished()`; nested task-tree cancellation before later side effects; first-poll deadlines; typed timeout/closure failures; bounded channel backpressure; started file/TCP/UDP cancellation and TLS/HTTP fault cases; 10 task, 9 async, 9 async-I/O, 7 async-TCP, 7 listener, and 8 UDP tests; DISP-CORE-0033 and 37 static/interpreter/native conformance fixtures; complete all-target assertions, formatting, checks, clippy, and diff hygiene pass. Windows policy-blocked temp artifacts execute through a byte-identical workspace launch fallback rather than being skipped |
| 017 | complete | Resource quotas for memory, CPU, recursion, I/O, processes, and generated output | Canonical policy module and generated native/protocol defaults; process/root execution, output, call-depth, task, thread, launch-attempt, and live-handle meters; native live managed allocator plus interpreter retained object-graph accounting sharing one ceiling with explicit `Memory`; handle coverage across channels, sockets, databases/DataStores, files, HTTP/process work, and live children; same-directory transactional overwrite/append/async-write/copy with final-size bounds, sync-before-replace, permission preservation, destination stability, and staging cleanup on failure; validated `DISP_MAX_*` controls; canonical protocol bounds; 28 focused exhaustion/configuration/preservation tests and DISP-CORE-0034; complete all-target matrix, strict clippy, formatting, and diff hygiene pass under Windows-policy-safe artifact hashes |
| 018 | active | Sandboxed processes, build scripts, macros, and foreign components | Runtime plus compiler/linker/`disp run` containment; canonical executable resolution; bounded tool diagnostics/deadlines; atomic Windows job association; breakaway denial and exact inherited-handle allowlist; Linux seccomp-locked fallback, descriptor sweep, fixed-identity cgroup-v2 helper with aggregate memory/PID/CPU/wall enforcement, hardened installer/service, and hostile probes; Rust/helper/generated-C Linux cross-compilation; Edition 1 build-script/procedural-macro/plugin manifests explicitly fail closed under DISP-CORE-0035; exact 8 MiB `disp.component.v1` binary transport with cleared environment, dedicated finite quotas, hostile framing/output/deadline tests, and DISP-CORE-0036; Linux direct/helper component paths deny socket operations, legacy `socketcall`, and io_uring setup under DISP-CORE-0037; Windows components use zero-capability, path-separated LPAC profiles with atomic Job/UI restrictions and exact handles; live child evidence proves AppContainer/Low identity, privilege ceiling, disabled `ALL_APPLICATION_PACKAGES`, host-file read/write denial, and network unavailability under DISP-CORE-0038/0039; Ubuntu/Windows CI gate; 9/9 focused Windows sandbox probes; latest Windows baseline 500/500 all-target tests across 57 harnesses plus clean strict lint/format gates; Windows-policy-blocked harnesses and native probe executions rerouted with all assertions executed; privileged hostile-helper execution and first Linux CI evidence remain |
| 019 | active | Cryptographic foundations, secure randomness, secrets, and constant-time primitives | RustCrypto/getrandom bootstrap core; OS-only entropy; bounded zeroizing/redacted secrets; SHA-256, HMAC-SHA-256, HKDF-SHA-256; randomized AES-256-GCM-SIV; opaque Ed25519 signing; fixed-policy Argon2id; canonical envelope/key/signature records; stable key IDs; deterministic activation/expiry/revocation verification; interpreter/native parity; versioned caller-buffer `disp-crypto-native` ABI; out-of-process external/hardware Ed25519 provider protocol and provider SDK with zeroizing opaque handles, pinned provider content and key identity, bounded exact frames, no private-key callback, and host-side signature verification under DISP-CORE-0040 through 0053. Stable source keystore APIs, audited TPM/Secure Enclave/PKCS#11 integrations and device grants, broader protocol construction, and independent review remain |
| 020 | active | Compiler/runtime fuzzing, sanitizers, dependency auditing, and vulnerability response | Pinned RustSec scans three graphs under DISP-CORE-0054; cargo-fuzz covers frontend/security frames under 0055; auditable artifacts are scanned under 0056; reporting, release blockers, threat model, and unsafe inventory are normative under 0057; date-pinned Linux Rust ASan/LSan is configured under 0058. Deterministic CycloneDX 1.6 SBOMs cover locked Rust plus Linux/Windows/macOS native resolution under 0059. Each desktop compiler/companion/SBOM set receives job-scoped GitHub OIDC/Sigstore SLSA provenance plus per-binary SBOM attestations outside pull requests under 0060. Local graph/Windows/Mach-O parser evidence passes; hosted platform attestations, additional sanitizers, and independent review remain |

## Passes 021–030 — System

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 021 | complete | Freestanding core profile with no OS, heap, or standard runtime requirement | `build --freestanding` directly emits deterministic machine images without C, assembler, linker, OS, allocator, libc, or language runtime. DISP-CORE-0061 through 0068 cover x86 boot integrity, multi-sector loading, checked exact-width computation, structured control flow, safe calls/recursion, and true byte storage. DISP-CORE-0069 through 0077 build the protected32 transition, relocated stages, exact computation, recursive frames, arrays, `DeviceIo`, exception IDT, and CR0.WP paging. DISP-CORE-0078 through 0086 add independent x86-64 long mode, checked scalars/control flow, guarded functions/recursion, arrays, explicit device I/O, NX execute whitelisting, differentiated faults, quarantined PIC routing, and a capability-controlled PIT timer. DISP-CORE-0087 through 0095 add deterministic Arm64 Images for versioned QEMU virt-8.2: checked exact scalar computation; guarded functions/arrays; exception containment; sparse W^X translation; bounded FDT RAM/PL011 discovery without a fixed device address; and `DeviceIo`-gated exact-width volatile MMIO whose relative offsets are page-bounded, naturally aligned, and ordered. Structural/unit/CLI, synthetic alternate-address execution, and Linux QEMU exact-output gates cover normal, checked-failure, exception, protection, discovery, authorized device access, and rejected out-of-page access. Richer devices, interrupts, relocation, and additional architectures continue under Passes 026–028 rather than extending this completed core-profile milestone |
| 022 | active | Stable C ABI, header generation, callbacks, dynamic libraries, and verified FFI contracts | DISP-CORE-0096 adds deterministic bounded ABI-v1 C headers, exact scalar/raw-pointer declarations, C/C++ guards, transactional CLI output, and direct C11/C++17 compilation evidence. DISP-CORE-0097 adds `export C fn`, `build --library`, real DLL/SO loading from C, status/out-result contracts, thread-local error retrieval, and checked panic containment across a transitively proved allocation-free direct-call graph, including helpers and recursion. DISP-CORE-0098 adds deterministic exact C-to-DISP callback typedefs exercised indirectly by a real C host. DISP-CORE-0099 adds typed context-free `CFunction<fn(...) -> ...>` values, exact `Foreign` authority, thin-pointer lowering, deterministic nested callback/raw-pointer typedefs, null guards, and interpreter/native execution of a real imported callback. DISP-CORE-0100 denies same-thread nested C→DISP→C→DISP entry without disarming the outer failure target. DISP-CORE-0101 adds linear thread-affine owned C registration handles, exact opaque context/release signatures, consuming deregistration, and exactly-once native scope cleanup. DISP-CORE-0102 adds explicit thread-local C-host attachment, deterministic transition failures, active-entry detach denial, and concurrent execution evidence from two foreign threads. DISP-CORE-0103 adds unsafe exact quiesce/release registration adoption and proves a real provider worker is joined before context release. DISP-CORE-0104 adds exact checked export callback handles and real provider-thread delivery of success and contained-failure wrappers. DISP-CORE-0105 atomically registers moved scalar-capturing DISP closures through signature-specific checked context trampolines and proves quiesce-before-capture-drop-before-provider-release. DISP-CORE-0106 extends those environments to structurally Send-compatible resource-owning captures, preserves reusable borrowed invocation, rejects secret/pointer/borrowed state, and proves repeated provider-thread calls before exactly-once capture destruction. DISP-CORE-0107 adds transactional heap-owning export graphs, restores call-depth state after long-jump containment, and survives one thousand allocate-then-fail calls under a leak-detecting memory ceiling. DISP-CORE-0108 adds explicit non-generic `export C struct` records, exact field/size/alignment assertions, nested Outernet packet layouts, strict C11/C++17 compilation, and real C-host round trips through a DISP library. DISP-CORE-0109 compiles the same fixed-record header into Windows x86-64 and i686 target assembly and verifies their distinct register/stack aggregate calling conventions. DISP-CORE-0110 adds thread-local typed rollback for `CRegistration`, proves reverse-order exactly-once release over one thousand contained failures, and preserves fail-closed rejection for every unhooked resource class. Additional handle-class hooks and wider platform ABI evidence remain |
| 023 | planned | C++ interoperability including layouts, exceptions boundary, and generated bindings | Cross-compiler integration fixtures |
| 024 | planned | Custom allocators, arenas, pools, stack allocation, and allocation-free APIs | Allocation accounting and failure-injection tests |
| 025 | planned | SIMD types, intrinsics, vectorization controls, and portable fallbacks | ISA inspection and cross-width correctness benchmarks |
| 026 | active | Hardware access: volatile memory, MMIO, interrupts, assembly, and device capabilities | `DeviceIo` is a distinct inferred/explicit authority. Protected32/x86-64 lower explicitly unsafe byte-port input/output; AArch64 lowers ordered exact-width MMIO only as checked offsets into its authenticated PL011 page. Rejection tests, alternate-address emulation, and Linux QEMU device gates are present. General volatile memory, interrupt control, constrained assembly, register-granular owned device grants, and physical hardware smoke tests remain |
| 027 | planned | Embedded targets, linker scripts, panic policies, and microcontroller HAL foundation | Reproducible firmware builds and board/emulator run |
| 028 | active | Kernel profile: boot, paging, interrupts, scheduling, drivers, and syscall boundaries | Protected32 now constructs and loads 32 DPL0 CPU-exception gates with a known-state fail-closed common handler, then enables a cleared single-table supervisor identity map with a non-present null page and 4 MiB ceiling before `main`. Per-vector diagnostics, PIC/APIC control, W^X, privilege isolation, scheduling, drivers, and syscall boundaries remain |
| 029 | planned | Portable filesystem, process, terminal, environment, time, and OS service APIs | Windows/Linux/macOS behavior matrix |
| 030 | planned | System proof applications: CLI, service, native library, embedded app, and kernel component | Reproducible end-to-end artifacts written primarily in DISP |

## Passes 031–040 — Data

DISP 1.0's default compiler, runtime, standard library, native Data APIs, offline toolchain, and data
file formats MUST NOT require SQLite or another third-party database engine. `DataStore` is the
bootstrap of the DISP-owned engine and is the only foundation for these passes. The preview
`Database`/SQLite boundary is legacy compatibility debt: before the Data passes complete, it MUST
move out of the core toolchain into an optional isolated connector whose absence changes no native
DISP Data behavior. Compatibility is not architecture and cannot become a fallback implementation.
PostgreSQL is a first-class interoperability target, not a foundation dependency: DISP will support
typed PostgreSQL access, migration, replication, and remote deployment through an optional isolated
connector. Removing that connector MUST leave the compiler, runtime, native Data engine, and offline
applications fully functional.

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 031 | active | DISP-owned catalog, stable schema language, constraints, native indexes, migrations, and schema evolution | Field and named composite constraints persist in native v5 catalogs with v1-v4 readability and interpreter/native parity. Safe evolution appends optional fields as `None`, adds required scalar defaults, explicitly renames with `from(old_name)`, adds validated indexes/constraints, and atomically replaces rows/catalog/indexes; rejection and commit failure preserve prior bytes. Destructive, reordered, type-changing, primary-changing, or weakening changes fail closed. Explicit value transformations remain |
| 032 | active | DISP-native typed relational algebra with joins, grouping, aggregates, subqueries, and null semantics | Direct typed `data count`/`data exists` plans execute without source-level list materialization, select field/composite indexes, use O(1) unfiltered native cardinality, and pass durable interpreter/native differential and compile-fail coverage. Grouping, value aggregates, joins, subqueries, and full null semantics remain |
| 033 | planned | DISP-owned cost-based optimizer, statistics, indexes, query plans, and explain tooling | Correctness corpus and measured plan improvements without SQL translation |
| 034 | planned | Native transaction isolation, MVCC, concurrent access, recovery, backup, and restore | Crash/fault/concurrency matrix over DISP's own WAL/page engine |
| 035 | planned | Streaming and incremental dataflow with bounded memory and backpressure | Long-running bounded-resource tests |
| 036 | planned | Columnar arrays, vectorized execution, compression, and zero-copy interchange | Analytical benchmarks and format conformance |
| 037 | planned | First-class but optional isolated PostgreSQL support, plus MySQL, SQLite-migration, object-store, and file-format connectors behind typed capabilities | PostgreSQL interoperability/replication/migration suites plus proof that removing every connector leaves the core compiler/runtime/Data engine functional |
| 038 | planned | Distributed storage, partitioning, replication, consensus boundary, and conflict policy | Multi-node fault and recovery tests |
| 039 | planned | Data security: row/column policy, encryption, audit trails, provenance, and retention | Policy bypass tests and cryptographic audit |
| 040 | planned | Complete typed backend and analytics application with no handwritten routine SQL | Deployable proof application and workload report |

## Passes 041–050 — Intelligence

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 041 | planned | Generic n-dimensional Tensor with checked shape, dtype, stride, view, and ownership semantics | Tensor property tests and interpreter/native parity |
| 042 | planned | CPU tensor kernels with SIMD, threading, fusion, and tuned layouts | Numerical suite and C++/Rust/Python-library comparisons |
| 043 | planned | Reverse/forward automatic differentiation integrated with ordinary DISP functions | Gradient checks across the operator corpus |
| 044 | planned | GPU intermediate representation, kernels, memory spaces, synchronization, and safe device ownership | CPU/GPU differential and race tests |
| 045 | planned | CUDA, Vulkan/compute, Metal, DirectML, and portable accelerator selection | Multi-backend conformance where hardware is available |
| 046 | planned | Neural-network modules, optimizers, losses, datasets, checkpoints, and mixed precision | Training convergence and reproducibility tests |
| 047 | planned | Classical ML, statistics, linear algebra, signal/image, and scientific numerical foundations | Reference-dataset and numerical-tolerance suite |
| 048 | planned | Model import/export and inference for ONNX plus bounded external model runtimes | Cross-runtime model parity and hostile-model tests |
| 049 | planned | First-class agents, tools, structured generation, retrieval, memory, permissions, and audit | Sandboxed agent scenarios and deterministic replay |
| 050 | planned | Train and serve a useful model entirely through DISP without Python orchestration | Reproducible training/inference proof and benchmark report |

## Passes 051–060 — Page

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 051 | planned | Normative Page component, property, event, state, lifecycle, and composition semantics | Parser/type/runtime conformance suite |
| 052 | planned | Typed layout, style, theme, animation, responsive design, and asset pipeline | Golden rendering and invalid-style compile tests |
| 053 | planned | Fine-grained reactive state, derived values, effects, scheduling, and deterministic updates | Dependency-graph and update-order tests |
| 054 | planned | DOM/WebAssembly backend with safe escaping, events, hydration, and browser capabilities | Cross-browser end-to-end and injection tests |
| 055 | planned | Server rendering, static generation, streaming, routing, forms, and metadata | Hydration parity and production-site fixtures |
| 056 | planned | Accessibility semantics, keyboard/focus, localization, bidi, Unicode, and assistive APIs | Automated and manual accessibility gates |
| 057 | planned | Native desktop renderer, windows, menus, clipboard, input, graphics, and OS integration | Windows/Linux/macOS UI fixtures |
| 058 | planned | Mobile renderer, touch, navigation, lifecycle, permissions, packaging, and stores | Android/iOS build and device/emulator flows |
| 059 | planned | Graphics, canvas, 2D/3D, audio, video, game loop, and XR extension boundaries | Interactive demos and frame/resource benchmarks |
| 060 | planned | Full-stack DISP application sharing types across Page, service, Data, and Intelligence | Deployed proof app with security/performance report |

## Passes 061–070 — targets, networking, and interoperability

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 061 | planned | Stable target model and native x64/ARM64 backends for Windows, Linux, and macOS | Cross-target conformance and ABI suites |
| 062 | planned | WebAssembly/WASI backend with component-model interoperability and sandboxing | Browser/WASI conformance and size benchmarks |
| 063 | planned | Reproducible object, static/shared library, debug-info, and link-time optimization pipeline | Bit-for-bit build and linker compatibility tests |
| 064 | planned | Portable sockets, DNS, TLS, HTTP/1.1, HTTP/2, HTTP/3, WebSocket, QUIC, and server APIs | Protocol conformance, fuzzing, and interop matrix |
| 065 | planned | RPC, schemas, serialization, service discovery, retries, load balancing, and observability | Multi-service fault scenarios |
| 066 | planned | Distributed actors/tasks, placement, messaging, supervision, and failure semantics | Multi-node partition/recovery tests |
| 067 | planned | Python import/export, extension modules, environment bridging, and typed binding generation | NumPy/Python package interoperability suite |
| 068 | planned | JavaScript/TypeScript packages, browser modules, Node/Bun/Deno, and generated declarations | Runtime and type-definition compatibility tests |
| 069 | planned | Rust, JVM, .NET, Swift, Go, and legacy ABI bridge strategy with generated bindings | Representative bidirectional integration fixtures |
| 070 | planned | Migration/transpilation tools for C/C++, Rust, Python, SQL, HTML/CSS, JavaScript, and TypeScript | Real-project migration corpus with semantic review |

## Passes 071–080 — developer experience and ecosystem

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 071 | planned | Complete formatter: token spacing, wrapping, comment attachment, configuration policy, and stability | Idempotence corpus and no-semantic-change differential tests |
| 072 | planned | Language server with incremental parsing/checking, completion, navigation, rename, and semantic tokens | Editor protocol and latency tests |
| 073 | planned | Source debugger with breakpoints, stepping, stack/locals, async/tasks, and native debug adapters | Debug scenario suite across targets |
| 074 | planned | Integrated test language/runner with unit, property, fuzz, benchmark, snapshot, and coverage modes | Self-tested runner and CI fixtures |
| 075 | planned | Documentation generator, runnable examples, API search, diagrams, and versioned hosting | Documentation build and link/example verification |
| 076 | planned | Package resolver with registry/Git/path sources, features, semantic versions, and reproducible lockfiles | Resolver corpus and supply-chain threat tests |
| 077 | planned | Signed transparent registry, publishing, provenance, audit, yanking, mirroring, and offline operation | End-to-end registry and compromise simulations |
| 078 | planned | Unified workspace/build graph, incremental compilation, caching, remote execution, and hermetic builds | Reproducibility and incremental latency benchmarks |
| 079 | planned | IDE, REPL/notebook, profiler, linter, refactoring, visualization, and project templates | Cross-tool integration flows and usability study |
| 080 | planned | Standard-library governance, compatibility tiers, contribution process, and ecosystem quality gates | Versioned policy plus package certification tests |

## Passes 081–090 — performance and independence

The cross-cutting release contract in `docs/INDEPENDENCE.md` applies immediately, not only when
Pass 087 begins. Bootstrap dependencies must remain locked and audited, optional integrations must
remain removable, and new core subsystems must have a DISP-owned implementation path.
Universal support is classified as native, first-class connector, or compatibility/migration;
advertised targets require versioned, security-bounded, executable conformance evidence.

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 081 | planned | Stable optimizer pipeline: SSA, inlining, devirtualization, escape analysis, and copy elimination | Differential optimizer suite and benchmarks |
| 082 | planned | Profile-guided, link-time, whole-program, and cross-domain optimization | Reproducible workload gains without semantic drift |
| 083 | planned | Data/Intelligence/Page fusion, zero-copy transfer, placement, and serialization elimination | Full-stack traces and memory/time reductions |
| 084 | planned | Compiler throughput, parallelism, incremental queries, low-memory mode, and startup optimization | Compiler performance dashboard with regression gates |
| 085 | planned | Runtime footprint, startup, allocation, energy, binary size, and deterministic latency optimization | Comparable benchmark dashboard |
| 086 | planned | Fair public benchmark suite against C, C++, Rust, Python, Mojo, JavaScript, TypeScript, Go, Swift, and Julia | Reproducible source, environments, and raw results |
| 087 | planned | Begin bootstrap: formatter, package tooling, tests, and standard components implemented in DISP | Stage-1 dogfood artifacts built by the bootstrap compiler |
| 088 | planned | DISP frontend, semantic analysis, and intermediate representations implemented in DISP | Differential bootstrap compiler suite |
| 089 | planned | Full self-hosting compiler and independently reproducible trusted bootstrap chain | DISP-compiles-DISP fixed point and diverse double compilation |
| 090 | planned | Independent specification, conformance suite, alternate implementation path, and offline toolchain | Clean-room implementation proof and air-gapped build |

## Passes 091–100 — proof, hardening, and 1.0 release

| Pass | Status | Deliverable | Completion evidence |
|---:|---|---|---|
| 091 | planned | Six real applications: CLI, service, data platform, AI workload, web/mobile UI, and embedded/system artifact | Maintained production-shaped repositories and runbooks |
| 092 | planned | Compatibility freeze candidate: syntax, semantics, libraries, manifests, formats, and promised ABI | Version matrix and migration rehearsals |
| 093 | planned | Independent security review of compiler, runtime, package chain, Data, Intelligence, System, and Page | Published findings resolved or explicitly release-blocking |
| 094 | planned | Reliability campaign: fuzzing, soak, fault injection, crash recovery, hostile inputs, and resource exhaustion | Zero unresolved critical failures in defined campaign |
| 095 | planned | Cross-platform release engineering, installers, updates, uninstall, signatures, SBOM, provenance, and reproducibility | Verified release artifacts for every supported target |
| 096 | planned | Complete language reference, standard-library reference, tutorials, migration books, and architecture docs | Fresh-user documentation validation and executable examples |
| 097 | planned | Release-candidate performance, security, accessibility, compatibility, and correctness gates | Signed gate report containing raw evidence |
| 098 | planned | Ecosystem launch: registry mirror, editor integrations, CI images, containers, SDKs, and support policy | Publicly installable end-to-end workflows |
| 099 | planned | DISP 1.0 release candidate with no unresolved release-blocking defect | RC soak period and issue audit |
| 100 | planned | DISP 1.0 stable release and post-release verification | Signed reproducible artifacts, stable specification, conformance results, and launch report |

## Current next action

Complete the cross-cutting independence increment by proving the bootstrap compiler has no static
SQLite import while explicit legacy `Database` programs still load the connector lazily. Then advance
Pass 022 with a versioned generated C header and bidirectional ABI conformance. Richer GICv2/timer
work continues under active hardware/kernel Passes 026 and 028 rather than reopening Pass 021.
Active Passes 018–020 still
retain their privileged Linux sandbox evidence, audited keystore-provider integrations, hosted
provenance, sanitizer, and independent-review work; those obligations are not treated as complete.
