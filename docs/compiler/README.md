# DISP compiler

This crate is the current Rust bootstrap implementation of DISP. It is not DISP 1.0 yet.

Implemented executable subset:

- UTF-8/NFC lexing with Unicode XID identifiers, literals, comments, operators, and spans
- functions, typed parameters, explicit returns, calls, and recursion
- `let`, `var`, deterministic bounded `const` evaluation/folding, assignment, unary and binary expressions
- bounded parsed-AST `Meta.repeat` and hygienic `Meta.map` generation with deterministic traces
- `if`/`else`, `while`, range `for`, `loop`, `break`, and `continue`
- nominal structs with checked construction and field access
- enums with typed payloads, recursive pattern-matrix exhaustiveness/redundancy proofs, bounded typed `|` alternatives, struct destructuring, read-only typed guards, and source-ordered interpreter/native `match` dispatch
- coherent static traits with exact generic/capability method contracts, `Self.Name` associated-type projection, cycle-safe constraint selection, and universal-domain validation for generic `Copy` implementations
- `Option<T>`, `Result<T, E>`, exact typed `?` propagation, and reverse-order cleanup
- lexical scope/name resolution, mutability checks, and basic static types
- ownership, moves, Copy validation, safe references, NLL-style borrow checking, and drop facts
- typed/resolved HIR with stable semantic identities and source spans
- ownership-explicit MIR with basic blocks, calls, moves, copies, borrows, drop flags, and cleanup
- typed native threads with owned `spawn`, consuming `join`, Send-style boundary checks, and deterministic joining
- lazy `Future<T>` state machines and structured `Task<T>` values with `Async.spawn`, cooperative scheduling, consuming `await`, cancellation, and deterministic result cleanup
- deadline-aware async timer waits and lazy owned text/byte file futures with background native I/O, UTF-8 validation, cancellation, and shutdown draining
- compact Copy `IpAddress` values, strict IPv4/IPv6 parsing and canonical formatting, synchronous and lazy deadline-aware DNS resolution with sorted/deduplicated owned results, and validated `SocketAddress` construction from names or addresses
- lazy deadline-aware TCP connect/read/write futures, serialized per-direction stream operations, EOF and half-close semantics, typed `NetworkError`, explicit close, and reference-counted deterministic native socket cleanup
- owned `TcpListener` bind/local-port operations and lazy nonblocking `accept`/`accept_timeout` futures with responsive cancellation and reference-counted native listener state
- owned `UdpSocket` datagram I/O with local-port discovery, sender-address metadata, synchronous and lazy deadline-aware operations, explicit truncation errors, serialized directions, and cancellation-safe native cleanup
- owned `TlsStream` transport with lazy consuming handshakes, system trust and host-name verification, SNI, TLS 1.2 minimum, certificate revocation checks, bounded plaintext reads, deadline-aware encrypted I/O, and deterministic close/drop cleanup
- lazy safe HTTP GET/POST/PUT/PATCH/DELETE operations plus linear owned `HttpRequest` values for custom methods, headers and text/byte bodies, with non-Copy responses, typed errors, bounded input/output and connection reuse, non-replay redirects, verified system TLS, and deterministic cleanup
- nominal owned `Url` values with injection-safe path/query builders, plus bounded validated `Json` documents with safe object/array navigation, checked scalar extraction, structured construction, compile-time specialized nominal codecs, and native JSON HTTP integration without reflection or a dynamic parser dependency
- a lazily loaded legacy SQLite compatibility boundary with prepared binding and deterministic cleanup; the default compiler has no static SQLite import, the boundary is explicitly separate from the DISP-owned DataStore engine, and it is scheduled to become an isolated optional connector before 1.0
- nominal `data` schemas plus compiler-owned add/save/find/remove syntax lowered through typed HIR/MIR Data plans; `data memory` executes them directly on a DISP-owned native row store, while `data open` persists the same plans in fixed-size DISP pages with changed-page WAL recovery, exclusive process locking, v1 migration, safe primary keys, limits, deterministic cleanup, and no SQL surface
- linear `ProcessCommand` values and owned streaming `ChildProcess` handles with shell-free arguments, working directories, validated environment overrides, incremental bounded text/byte I/O, polling, deadlines, kill/reap, typed failures, and deterministic child/thread cleanup
- explicitly shared `Mutex<T>`, owning lock guards, and checked sequentially consistent `AtomicInt`
- defined `extern C` declarations, fixed C ABI aliases, checked `CString`, borrowed `CStr`, native library linking, deterministic C11/C++17 `disp header` ABI-v1 contracts, explicit nested `export C struct` records with field/size/alignment proofs, real by-value C-host round trips, and verified Windows x86-64/i686 aggregate calling sequences, transitively verified pure `export C fn` shared libraries with helpers/recursion/contained failures, transactional heap rollback, and typed reverse-order `CRegistration` rollback, generated C-to-DISP callback types, typed context-free DISP-to-C `CFunction` values, checked `CExport.callback` handles, atomic resource-owning Send-compatible context trampolines, linear `CRegistration` ownership with reusable borrowed invocation and quiesce-before-capture-drop-before-release cleanup, and explicit thread-local C-host attachment with concurrent-entry evidence
- owned aligned `Memory`, bounds-checked byte operations, provenance-carrying checked pointer views, and separate thin FFI pointers
- deterministic file modules with `pub`, selective imports, aliases, re-exports, cycle checks, and source-aware diagnostics
- strict edition/feature-pinned `DISP.toml` packages, directory builds, `disp new`, and idempotent `disp migrate`/`disp migrate --check`
- bounded transitive local dependencies, SHA-256 source integrity, deterministic `DISP.lock`, `disp lock`, and `disp tree`
- vetted bootstrap cryptographic foundations with OS-only entropy, zeroizing/redacted secrets,
  constant-time equality, SHA-256, HMAC-SHA-256, HKDF-SHA-256, auto-nonce AES-256-GCM-SIV,
  strict Ed25519, fixed-policy Argon2id, published known-answer tests, and source-level
  capability-checked `Crypto.random_bytes` plus opaque, redacted, zeroizing
  `Crypto.random_secret`/`SecretBytes`, consuming secret import, and provider-backed SHA-256/HMAC
  plus zeroizing HKDF-SHA-256, opaque AES-256-GCM-SIV envelopes, and opaque non-exportable
  Ed25519 signing keys, stable public-key IDs, identity pinning, activation/expiry/revocation
  policy, canonical ciphertext/key/signature records, and
  fixed-policy Argon2id password hashing with interpreter/native parity
- versioned `disp-crypto-native` companion ABI for exact primitives unavailable from platform
  providers, including authenticated-before-output RustCrypto AES-256-GCM-SIV and strict Ed25519
- CFG predecessor/successor, reachability, reverse-postorder, and back-edge analysis
- checked integer arithmetic and an explicit `disp interpret` semantic oracle
- deterministic monomorphization, target-aware layouts, ABI classification, and native MIR lowering
- `disp check`, `disp build`, native `disp run`, `disp interpret`, syntax-validating idempotent `disp fmt`/`disp fmt --check`, stable `--diagnostic-format=json` output, and deterministic `check --dump-effects`, `--dump-constants`, and `--dump-expansions` manifests; native builds use a content-addressed cache that is invalidated by compiler, option, entry-source, or imported-source changes

The numeric runtime implements distinct checked signed and unsigned widths, `int`/`uint`, `f32`/`f64`, safe widening, checked explicit conversions, and wrapping/saturating integer operations. Generic functions and ADTs use substitution-based inference, and traits use coherent static dispatch with exact method contracts, complete associated-type definitions, order-independent constraint proofs, and cycle-safe selection.

The initial native backend supports Windows and Linux x86-64. It lowers validated, monomorphized MIR to deterministic C as a temporary backend IR, asks GCC (discovered through `PATH`) for a native object, and links a standalone executable. This is intentionally described as a C/object backend, not LLVM. Calling-convention classification and concrete layouts remain separate from code generation so a direct machine-code backend can replace the temporary lowering without touching the frontend.

From the repository root, enter the compiler crate and run:

```text
cd compiler
cargo run -- run examples/control_flow.disp
cargo run -- build examples/control_flow.disp
cargo run -- build --release examples/control_flow.disp
cargo run -- build --sanitize examples/control_flow.disp
cargo run -- build --emit-c examples/control_flow.disp
cargo run -- interpret examples/control_flow.disp
cargo run -- check examples/control_flow.disp
cargo run -- --diagnostic-format=json check examples/control_flow.disp
cargo run -- check --dump-effects examples/async_io.disp
cargo run -- check --dump-constants examples/control_flow.disp
cargo run -- check --dump-expansions examples/control_flow.disp
cargo run -- fmt examples/control_flow.disp
cargo run -- fmt --check examples/package
cargo run -- check --dump-hir examples/control_flow.disp
cargo run -- check --dump-mir examples/control_flow.disp
cargo run -- run examples/c_interop.disp
cargo run -- run examples/system_memory.disp
cargo run -- run examples/modules/main.disp
cargo run -- run examples/package
cargo run -- lock examples/dependencies/app
cargo run -- run examples/dependencies/app
cargo run -- tree examples/dependencies/app
cargo run -- new hello
cargo run -- migrate hello
cargo run -- migrate --check hello
```

A directory project contains a strict `DISP.toml` and an entry source under `src/`.
Current manifests cover package identity, entry selection, and explicitly declared local
path dependencies. Applications require an exact generated `DISP.lock` before dependency
code is compiled. Registry, Git, remote, version-range, and feature dependency forms are
not implemented and are rejected instead of silently receiving guessed semantics.

Fuzz targets for the lexer and complete frontend live under `fuzz/` and can be run with `cargo fuzz run lexer` and `cargo fuzz run frontend` when `cargo-fuzz` and its required Rust toolchain are installed.
