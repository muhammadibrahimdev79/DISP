# Cryptographic foundations

Status: Pass 019 active. The compiler/runtime foundation and first public/secret randomness APIs
exist; remaining stable cryptographic APIs are not complete and no independent external review has
yet been performed. The current repository-wide verification baseline is 525/525 assertions across
60 harnesses.

DISP cryptography composes ecosystem primitives rather than implementing algorithms inside the
compiler. The foundation provides operating-system secure randomness, zeroizing owned secret bytes,
content-independent equality for equal-length secrets, SHA-256, HMAC-SHA-256, HKDF-SHA-256,
AES-256-GCM-SIV authenticated encryption, strict Ed25519 signatures, and Argon2id password hashing.
Published SHA-256, RFC 4231, RFC 5869, RFC 8452, and RFC 8032 vectors are executable tests.

## Required behavior

- Security randomness comes only from the operating system and fails explicitly if unavailable.
  There is no time-, process-, seed-, or deterministic fallback.
- Random and secret allocation requests are bounded before allocation.
- `SecretBytes` is owning, non-copying, non-cloning, debug-redacted, non-serializable,
  non-transferable across thread boundaries, and zeroized on drop. The bootstrap Rust wrapper's
  internal exposure is named `expose_secret`; the DISP source type exposes no byte extraction.
- Secret comparison uses a constant-time primitive for equal-length values. Length is not secret.
- HMAC verification uses the MAC implementation's verification operation rather than ordinary byte
  equality.
- HKDF output is nonempty and limited to the RFC 5869 SHA-256 maximum of 8,160 bytes.
- AES-256-GCM-SIV accepts exactly 32-byte keys, generates every 96-bit encryption nonce internally
  from the operating system, authenticates caller-provided associated data, and returns plaintext
  only after successful authentication. Ciphertext, plaintext, and associated data are bounded
  before cryptographic work or allocation. Imported envelopes are accepted only for decryption.
  The dependency's opt-in intermediate-key zeroization is enabled explicitly.
- Ed25519 signing keys are generated from operating-system entropy, cannot be cloned or exported by
  the bootstrap wrapper, redact secret state, and are zeroized by the underlying implementation.
  Verification is strict and treats malformed keys, malformed signatures, weak keys, altered
  messages, and invalid signatures as failure.
- Password hashes use Argon2id version 19 with a fresh 128-bit salt, 32-byte output, 19 MiB memory,
  two iterations, and parallelism one. Verification rejects noncanonical algorithms, versions,
  parameters, salts, output sizes, oversized encodings, and attacker-selected resource costs before
  hashing. Wrong passwords return a normal negative result. Argon2 working-state zeroization is
  enabled explicitly.
- Randomness is represented by the `Random` effect. Pure code must not silently acquire entropy.
- The stable source-level operation
  `Crypto.random_bytes(integer) -> Result<List<u8>, CryptoError> uses Random` is implemented by the
  static checker, effect inference, interpreter, and native backend. It accepts 1 through 1,048,576
  bytes and uses `BCryptGenRandom` on Windows or the `getrandom` system call on Linux without an
  insecure fallback. Its result is public byte material.
- The stable source-level operation
  `Crypto.random_secret(integer) -> Result<SecretBytes, CryptoError> uses Random` has the same
  bounds and provider contract. `SecretBytes` exposes only `len`, `is_empty`, and
  `constant_time_equals`; direct formatting and ordinary equality are compile errors. Nested
  diagnostic formatting is redacted, and native ownership cleanup zeroizes the full allocation
  before deallocation.
- `Crypto.import_secret(List<u8>)` consumes its input and transfers the allocation into opaque
  `SecretBytes`; rejected inputs are zeroized before release. It cannot erase copies created before
  import and provides no reverse extraction operation.
- `Crypto.sha256(List<u8>)`, `Crypto.hmac_sha256(SecretBytes, List<u8>)`, and
  `Crypto.hmac_sha256_verify(SecretBytes, List<u8>, List<u8>)` are Pure, borrow messages and secret
  keys without consuming them, and cap messages at 16 MiB. Native programs use Windows CNG or the
  Linux kernel AF_ALG provider rather than a compiler-embedded hash implementation. Verification
  computes the authenticator and uses an explicit fixed-length, content-independent comparison.
- `Crypto.hkdf_sha256(salt, input, info, output_length)` is Pure, borrows its byte context and
  secret input key material, accepts an empty salt using the RFC 5869 default, caps salt and info at
  1 MiB each, requires 1 through 8,160 output bytes, and returns opaque `SecretBytes`. Native extract
  and expand steps use the same operating-system HMAC provider; PRK, expansion blocks, temporary
  messages, failed partial output, and final owned output all follow explicit zeroization rules.
- Native primitives unavailable from operating-system providers use the bundled, versioned
  `disp-crypto-native` C ABI. Its first ABI version exposes AES-256-GCM-SIV seal/open backed by
  RustCrypto. Callers own every output buffer; no allocator ownership crosses the boundary. Seal
  generates its nonce internally, open writes caller-visible plaintext only after authentication,
  panics are contained, and malformed pointers, lengths, keys, capacities, and tampering fail with
  stable status codes.
- `AeadEnvelope` is an opaque, owned, non-Copy source type. Source-level AES-256-GCM-SIV seal/open
  borrows keys, plaintext/envelopes, and associated data. Interpreter and native programs agree on
  round trips and fail closed for wrong keys or altered associated data. Native builds link the
  exact compiler-bundled ABI, include its content in the build fingerprint, and stage a
  SHA-256-identical companion beside the executable only when a companion-backed intrinsic is
  present.
- `Ed25519SigningKey` is opaque, non-Copy, non-comparable, non-serializable, formatting-prohibited,
  and zeroized before release. `Crypto.ed25519_generate()` requires `Random` and obtains its seed
  from the operating system. Public-key derivation and signing borrow the key; strict verification
  treats malformed keys/signatures and altered messages as `Ok(false)`. Messages are capped at
  16 MiB. Interpreter and native programs use the same Ed25519 semantics, while native programs
  call the versioned companion with caller-owned key and output buffers.
- `Crypto.argon2id_hash_password` requires `Random`, borrows an opaque password, generates a fresh
  128-bit operating-system salt, and emits a PHC string using the fixed Argon2id v19 policy of
  19 MiB, two iterations, parallelism one, and a 32-byte output. Verification is Pure, rejects
  malformed or noncanonical parameters before hashing, and treats a wrong password as
  `Ok(false)`. Passwords and encodings are bounded at 1 KiB. The native companion owns no caller
  allocations and exposes only bounded caller-buffer operations.
- Authenticated envelopes have one canonical public encoding: `DISP` magic, format version one,
  algorithm identifier, nonce/tag sizes, unsigned 64-bit big-endian ciphertext length, nonce, and
  ciphertext. Decode rejects unknown versions and algorithms, malformed lengths, overflow,
  truncation, trailing bytes, and oversized records. Decode reconstructs only the opaque envelope;
  it never authenticates or releases plaintext. Interpreter and native bytes are identical.
- Ed25519 public keys and signatures have distinct canonical public records. Both use `DISP`
  magic, version one, algorithm one, a record-kind discriminator, and an exact payload length.
  Public-key records carry 32 bytes under kind two; signature records carry 64 bytes under kind
  three. Strict decoders reject cross-kind confusion, unknown metadata, truncation, and trailing
  bytes. These operations are Pure and borrow their inputs.
- `Crypto.ed25519_key_id` validates a non-weak public key and computes a stable, domain-separated
  32-byte SHA-256 identifier. It is Pure and contains no secret material, allowing durable audit,
  rotation, and revocation records to identify keys independently of mutable names.
- `Crypto.ed25519_verify_keyed` binds strict signature verification to an expected stable key ID.
  Identifier comparison avoids content-dependent early exit, and a valid signature from any
  unapproved replacement key returns `Ok(false)`. This is the primitive used by rotation,
  revocation, deployment pinning, and audit policies.
- `Crypto.ed25519_verify_lifecycle` additionally enforces an inclusive activation/expiry window and
  explicit revocation. Its evaluation time is caller-supplied, so results are Pure, reproducible,
  and auditable rather than dependent on an ambient clock. Invalid windows are errors; premature,
  expired, revoked, or identity-mismatched keys return `Ok(false)`.
- The bootstrap `HardwareEd25519Key` boundary keeps external and hardware private keys outside the
  compiler process. It stores only a non-cloning, redacted, zeroizing opaque handle, pins the
  provider executable's SHA-256 content and expected DISP key ID, and communicates through the
  bounded, cleared-environment component host. Its exact `disp.keystore.v1` frames expose only
  public-key and sign operations. DISP rejects key substitution and independently verifies every
  returned signature against the requested message before returning it.
- Provider implementations use `Ed25519KeyProvider` and `serve_ed25519_provider_once`. The SDK
  dispatches only an opaque handle and an optional bounded message, represents rejection with a
  nonzero byte rather than provider diagnostics, and writes exactly one component-framed response.
  This supplies a reusable integration surface without defining any private-key export callback.

## Explicit non-guarantees while Pass 019 is active

- Zeroization cannot erase copies made before a value entered `SecretBytes`, compiler spills,
  registers, swap, crash dumps, or hardware side channels.
- Constant-time behavior is a whole-system property; the current primitive is best-effort software
  resistance and is not a hardware timing certification.
- Additional opaque source-level key types, key serialization, rotation/versioning, protocol
  construction, password peppers, policy migration, stable source-level keystore APIs, audited
  platform-specific TPM/Secure Enclave/PKCS#11 providers, and provider-specific device grants
  remain unfinished. Content rechecking narrows but cannot eliminate the operating system's
  path-replacement interval between the final measurement and process creation; production
  deployments also require OS ownership/ACL and code-signing policy.
- AES-GCM-SIV reduces the damage of accidental nonce reuse but does not make unlimited reuse safe,
  hide message lengths, prevent rollback, or supply a storage/wire protocol.
- The selected AES-GCM-SIV crate states that it has not received a dedicated security audit; some
  underlying AES and POLYVAL dependencies were reviewed. This implementation therefore remains a
  bootstrap foundation, not a cryptographic certification.
- DISP does not yet claim an external cryptographic review.
