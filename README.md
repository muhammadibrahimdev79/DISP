# DISP

**DISP — Data Intelligence System Page** is an evolving programming language
designed around a simple goal: make ordinary programming easy, safe by default,
and capable of high-performance native execution.

The repository contains the Rust compiler, native backend, interpreter,
examples, fuzz targets, test suites, and the evolving design material.

## Current implementation

The current compiler includes static typing, ownership and borrowing, native
code generation, first-class functions and ownership-safe closures, algebraic
data types, generics and traits, strings, slices,
lists, maps, sets, iteration, paths, filesystem operations, and time
foundations, UTF-8 command-line arguments, explicit environment reads, injection-safe direct
process execution plus linear configured commands with bounded input/output and timeouts, native concurrency, checked C
interoperability, explicit system-memory
control, deterministic multi-file modules, content-locked local package dependencies,
and lazy owned `Future<T>` values compiled to resumable native `async fn` state machines.
`await` is available inside async functions, `Async.yield()` provides cooperative suspension,
and `Async.spawn(future)` creates a structured, cooperatively scheduled `Task<T>` that must be
awaited or is cancelled with deterministic cleanup before its async scope exits. The separate
`spawn function()` syntax creates an operating-system thread. An `async fn main()` is driven
automatically. `Async.sleep`, `Async.read_text`, `Async.read_bytes`, `Async.write_text`, and
`Async.write_bytes` are lazy owned futures; synchronous `Time.*` and `File.*` operations remain
available. `IpAddress` provides compact Copy IPv4/IPv6 values with canonical formatting,
`Dns.resolve` and lazy deadline-aware `Async.resolve` provide sorted, deduplicated owned address
lists, and `SocketAddress` validates owned host/IP and port pairs. `Async.connect` produces an owned
`TcpStream` with typed `NetworkError` failures, synchronous or lazy nonblocking byte
reads/writes, operation deadlines, explicit read/write half-close, explicit close, and automatic
drop cleanup. Asynchronous writes copy their byte input into the future, so later caller mutation
cannot change pending network output. `TcpListener.bind` provides owned server sockets with lazy readiness-polled
`accept` futures, optional deadlines, local-port discovery, and cancellation-safe listener
cleanup. `UdpSocket` adds owned datagram sockets with synchronous or lazy deadline-aware
send/receive operations, sender-address metadata, explicit truncation errors, zero-length
datagrams, and cancellation-safe reference-counted native state. `Tls.connect` and
`Tls.connect_timeout` consume a `TcpStream` into a lazy handshake future and produce an owned
`TlsStream` using the operating-system trust store, verified host names, SNI, certificate
revocation checks, strong cryptography, and TLS 1.2 or newer. TLS streams provide synchronous and
lazy deadline-aware encrypted reads and writes, explicit close, and deterministic authenticated
shutdown/drop cleanup. Safe lazy HTTP/HTTPS GET/POST/PUT/PATCH/DELETE operations and linear owned
custom requests provide typed responses, snapshotted headers and bodies, safe TLS defaults,
strict validation, bounded redirects and input/output, non-replay protection, typed failures, and
bounded connection reuse with deterministic cleanup. Cancelling an unpolled I/O future has no side effects. Once native I/O has started,
cancellation discards its result but lets the operating-system operation finish, and shutdown
drains that work so owned resources are released deterministically. The implementation
also includes nominal structured HTTP URLs with injection-safe path/query builders and bounded
validated JSON documents with safe navigation, checked scalar extraction, array/object
construction, automatic type-safe struct/enum conversion, native HTTP body integration,
deterministic cleanup, and matching interpreter
semantics. An owned SQLite `Database` foundation provides prepared parameter binding, bounded
JSON-object query rows, explicit transactions, typed failures, and deterministic rollback/close
in both native execution and the interpreter. First-class nominal `data` schemas and
compiler-owned `data add`, `data save`, `data find`, and guarded `data remove` expressions now
lower through typed logical Data plans in HIR/MIR. `data memory` uses DISP's own typed row store
and evaluates those plans directly in both native binaries and the interpreter; it does not
translate them to SQL. `DataStore` is nominally separate from the compatibility `Database` type,
so raw SQL methods cannot leak into DISP Data code. Durable `data open` currently retains the
SQLite physical provider while the DISP-native page and recovery format is being built.
The implementation
remains under active development and should not
yet be treated as a stable production language.

The compiler and its tests are the authority for currently implemented
behavior. See the [documentation index](docs/README.md) for verified compiler
documentation and clearly separated design drafts.

## Build and test

The compiler is a Rust crate in `compiler/`:

```sh
cd compiler
cargo build
cargo test -- --test-threads=1
```

Run a DISP example through native compilation:

```sh
cargo run -- run examples/easy_disp.disp
```

Pass arguments after `--`; `fn main(args: List<String>)` receives only the program arguments:

```sh
cargo run -- run examples/process.disp -- first "second argument"
```

Run the same program through the interpreter:

```sh
cargo run -- interpret examples/easy_disp.disp
```

Create and run a directory project:

```sh
cargo run -- new hello
cargo run -- run hello
```
