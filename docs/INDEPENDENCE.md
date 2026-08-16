# DISP independence contract

DISP independence means that a complete default compiler, runtime, standard library, native Data
engine, package toolchain, and offline application can be built and operated without installing a
third-party language runtime, database engine, hosted service, or proprietary compiler backend.
Interoperability is supported, but never confused with ownership of the core.

## Dependency classes

Every external component must be assigned to exactly one class:

1. **Platform boundary** — CPU instruction sets, firmware contracts, operating-system kernels and
   their documented system APIs. These are target interfaces, not hidden language requirements.
2. **Bootstrap-only** — Rust, Cargo, the bootstrap crate graph, C toolchains, and temporary host
   utilities used while DISP is not yet self-hosting. Each must be locked, audited, reproducible,
   inventoried, and have a named DISP-owned replacement or elimination pass.
3. **Optional connector** — PostgreSQL, SQLite migration, language ABIs, browser/OS integration,
   accelerators, and remote services selected explicitly by an application. Removing every connector
   must leave the compiler and all native/offline facilities functional.
4. **DISP-owned core** — language semantics, frontend, intermediate representations, code generation,
   runtime, Data engine and formats, package tooling, security policy, and conformance tests.

No external component may silently move from an optional or bootstrap role into the DISP-owned core.
An unavailable connector must produce a bounded diagnostic for programs that explicitly select it;
it must never become a fallback implementation of native DISP behavior.

## Universal support contract

DISP pursues universal computing coverage without making every ecosystem part of its trusted core.
Each domain and external system receives one declared support tier:

- **Native** — implemented and specified by DISP, available offline, and covered by the conformance
  suite. This is the preferred tier for fundamental language, runtime, Data, Intelligence, System,
  and Page capabilities.
- **First-class connector** — a typed, capability-gated, isolated integration maintained with DISP
  and tested against the external system. PostgreSQL and major OS, browser, cloud, accelerator, and
  protocol targets belong here until a native facility is appropriate.
- **Compatibility or migration** — generated bindings, ABI bridges, importers, exporters, and
  transpilation tools for existing languages, libraries, formats, and applications.

Every advertised integration must publish its tier, supported versions, required authority,
resource limits, failure semantics, security boundary, and executable conformance evidence. A target
is not called supported merely because syntax exists. New targets can be added without changing the
language core, and removing a connector cannot break unrelated programs.

## Current bootstrap debt

The 0.1 compiler is intentionally and truthfully a Rust bootstrap. Its locked Cargo graph currently
provides Unicode, cryptographic, URL/TLS, platform-API, and bootstrap implementation support. Native
hosted linking currently uses a platform C toolchain, while freestanding image generation already
emits machine code directly without C, an assembler, a linker, an OS, an allocator, or libc.

The legacy `Database` compatibility API reaches SQLite through a quarantined lazy loader in the
bootstrap interpreter and through feature-selected hosted native linking. The default compiler has no
static SQLite import, and an unavailable library affects only explicit `Database` construction. This
is not the DISP Data engine and must still move to an optional isolated connector. `DataStore` already
uses DISP's own fixed-page format, checksums, write-ahead recovery, locks, typed plans, and direct
interpreter/native execution without translating those plans to SQL.

## Release invariants

DISP 1.0 is blocked until all of the following are demonstrated:

- DISP compiles itself to a reproducible fixed point, with a documented independently reproducible
  bootstrap and diverse-double-compilation evidence.
- The default installation and native/offline programs require no database server, SQLite library,
  language VM, network service, LLVM installation, or proprietary compiler toolchain.
- The native Data engine passes schema, query, transaction, recovery, corruption, concurrency, backup,
  and restore suites using only DISP-owned formats and execution code.
- PostgreSQL passes first-class typed interoperability, migration, replication, fault, and security
  suites as an optional connector; removing it changes no native Data semantics.
- Every retained platform boundary and bootstrap input appears in signed SBOM/provenance evidence and
  has an offline verification path.

Independence does not mean reimplementing weak or unaudited cryptography for branding. Cryptographic
primitives require specifications, test vectors, side-channel review, and independent audit before a
DISP-owned implementation can replace a vetted bootstrap component.
