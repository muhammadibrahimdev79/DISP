# DISP 1.0 release plan

Target date: **2026-08-31**. The date is a target, not permission to weaken the language's safety,
independence, compatibility, or evidence requirements. If a mandatory gate is not green, the build
remains a release candidate and MUST NOT be labelled stable 1.0.

## Release-critical scope

DISP 1.0 is the first stable language/toolchain contract. It does not claim that every API from
every existing language has already been reproduced. It MUST provide a coherent native path for
general applications, systems work, data applications, web/service interoperability, and C ABI
embedding, with compatibility work able to grow without breaking the stable core.

The release includes:

- frozen edition-1 grammar, type, ownership, effect, error, and unsafe-containment contracts;
- deterministic compiler, package, formatter, diagnostics, native executable, and library flows;
- a DISP-owned offline Data engine, with SQLite and PostgreSQL available only as optional connectors;
- stable C ABI v1 imports and exports, including generated headers and contained failures;
- documented platform coverage and honest diagnostics for unsupported targets or profiles;
- versioned conformance tests, migration policy, security policy, and reproducible release evidence.

## Mandatory ship gates

1. Every normative `DISP-CORE-*` rule maps to live passing evidence.
2. The full locked test suite, formatting, warnings-denied lint, security scans, fuzz smoke suite,
   sanitizer jobs, and platform CI matrix are green on the release commit.
3. The independence invariants in `docs/INDEPENDENCE.md` are satisfied, including self-hosting and
   diverse-double-compilation evidence. Bootstrap-only components are not represented as native DISP.
4. The default installation has no SQLite, PostgreSQL, language-VM, hosted-service, LLVM, or
   proprietary-toolchain runtime dependency. Optional connectors fail closed when absent.
5. Data corruption/recovery, resource exhaustion, unsafe containment, FFI failure, and installer
   rollback tests pass from clean supported systems.
6. Release artifacts are reproducible, signed, accompanied by verified SBOM/provenance, and install,
   upgrade, uninstall, and run offline on every advertised platform.
7. The specification, command help, examples, migration guide, limitations, and release notes match
   the shipped compiler exactly. No aspirational feature is described as implemented.
8. At least one independent security review has no unresolved release-blocking finding.

## Current decision

Status: **NO-GO for stable 1.0; active release-candidate development.**

Pass 22 now has deterministic C ABI headers and an initial C-callable shared-library path with real
C-host execution and checked-panic containment across transitively verified allocation-free helpers
and recursion. Generated callback types support indirect C-to-DISP invocation, while typed thin
`CFunction` values support context-free DISP-to-C invocation under explicit `Foreign` authority.
Same-thread nested foreign re-entry now fails closed without disarming the outer export. Owned
callback contexts now have linear `CRegistration` ownership with consuming close and exactly-once
native scope cleanup. Foreign C threads now attach and detach explicitly, with thread-local failure
containment and concurrent real-host evidence. Asynchronous provider registrations now have explicit
quiesce-before-release cleanup backed by a real threaded fixture. Exact `CExport.callback` handles
now deliver checked DISP wrappers through real provider-created threads. Atomic
`CRegistration.register_async` now owns structurally Send-compatible captures, including
heap-owning strings, behind checked context trampolines, borrows the reusable environment on every
invocation, and drops it only after provider quiescence. Cleanup-aware resource-owning exports
cover heap-only values through transactional allocation and call-depth rollback. `CRegistration`
now adds typed reverse-order rollback across contained failure, with exactly-once provider release
over one thousand acquire-then-fail calls. Other handle classes remain fail-closed until separately
hooked. Explicit C ABI records now preserve nested packet fields
across a real C-to-DISP-to-C round trip with strict C11/C++17 layout assertions, while target
assembly verifies their Windows x86-64 and i686 calling conventions. More importantly,
self-hosting, diverse bootstrap verification, complete connector isolation,
the full platform release matrix, and independent review remain open mandatory gates.

The next release review occurs whenever one of those blockers receives executable evidence. The
August 31 target remains visible, but stability is decided only by the gates above.
