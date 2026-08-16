# DISP 0.1.0 Developer Preview

DISP 0.1 is the first public developer preview of the Data · Intelligence · System ·
Page language. It is intended for experimentation, examples, compiler feedback, and
early native programs. It is not a stable production release.

## Included

- One Windows x64 installer containing `disp.exe` and its private native toolchain.
- Direct `disp file.disp`, `disp run`, `disp build`, `disp check`, and
  `disp interpret` workflows.
- Stable stage-level diagnostic codes and the `disp.diagnostic.v1` JSON envelope for tooling.
- Inferred and explicit `uses` capability contracts plus deterministic effect manifests.
- Bounded deterministic `const` evaluation with pre-HIR folding, plus structured hygienic
  `Meta.repeat`/`Meta.map` generation and inspectable expansion traces.
- Coherent generic traits with exact method and capability contracts, complete `Self.Name`
  associated types, order-independent constraint proofs, and fail-closed cycle detection.
- Functions, recursion, lexical scopes, mutable and immutable bindings, checked
  numerics, control flow, structs, enums, exhaustive matching, `Option`, `Result`, and
  `?` propagation.
- Ownership, moves, borrowing, non-lexical loan checking, dynamic indexed/subslice
  places, deterministic drops, slices, borrowed UTF-8 `str`, and generic collections.
- HIR, MIR, CFG validation, concrete native layouts, monomorphization, C ABI/FFI,
  native executable generation, checked `export C fn` shared libraries with transactional
  heap rollback plus typed reverse-order `CRegistration` rollback, and explicit nested `export C struct` packet records whose public C11/C++17
  headers prove every field offset, size, and alignment across inspected Windows x86-64/i686
  aggregate calling sequences.
- Files, paths, time, processes, threads, mutexes, atomics, networking, TLS, HTTP, URL,
  JSON, and SQLite compatibility foundations.
- OS process-tree containment for runtime children and compiler tools, plus the exact bounded
  `disp.component.v1` transport for resource-contained out-of-process foreign components.
- Nominal DISP Data schemas and compiler-owned add/save/find/remove plans. `data
  memory` executes on the first DISP-owned native row engine without translating plans
  to SQL.

## Important preview limits

- Language and library compatibility may change before DISP 1.0.
- This installer targets 64-bit Windows. Other operating systems and architectures are
  not included in this artifact.
- Intelligence and Page are early compiler domains, not complete AI or application UI
  platforms yet.
- Durable `data open` now uses DISP's native fixed-page storage and write-ahead recovery
  format. Advanced indexes, joins, query optimization, replication, and distributed
  storage remain under development. SQLite is available only through the separate legacy
  compatibility `Database` API; it is not the DISP engine and must become an optional isolated
  connector outside the core toolchain before 1.0.
- PostgreSQL is a first-class future interoperability and remote-deployment target, but remains an
  optional typed connector. The compiler, runtime, native Data engine, recovery path, and offline
  applications will not require a PostgreSQL installation.
- The bootstrap compiler no longer statically imports SQLite. Its legacy `Database` compatibility
  path resolves the system library only after explicit construction; native DataStore programs remain
  independent, while complete out-of-process connector isolation remains required before 1.0.
- `disp header` now emits deterministic, transactionally installed C import headers for ABI version 1.
  Exact scalar, pointer-width, C-string, and recursively qualified raw-pointer declarations compile
  under strict C11 and C++17; private DISP runtime layouts fail closed instead of leaking into C.
- The registry-backed package manager, debugger, language server, self-hosting compiler,
  freestanding OS target, GPU toolchain, and full Page renderer are not complete.
- General user-defined macros are not included; the current `Meta` surface is deliberately
  compiler-owned, authority-free, and resource-bounded.
- Pass 019 cryptography is active: the bootstrap runtime has OS-only entropy, zeroizing secret
  storage, constant-time secret/MAC comparison, SHA-256, HMAC-SHA-256, HKDF-SHA-256,
  auto-nonce AES-256-GCM-SIV, strict Ed25519, and resource-capped Argon2id with published vectors.
  The current full Windows matrix passes 525/525 assertions across 60 harnesses.
  Stable source keystore APIs, audited platform hardware-provider integrations, broader protocol
  construction, and external review remain.
- `Crypto.random_bytes` is the first stable source/native Pass 019 operation. It returns typed
  `CryptoError`, requires or infers `Random`, enforces the same one-megabyte bound in both engines,
  and uses Windows BCrypt or Linux `getrandom` without an insecure fallback.
- `Crypto.random_secret` returns the new opaque `SecretBytes` source type. Secrets are non-Copy,
  stay off spawned-thread boundaries, reject extraction/indexing/serialization/direct formatting
  and ordinary equality, support only length inspection and explicit constant-time comparison,
  redact nested displays, and zeroize native storage before release.
- `Crypto.import_secret` consumes public bytes into opaque storage and wipes rejected imports.
  Pure `Crypto.sha256`, `Crypto.hmac_sha256`, and `Crypto.hmac_sha256_verify` borrow bounded inputs,
  pass published known-answer vectors in both engines, and use Windows CNG or Linux AF_ALG in native
  programs rather than a handwritten runtime hash.
- `Crypto.hkdf_sha256` provides bounded Pure RFC 5869 derivation into opaque secrets. Both engines
  pass the published case-one vector; native extract/expand uses the operating-system HMAC provider
  and wipes PRK, block, assembled message, and rejected partial output.
- A minimal versioned `disp-crypto-native` companion DLL now provides the exact AES-256-GCM-SIV
  primitive that Windows CNG does not. Its caller-owned-buffer ABI contains panics, validates all
  pointer/length/capacity inputs, generates nonces internally, and never exposes unauthenticated
  plaintext. DISP source now exposes opaque `AeadEnvelope` seal/open operations with
  interpreter/native parity. Native builds content-fingerprint, link, and stage the exact bundled
  companion only when companion-backed cryptography is used.
- Opaque source-level `Ed25519SigningKey` values now support OS-backed key generation, public-key
  derivation, signing, and strict verification with interpreter/native parity. Secret keys cannot
  be printed, compared, serialized, copied, or exported and are zeroized before release.
- Fixed-policy source-level Argon2id password hashing and verification now use fresh OS salts,
  bounded opaque passwords, canonical PHC validation before expensive work, and the versioned
  native companion. Wrong passwords remain normal `Ok(false)` results.
- `AeadEnvelope` now has a canonical version-one portable encoding with explicit algorithm,
  nonce/tag, and ciphertext-length fields. Strict decoding rejects ambiguous, truncated,
  oversized, unknown-version, and trailing-byte encodings before decryption.
- Ed25519 public keys and signatures now have separate canonical version-one records with strict
  kind and length discrimination, preventing raw-key/signature format confusion across storage and
  protocol boundaries.
- Valid non-weak Ed25519 public keys now have deterministic domain-separated 32-byte key IDs for
  durable audit, rotation, and revocation references without secret-key exposure.
- Identity-bound Ed25519 verification rejects otherwise valid signatures from keys other than the
  explicitly approved stable key ID, providing a safe primitive for deployment pinning and key
  rotation/revocation policy.
- Deterministic Ed25519 lifecycle verification now enforces inclusive activation/expiry windows
  and explicit revocation against a caller-supplied audited time, with interpreter/native parity.
- A fail-closed external Ed25519 key-provider boundary now keeps hardware/private key material out
  of process. Opaque handles are non-cloning, redacted, and zeroizing; provider content and expected
  key identity are pinned; exact bounded component frames expose only public-key/sign operations;
  and DISP verifies every returned signature before releasing it. A provider-side SDK serves one
  exact frame and exposes callbacks containing only opaque handles and bounded public messages.
  Platform TPM/Secure Enclave/PKCS#11 integrations and their audited device grants remain future
  work.
- Pass 020 has begun with a pinned, daily RustSec dependency gate. It scans the exact compiler and
  cryptographic-companion lockfile with no ignored advisories and fails for low-or-higher
  vulnerabilities, informational warnings, yanked crates, stale advisory data, or scanner errors.
  The first live audit on August 16, 2026 checked 115 dependencies against 1,216 advisories with no
  findings.
- Pinned nightly libFuzzer CI now continuously attacks the lexer, full frontend, canonical AEAD and
  Ed25519 records, component transport, and keystore request/response frames with sanitizer
  instrumentation, finite per-input/campaign deadlines, and committed structured protocol tokens.
  The separately locked 49-dependency crypto-companion and 119-dependency fuzz graphs are also
  included in the RustSec gate.
- Release CI now uses pinned cargo-auditable 0.7.4 to embed the exact Rust package inventory into
  the compiler and native cryptographic companion, then scans both produced artifacts rather than
  relying only on Cargo.lock. Non-Rust system-library SBOM coverage remains unfinished.
- The repository now publishes private vulnerability reporting, severity response targets,
  coordinated-disclosure steps, and explicit security release blockers. A versioned threat model
  records assets, adversaries, trust boundaries, controls, and residual risks. CI confines all Rust
  unsafe code to four documented boundary files and rejects new unsafe locations or suppression.
- A date-pinned Linux nightly job now compiles and executes high-risk compiler, cryptography, native
  ABI, hostile-frame, and governance regressions under Rust AddressSanitizer with leak detection;
  missing instrumentation or sanitizer findings fail the job. First hosted execution is pending.
- A repository-owned deterministic CycloneDX 1.6 generator records all three locked Cargo graphs.
  Linux release CI additionally resolves the actual compiler/companion shared libraries with `ldd`,
  records distribution versions and SHA-256 file hashes, rejects unresolved/dangling/malformed
  inventory, and publishes verified SBOM artifacts. Rust-only generation locally verified 115, 49,
  and 119 components; first hosted native inventory remains pending.
- The SBOM generator now parses Windows PE normal and delay-load import tables without executing the
  artifact, resolves concrete DLLs from controlled loader locations, hashes and versions them, and
  represents API-set contracts explicitly. Local Windows release inspection verified 17 compiler
  and 7 companion native imports. Windows CI now audits embedded provenance and publishes both
  verified native SBOMs; first hosted execution remains pending.
- macOS release CI now builds and audits compiler/companion provenance, parses thin or universal
  Mach-O dylib/rpath commands without executing artifacts, resolves and hashes concrete libraries,
  represents dyld shared-cache system components explicitly, and publishes verified SBOMs. Two
  cross-platform synthetic parser tests pass; first hosted macOS artifact inventory remains pending.
- Linux, Windows, and macOS release-security jobs now create GitHub OIDC/Sigstore SLSA provenance
  over compiler, companion, and SBOM subjects, plus separate CycloneDX SBOM attestations bound to
  each executable/library. Write authority is job-scoped and unavailable to pull-request, fuzz, or
  sanitizer jobs. First hosted signed attestations remain pending.
- Pass 021 completed with a direct freestanding compiler path. `disp build --freestanding` validates
  a strict `fn main()` and emits a deterministic, bootable x86 BIOS disk image
  without invoking C, an assembler, a linker, an operating system runtime, libc, or an allocator.
  Its direct machine-code backend now supports fixed-memory scoped `u16` variables, checked integer
  arithmetic, comparisons, boolean short-circuiting, mutation, `if`/`while`, and allocation-free
  decimal output. Invalid arithmetic enters a deterministic failure path. Unsupported hosted
  facilities fail closed, image replacement is staged and synchronized, and a Linux QEMU gate is
  configured to verify computed output. Later Pass 021 increments delivered additional numeric
  types, protected-mode address spaces, x86-64, and AArch64.
- Freestanding programs are no longer confined to one sector. Programs over 510 bytes receive a
  deterministic signed loader plus an origin-relocated, padded second stage. The loader preserves
  the BIOS drive, performs a bounded extended disk read into `0x7e00`, fails visibly on I/O error,
  and enforces the real-mode address ceiling. CI uses a genuine multi-sector computation fixture;
  protected-mode images remain future work.
- Exact-width freestanding execution now includes aligned `u32`, `i32`, and `bool` locals alongside
  `u16`. Directly emitted instructions distinguish signed and unsigned comparisons and reject carry,
  borrow, high-half multiplication overflow, signed overflow, division by zero, and `i32::MIN / -1`
  before invalid results escape. Allocation-free routines print full-width signed/unsigned decimals
  and booleans. The multi-sector QEMU fixture requires exact computed output across all supported types;
  other widths remain future work.
- Allocation-free freestanding user functions now accept and return scalar exact-width values or
  `Unit`. Forward labels and fixed parameter slots are deterministic; nested-call arguments evaluate
  left-to-right on the machine stack before reverse slot commitment, and results return in the exact
  accumulator width. Calls to `main` are rejected before image emission.
- Direct and mutual freestanding recursion now use pre-inventoried, stack-snapshotted fixed frames.
  Each call saves every callee parameter/local slot and restores it after preserving the return
  accumulator. A generated stack-floor guard includes frame, argument, return-address, and expression
  reserve costs; exhaustion prints a deterministic diagnostic and halts before local-memory overlap.
  The QEMU fixture now exercises a recursive factorial alongside nested and forward calls.
- Exact `u8` freestanding values now occupy one byte in fixed memory, zero-extend on load, use
  width-correct two-byte real-mode stack transport, and participate in guarded recursive frames.
  Byte addition, subtraction, multiplication, division, and remainder fail closed on invalid
  results, while calls, returns, comparisons, and decimal output preserve the exact unsigned value.
  The multi-sector QEMU oracle now includes checked byte computation.
- `disp build --freestanding32` now directly emits the first genuine 32-bit protected-mode DISP boot
  image. The signed sector enables A20, loads flat 4 GiB code/data GDT descriptors, sets `CR0.PE`,
  far-transfers into a 32-bit code segment, reloads all data segments, and establishes a 32-bit stack.
  Constant ASCII output bypasses unavailable BIOS services and writes directly to VGA text memory
  while mirroring exact output to port `0xe9`. Unsupported source and oversized sectors fail closed;
  Linux CI boots the image in QEMU and compares its complete output. Later Pass 021 increments
  delivered the full bounded protected scalar profile.
- The protected32 encoder retains a minimal one-sector form when a complete payload fits and otherwise
  reuses the bounded EDD loader to load a sector-padded stage at `0x7e00`. Mandatory IDT/paging
  infrastructure means current artifacts take the staged path. Code generation relocates the GDTR
  operand, GDT base, and protected far jump for the stage origin, rejects more than 64 sectors, and
  remains byte-deterministic. The protected QEMU fixture is deliberately multi-sector so CI covers
  disk loading and the 32-bit transition together. Remaining exact widths and functions remain active.
- Protected32 now lowers checked `u32` computation and booleans rather than constant output alone.
  Up to 128 deterministic four-byte locals begin at `0x100000`; the bootstrap therefore verifies A20
  with a reversible alias probe and restores both test bytes before transition. Bindings, mutation,
  checked arithmetic, comparisons, boolean short circuiting, `if`/`else`, `while`, indefinite loops,
  lexical break/continue, and runtime decimal/boolean output execute as direct 32-bit instructions.
  Carry, borrow, multiplication high-half overflow, and zero divisors enter a defined diagnostic halt.
  The protected QEMU oracle computes and prints `55` and `true` from high-memory locals.
- Protected32 exact types now include compact one-byte `u8`, aligned two-byte `u16`, native `u32`,
  signed `i32`, and booleans in a bounded high-memory arena. Narrow loads zero-extend, narrow stores
  touch only their declared bytes, and all arithmetic remains checked. Signed lowering distinguishes
  overflow and signed comparisons, validates multiplication sign extension and `i32::MIN / -1`, and
  prints the complete signed domain allocation-free. The QEMU oracle now covers every protected
  scalar kind, both signs, compact maximums, values above two billion, and `i32::MIN`.
- Protected32 helper functions now accept and return every current exact scalar kind or `Unit`.
  Forward/nested calls and direct/mutual recursion use compiler-preinventoried high-memory frames:
  each call guards `ESP`, snapshots every callee slot in machine words, evaluates arguments
  left-to-right, commits in reverse, preserves the return accumulator, and restores the prior frame.
  The stack cannot cross `0x80000`; exhaustion prints a deterministic diagnostic and halts. The
  protected QEMU fixture now exercises nested calls, recursive locals, and mutual boolean recursion.
- Protected32 now includes bounded fixed arrays with exact-width checked indexing, the distinct
  `DeviceIo` authority and explicitly unsafe byte-port I/O, a 32-vector exception IDT, and active
  CR0.WP paging with an unmapped null page, read-only stage, and 4 MiB ceiling.
- `disp build --freestanding64` now emits an independent x86-64 long-mode BIOS image. It verifies
  A20 and CPUID long-mode support, constructs a zeroed four-level 4 KiB hierarchy with an unmapped
  null page and read-only stage, enables PAE/EFER.LME/PG/WP in architectural order, far-transfers
  through a 64-bit GDT descriptor, installs a 64-bit IDT, and writes output directly to VGA and the
  debug port. Linux CI boots the artifact and checks exact output.
- The x86-64 profile now executes checked `u8`, `u16`, `u32`, `i32`, and `bool` computation directly
  in long mode. It provides a bounded writable local page, explicit byte/word/dword absolute memory
  operands, balanced 64-bit expression stacks, checked arithmetic and division, comparisons,
  short-circuit Boolean logic, structured control flow, and typed output. A second QEMU fixture
  verifies the complete scalar program output byte-for-byte.
- X86-64 scalar functions now support forward and nested calls, exact parameters and returns,
  recursion, and mutual recursion. Calls guard `RSP`, snapshot every callee slot as a 64-bit word,
  evaluate arguments left-to-right, restore frames in reverse, and preserve scalar returns. Dedicated
  QEMU fixtures verify recursive results and deterministic `x86-64 stack limit exceeded` handling.
- X86-64 fixed arrays now use compact exact-width storage, once-evaluated checked indices, and direct
  or compound element mutation. Array elements participate in recursive frame preservation. Normal
  recursive-array and deliberate `x86-64 index out of bounds` QEMU fixtures are release gates.
- X86-64 now carries the distinct `DeviceIo` effect through functions and permits direct `in al,dx`
  and `out dx,al` only inside explicit `unsafe uses DeviceIo` regions. An authorized debug-port QEMU
  fixture verifies the exact instruction path and output.
- X86-64 paging now requires CPUID execute-disable support, enables `EFER.NXE`, marks all present
  leaves NX by default, and clears NX only for the bounded read-only loader/stage envelope. Stack,
  VGA, page-table, IDT, and local-arena pages are writable only where required and never executable.
- X86-64 now routes invalid opcode, general protection, and page fault through distinct DPL0
  interrupt gates. Each handler restores a known stack/output state, emits a stable fault-specific
  diagnostic, and halts without returning; all other first-32-vector faults retain the common
  fail-closed handler.
- X86-64 now expands its IDT to cover vectors 32–47, remaps both legacy PICs away from CPU
  exceptions, and masks every IRQ before user code. Any unexpected pending IRQ is acknowledged by a
  known-state non-returning diagnostic handler; the profile deliberately keeps `IF=0`.
- `Time.ticks() -> u32` now carries distinct `Timer` authority and advances in fixed 10 millisecond
  units across hosted engines. An explicit x86-64 `uses Timer` contract installs one bounded PIT
  IRQ0 service; all other IRQs remain masked, and a QEMU fixture must observe a real tick before it
  prints success.
- `disp build --freestanding-aarch64` introduces DISP's first direct non-x86 artifact: a deterministic
  Arm64 Image for versioned QEMU `virt-8.2`. Its baseline directly streams bounded UTF-8 through
  PL011 and halts in `wfi`; Linux CI boots it on `cortex-a53` and compares exact serial bytes without
  an assembler, linker, C compiler, OS, or runtime.
- The direct AArch64 target now lowers checked `u32` and `bool` locals, assignments, unsigned
  arithmetic/comparisons, short-circuit logic, and structured `if`/`while`/`loop` control. Static
  position-independent slots stay bounded, arithmetic faults use one deterministic non-returning
  diagnostic, and QEMU gates both successful computation and deliberate overflow.
- AArch64 now has exact `u8`, `u16`, and `i32` computation with true compact storage, signed
  overflow/division guards, signed comparisons, and direct decimal/boolean scalar printing. The
  formatter uses one bounded image-local buffer and has exact-output QEMU evidence across compact
  maxima, negative values, `i32::MIN`, booleans, and intentional signed overflow.
- AArch64 plain functions now support exact-scalar parameters/results, `Unit` calls, nested calls,
  and recursion. The entry installs a 16 KiB image-owned aligned stack; guarded 16-byte pushes isolate
  callee frames and expression temporaries, while exhaustion emits one stack-independent diagnostic.
  Emulator and QEMU fixtures verify recursive factorial, mixed-width arguments, restoration, and
  deliberate recursion exhaustion.
- AArch64 local fixed arrays now provide contiguous exact-width `u8`, `u16`, `u32`, `i32`, and
  `bool` storage, dynamic checked indexing, and checked element compound assignment. Bounds are
  rejected before address calculation, memory access, or assignment right-hand-side effects;
  recursive frame snapshots preserve every element, and an exact-output fixture verifies the
  non-returning bounds diagnostic.
- AArch64 startup now masks asynchronous exceptions, detects EL1 versus EL2, installs the matching
  VBAR register, and exposes a complete image-aligned 16-entry vector table. Synchronous, IRQ, FIQ,
  and system-error classes have distinct stack-independent, non-returning diagnostics. Linux QEMU
  boots the unchanged fixture and a copied image whose fixed NOP checkpoint is replaced with `BRK`,
  proving real synchronous exception delivery without adding a source-level trap backdoor.
- AArch64 now emits five sparse 4 KiB stage-1 translation tables for either EL1 or EL2. Only the
  image and one PL011 page are mapped: code/vectors are read-only executable, data/stack are writable
  execute-never, page tables are read-only execute-never, and UART is device-memory execute-never.
  WXN remains enabled. A copied QEMU fixture replaces one post-MMU NOP with a store into code and
  must receive the exact non-returning memory-protection diagnostic.
- AArch64 boot now authenticates hardware descriptions before any MMIO. A 2 MiB-bounded FDT parser
  checks header/block/string/token ranges, a depth-64 direct-child schema, exact root address/size
  cells, unique `arm,pl011`, and RAM containment of the complete image. It then patches only the
  discovered UART's sparse Device-NX path before enabling translation; generated artifacts no longer
  embed `0x09000000`. Invalid or ambiguous input halts silently, while checked-in DTB/QEMU and
  alternate-address instruction-emulation fixtures cover successful discovery and fail-closed paths.
- AArch64 now exposes exact `u8`/`u16`/`u32` volatile `Mmio` reads and writes only inside explicit
  `unsafe uses DeviceIo`. Addresses are `u16` offsets relative to the authenticated PL011 page;
  width-specific end bounds and natural alignment are checked before access or write-value effects.
  Direct A64 loads/stores are surrounded by `DMB OSH`; invalid offsets produce one deterministic
  device-access diagnostic. Alternate-address emulation and Linux QEMU fixtures cover real status
  reads, data writes, and fail-before-access rejection.
- Freestanding structured control flow now includes indefinite `loop`, `break`, and `continue`.
  Direct branches bind only to the innermost lexical loop: `while` continues re-check conditions,
  indefinite-loop continues return to the body head, and breaks target the unique post-loop point.
  The QEMU fixture exercises nested loop scopes without exposing arbitrary safe-code jumps.
- The component host guarantees bounded protocol/process containment. Linux components additionally
  deny network syscalls. Windows components run in path-separated AppContainers with no capability
  SIDs and opt out of `ALL_APPLICATION_PACKAGES`, producing LPAC children. Live probes verify Low
  integrity, restricted privileges, absence of ambient package membership, host-file read/write
  denial, network unavailability, and UI restrictions. Audited user-selected resource grants are
  not yet exposed. Trusted in-process C calls remain unsafe.
- The installer is not yet code-signed, so Windows may show an unknown-publisher
  warning. Verify the published SHA-256 before running it.

## First program

Create `hello.disp`:

```disp
fn main() {
    print("Hello from DISP 0.1")
}
```

Run it from a new terminal:

```text
disp hello.disp
```
