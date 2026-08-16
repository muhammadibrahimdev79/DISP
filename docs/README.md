# DISP documentation

DISP documentation is separated by reliability so readers can distinguish
working behavior from future design ideas.

## Verified implementation documentation

These documents are maintained against the compiler and its tests:

- [Compiler status and usage](compiler/README.md)
- [Implemented surface syntax](language/SURFACE_SYNTAX.md)
- [DISP Core Specification 1.0 Candidate 1](language/SPECIFICATION_1.md)
- [Stable diagnostic codes and JSON protocol](language/DIAGNOSTICS.md)
- [Capability/effect model and threat analysis](language/CAPABILITIES.md)
- [Deterministic compile-time model](language/COMPTIME.md)
- [Typed failure and exactly-once cleanup model](language/ERRORS.md)
- [Edition, feature-gate, deprecation, and migration policy](language/COMPATIBILITY.md)
- [Ownership, loan, lifetime, and destruction state machine](language/OWNERSHIP.md)
- [Capability-bounded unsafe execution](language/UNSAFE.md)
- [Initialization, representation, pinning, and aliasing model](language/MEMORY_SAFETY.md)
- [Thread, mutex, and atomic memory-ordering model](language/CONCURRENCY.md)
- [Structured async, task cancellation, deadlines, and cleanup](language/ASYNC.md)
- [Resource quotas and exhaustion behavior](language/RESOURCE_LIMITS.md)
- [Active OS sandbox threat model and completion gates](language/SANDBOX.md)
- [Bounded out-of-process foreign component protocol](language/COMPONENTS.md)
- [Active cryptographic foundations and threat model](language/CRYPTOGRAPHY.md)
- [Public core conformance corpus](../conformance/README.md)
- [Native benchmark baselines](performance/NATIVE_BASELINES.md)
- [0.1 developer-preview release notes](releases/RELEASE_NOTES_0.1.md)
- [DISP 1.0 release plan and ship gates](releases/RELEASE_PLAN_1.0.md)
- [DISP 1.0 100-pass execution ledger](PASSES.md)
- [DISP showcase projects and executable proof gates](SHOWCASE_PROJECTS.md)
- [Executable examples](../compiler/examples/)
- [Compiler test suites](../compiler/tests/)

When documentation and compiler behavior disagree, the compiler tests describe
the current implementation.

## Design drafts

The documents in [drafts](drafts/) were generated during early GPT-assisted
design work. They preserve the project vision and possible future directions,
but they have not been fully reconciled with the implementation. They are not a
stable language specification and may contain contradictions or unimplemented
features.
