# DISP editions, features, and compatibility

DISP editions make source interpretation explicit. Edition `1` is the only Candidate 1 edition.
A package selects it with `edition = "1"` in `DISP.toml`; a legacy manifest without the field is
interpreted as edition `1` so an existing project does not silently change meaning. Standalone
`.disp` files also use edition `1` with an empty feature set.

## Compatibility contract

- An accepted edition keeps its syntax and semantics for the lifetime of that edition.
- Additive syntax may enter an edition only when it cannot change the meaning of accepted source.
- A breaking syntax or semantic change requires a new opt-in edition.
- A deprecation remains accepted for at least the rest of its edition. Removal requires a later
  edition and a deterministic migration.
- Package dependencies select editions independently. A dependency does not change its caller's
  edition or feature set.
- Unknown editions and feature names fail before parsing package source. The compiler never
  guesses a closest edition, enables a feature transitively, or silently ignores a request.

## Feature gates

`features = []` is an explicit, bounded set of opt-in language features. Names are unique lowercase
ASCII identifiers containing letters, digits, and hyphens. Candidate 1 deliberately exposes no
unstable language features, so every non-empty set is rejected. Future preview work must name,
document, test, and locally enable its gate before it can affect source interpretation.

Feature requests are package-local and are part of the manifest bytes covered by dependency
locking. Stable language behavior never requires an unstable feature gate.

## Migration

`disp migrate <project>` inserts the current edition and explicit feature set into a legacy
manifest. It does not rewrite source files, and running it again makes no changes.
`disp migrate --check <project>` performs the same analysis without writing and fails when an
update is required, making it suitable for CI. Migration refuses malformed manifests, unknown
editions, and unsupported feature requests rather than attempting a lossy rewrite.
