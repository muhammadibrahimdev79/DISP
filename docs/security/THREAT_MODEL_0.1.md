# DISP 0.1 threat model

Status: living Pass 020 security artifact, August 16, 2026. This model describes the repository's
current bootstrap implementation; it is not an independent security assessment or certification.

## Security objectives

DISP aims to make memory, authority, effects, resource consumption, and unsafe operations explicit;
fail before committing rejected effects; keep secrets out of ordinary values and diagnostics;
contain untrusted tools/components outside the compiler process; and make builds and release
artifacts reproducible and auditable. A claim applies only where a normative rule and executable
evidence exist.

## Protected assets

- Compiler/runtime integrity and deterministic language semantics.
- Source, package, build-cache, generated-code, and release-artifact integrity.
- Process, filesystem, network, database, and device authority.
- Cryptographic keys, passwords, plaintext, opaque provider handles, and signing identities.
- DataStore durability, transaction boundaries, and application data.
- CI credentials, release signing material, provenance, and dependency metadata.
- Availability under hostile source, protocol, package, and runtime inputs.

## Adversaries and assumptions

In scope are malicious DISP source/packages; hostile files, database content, network peers,
components and key providers; compromised or vulnerable dependencies; malformed ABI callers; and
an unprivileged local user attempting escape or resource exhaustion. Inputs may be deliberately
ambiguous, enormous, concurrent, truncated, or timed to race cleanup.

The operating-system kernel, hardware root of trust, administrator/root account, CI control plane,
and compiler host hardware are trusted in Candidate 1. Their compromise is not claimed survivable.
Physical attacks, invasive hardware extraction, power/EM analysis, speculative-execution leakage,
swap/crash-dump capture, and malicious compiler/CPU backdoors are not currently mitigated. These
are explicit residual risks, not evidence that affected deployments are safe.

## Trust boundaries and controls

| Boundary | Principal threats | Current controls | Residual work |
|---|---|---|---|
| Bytes/source to lexer, parser, checker | crash, ambiguity, algorithmic exhaustion | bounded nesting/work, exact diagnostics, frontend fuzzing | long-duration fuzzing and independent corpus |
| Package/filesystem to compiler | build-script/plugin execution, path escape, cache poisoning | Edition 1 rejection, canonical paths, content fingerprints, transactional writes | signed registry and hermetic build graph |
| Compiler/runtime to compiler/linker/process | command injection, handle leakage, descendants, exhaustion | structured arguments, canonical executables, cleared/minimal environment, quotas, job/cgroup containment | privileged Linux CI evidence remains active |
| Foreign component process | malformed frames, ambient authority, network/filesystem escape | exact bounded frames, LPAC/seccomp profiles, finite deadlines/output | audited user grants and broader platform validation |
| Generated native code and allocator | overflow, use-after-free, double cleanup, quota bypass | ownership/static checks, checked helpers, live allocation meters, differential tests, generated-C and Rust ASan/LSan gates | additional target sanitizers and independent audit |
| Native cryptographic C ABI | pointer/length confusion, panic crossing ABI, unauthenticated output | caller buffers, pointer/length checks, panic containment, authenticate-before-output, ABI tests/fuzzing | dedicated external cryptographic review |
| Secret/key values | copies, formatting, extraction, weak verification | opaque non-Clone types, redaction, zeroization, strict verification, bounded operations | spills/registers/swap and side-channel certification |
| External/hardware keystore | private-key export, provider swap, key substitution, forged signature | opaque handle only, provider digest/key-ID pinning, exact frames, host verification | TPM/Secure Enclave/PKCS#11 adapters, OS ACL/signing policy, path-launch race |
| Dependency and release pipeline | vulnerable/yanked crate, stale advisory data, artifact/lock mismatch | three locked graphs, daily RustSec gate, sanitizer fuzzing, embedded artifact dependency inventory, three-platform native CycloneDX SBOMs, OIDC/Sigstore SLSA and SBOM attestations | hosted evidence and independent verification |
| Network/TLS/HTTP/database inputs | parser confusion, injection, remote exhaustion, transaction corruption | typed APIs, parameter binding, protocol/resource bounds, TLS provider | protocol fuzz depth and production interoperability campaign |

## Unsafe-code inventory

Rust `unsafe` is permitted only in these boundary files and must retain local `SAFETY:` rationale:

- `compiler/crypto-native/src/lib.rs` — versioned C ABI pointer validation and contained calls.
- `compiler/src/data_store.rs` — operating-system file-lock handles and ownership contracts.
- `compiler/src/interpreter.rs` — legacy SQLite compatibility C ABI boundary and runtime ownership; this boundary is not the DISP engine and must leave the core toolchain before 1.0.
- `compiler/src/sqlite_compat.rs` — lazy platform-library loading and exact legacy SQLite C ABI dispatch; the library is resolved only after explicit `Database` construction.
- `compiler/src/process_sandbox.rs` — Windows/POSIX process, token, job, handle, and syscall APIs.

An executable repository test rejects unsafe Rust in any other compiler path and rejects attempts
to suppress unsafe-code diagnostics. This inventory scopes review; it does not prove each block
correct. Changes to these files require boundary-specific negative tests and reviewer attention.

## Abuse cases and required outcomes

- Malformed or oversized data must return a typed/controlled failure before unbounded allocation.
- A component or provider emitting trailing bytes, wrong operation IDs, or ambiguous lengths must
  be rejected; private key bytes must never be requested.
- A valid signature under the wrong identity, inactive/expired/revoked key, or changed message must
  not authorize an operation.
- Failed authentication must not expose partial plaintext.
- Cancellation, timeout, quota exhaustion, or child failure must clean up owned resources exactly
  once and must not commit a rejected filesystem effect.
- A dependency advisory, missing artifact provenance, stale database, fuzz crash, sanitizer report,
  sandbox escape, or unsafe-inventory expansion must block release pending explicit remediation.

## Verification and review cadence

Every dependency-policy change and daily schedule runs advisory and sanitizer-backed fuzz gates.
Every release candidate must refresh this model, audit all three lockfiles and produced artifacts,
run the full cross-platform conformance suite, review unsafe-inventory changes, and record known
residual risks. Pass 093 still requires independent review; this document cannot satisfy it.
