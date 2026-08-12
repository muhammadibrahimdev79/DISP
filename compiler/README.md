# DISP compiler

This crate is the current Rust bootstrap implementation of DISP. It is not DISP 1.0 yet.

Implemented executable subset:

- UTF-8/NFC lexing with Unicode XID identifiers, literals, comments, operators, and spans
- functions, typed parameters, explicit returns, calls, and recursion
- `let`, `var`, `const`, assignment, unary and binary expressions
- `if`/`else`, `while`, range `for`, `loop`, `break`, and `continue`
- nominal structs with checked construction and field access
- enums with typed payloads, ordered patterns, and exhaustive `match`
- `Option<T>`, `Result<T, E>`, and type-checked `?` propagation
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
- explicitly shared `Mutex<T>`, owning lock guards, and checked sequentially consistent `AtomicInt`
- defined `extern C` declarations, fixed C ABI aliases, checked `CString`, borrowed `CStr`, and native library linking
- owned aligned `Memory`, bounds-checked byte operations, raw pointer views, and explicit unsafe pointer arithmetic/read/write
- deterministic file modules with `pub`, selective imports, aliases, re-exports, cycle checks, and source-aware diagnostics
- strict local `DISP.toml` packages, directory builds, and `disp new`
- bounded transitive local dependencies, SHA-256 source integrity, deterministic `DISP.lock`, `disp lock`, and `disp tree`
- CFG predecessor/successor, reachability, reverse-postorder, and back-edge analysis
- checked integer arithmetic and an explicit `disp interpret` semantic oracle
- deterministic monomorphization, target-aware layouts, ABI classification, and native MIR lowering
- `disp check`, `disp build`, native `disp run`, and `disp interpret`

The numeric runtime implements distinct checked signed and unsigned widths, `int`/`uint`, `f32`/`f64`, safe widening, checked explicit conversions, and wrapping/saturating integer operations. Generic functions and ADTs use substitution-based inference, and traits use coherent static dispatch with associated-type completeness checking.

The initial native backend supports Windows x86-64. It lowers validated, monomorphized MIR to deterministic C as a temporary backend IR, asks GCC (discovered through `PATH`) for a real PE/COFF object, and links a standalone `.exe`. This is intentionally described as a C/object backend, not LLVM. Calling-convention classification and concrete layouts remain separate from code generation so a direct machine-code backend can replace the temporary lowering without touching the frontend.

Run:

```text
cargo run -- run examples/control_flow.disp
cargo run -- build examples/control_flow.disp
cargo run -- build --release examples/control_flow.disp
cargo run -- build --emit-c examples/control_flow.disp
cargo run -- interpret examples/control_flow.disp
cargo run -- check examples/control_flow.disp
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
```

A directory project contains a strict `DISP.toml` and an entry source under `src/`.
Current manifests cover package identity, entry selection, and explicitly declared local
path dependencies. Applications require an exact generated `DISP.lock` before dependency
code is compiled. Registry, Git, remote, version-range, and feature dependency forms are
not implemented and are rejected instead of silently receiving guessed semantics.

Fuzz targets for the lexer and complete frontend live under `fuzz/` and can be run with `cargo fuzz run lexer` and `cargo fuzz run frontend` when `cargo-fuzz` and its required Rust toolchain are installed.
