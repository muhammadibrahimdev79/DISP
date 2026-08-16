# DISP capability and effect model

Status: Candidate 1 static authority model.

DISP separates memory unsafety from ambient authority. `unsafe` permits a narrow operation whose
contract the compiler cannot prove; it does not grant filesystem, network, process, GPU, or UI
authority. Function effects are written after the result type:

```disp
fn load(path: Path) -> Result<String, IoError> uses FileSystem {
    return File.read_text(path)
}

fn lookup() -> Result<uint, NetworkError> uses Network {
    return Ok(Dns.resolve("example.com")?.len())
}

fn add(left: int, right: int) -> int uses Pure = left + right
```

The capability names are `FileSystem`, `Network`, `Process`, `Foreign`, `RawMemory`, `DeviceIo`,
`Timer`, `Random`, `Gpu`, and `Ui`.
`Pure` is an explicit empty contract and cannot be combined with another capability.

## Inference and contracts

An omitted `uses` clause requests whole-program inference. This keeps ordinary code concise while
still producing a concrete effect type. An explicit clause is a maximum contract: every operation
and every transitively called function must fit inside it. Calling a function with an explicit
contract propagates the complete declared contract, even if its current implementation uses less;
that preserves API substitutability as implementations evolve.

```text
disp check --dump-effects project
```

prints the deterministic authority manifest. `[inferred]` distinguishes inferred contracts from
source-declared ones. Public libraries should declare contracts; application-local helpers can use
inference without hiding their final authority from tooling.

## Authority sources

Candidate 1 classifies authority as follows:

| Capability | Ambient acquisition operations |
|---|---|
| `FileSystem` | `File.*`, async file operations, `Database.open`, durable `data open` |
| `Network` | DNS, TCP/UDP bind/connect, TLS, HTTP, and async network acquisition |
| `Process` | process creation/control and explicit environment access |
| `Foreign` | calls to functions declared in `extern C` blocks and typed `CFunction` callback invocation |
| `RawMemory` | raw-pointer dereference and offset/read/write inside a bounded unsafe region |
| `DeviceIo` | protected-target port I/O or authenticated-page AArch64 MMIO inside an explicitly contracted unsafe region |
| `Timer` | acquisition of a progressing clock source through `Time.ticks()` |
| `Random` | operating-system entropy and secret/key generation |
| `Gpu` | reserved GPU device/queue acquisition roots |
| `Ui` | reserved Page/window/UI host acquisition roots |

Operations on an already owned resource do not reacquire ambient authority. For example, a
function receiving an owned `TcpStream` may read it without global `Network`; possession of that
linear resource is the authority. It cannot create a new connection without `Network`. The same
principle will apply to directory handles, GPU devices, and UI hosts as their owned APIs mature.

## Effect erasure rules

Candidate 1 function-value types do not yet encode effect rows. Therefore a capability-bearing
named function cannot be converted to a function value, and a closure that performs or calls
capability-bearing work is rejected. Direct calls remain fully supported. This is deliberately
fail-closed: silently treating an effectful callable as pure would allow authority to bypass a
caller contract. A later effect-row extension may lift this restriction without weakening it.

Trait method calls conservatively inherit the union of matching implementation-method contracts
when syntax alone does not identify one implementation. This can over-report authority but cannot
under-report it.

## Threat model

| Threat | Candidate 1 defense | Remaining work |
|---|---|---|
| Hidden file/network/process access | Whole-program inference plus checked explicit contracts | OS sandbox enforcement in the sandbox pass |
| FFI used as an authority escape | Every external call requires both `unsafe` and `Foreign` | Foreign-library policy and process isolation |
| Raw memory hidden in a helper | Explicit unsafe regions contribute `RawMemory`, which propagates through direct calls | `MemoryPtr<T>` retains typed provenance and lifetime loans; thin FFI pointers remain trusted |
| Hardware access hidden in a helper | `Port.*` and `Mmio.*` require explicit `unsafe uses DeviceIo` and propagate `DeviceIo` through direct calls; AArch64 MMIO is relative to one authenticated, width/alignment/range-checked page | Fine-grained owned register/device grants |
| Hardware timer enabled implicitly | `Time.ticks()` propagates distinct `Timer` authority; x86-64 requires an explicit contract before unmasking IRQ0 | APIC/high-resolution timer providers |
| Effectful helper called from `Pure` | Transitive fixed-point propagation rejects the call site | Incremental effect-query optimization |
| Effect hidden in callback/closure | Capability-bearing function values and closures fail closed | Effect-bearing function types |
| Durable Data silently opens files | `data open` requires `FileSystem`; `data memory` remains pure | Fine-grained directory/database handles |
| Future GPU/UI APIs bypass model | `Gpu` and `Ui` identities are reserved in the capability type | Concrete device/host capabilities in domain passes |
| Declared permission exceeds actual need | Deterministic manifest exposes every declaration | Least-authority lint and interactive grant tooling |

This pass provides static visibility and isolation. It does not claim that the operating system has
already sandboxed a process. Runtime grant enforcement, build-script isolation, and foreign
component sandboxing are tracked separately in the 100-pass release ledger.
