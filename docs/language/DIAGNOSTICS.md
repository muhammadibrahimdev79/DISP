# DISP diagnostic protocol

Status: stable Candidate 1 diagnostic envelope.

DISP diagnostics have two presentations over the same information. Human output remains the
default. Tools use the versioned JSON format:

```text
disp --diagnostic-format=json check source.disp
disp build --diagnostic-format=json source.disp
```

The option is global and may occur anywhere before `--`. Text after `--` belongs to the DISP
program and is never consumed by the compiler as a diagnostic option.

## Stable category codes

| Code | Stage |
|---|---|
| `DISP-LEX-0001` | lexical validation |
| `DISP-PARSE-0001` | syntax and bounded parsing |
| `DISP-RESOLVE-0001` | names, scopes, modules, visibility, and mutability resolution |
| `DISP-TYPE-0001` | types, ownership, borrowing, effects currently enforced as types |
| `DISP-RUNTIME-0001` | interpreter/runtime failure |
| `DISP-INTERNAL-0001` | controlled compiler invariant failure |
| `DISP-BACKEND-0001` | native target, code generation, toolchain, or linker failure |
| `DISP-DRIVER-0001` | command-line, host process, or driver failure without a source span |

These category identities are stable. Later versions may add more specific leaf codes within a
category; tooling that only needs the compilation stage can continue matching these category
codes or the explicit `stage` field.

## JSON schema

Each diagnostic is one UTF-8 JSON object on stderr followed by a newline. Candidate 1 compilation
is fail-fast, so a command currently emits at most one object. The envelope is named
`disp.diagnostic.v1`:

```json
{
  "schema": "disp.diagnostic.v1",
  "code": "DISP-RESOLVE-0001",
  "severity": "error",
  "stage": "resolver",
  "message": "unknown name `missing`",
  "file": "source.disp",
  "span": {
    "start": { "line": 2, "column": 11 },
    "end": { "line": 2, "column": 18 }
  },
  "help": null
}
```

Source positions are one-based Unicode-scalar columns and end-exclusive. A compiler diagnostic
always supplies `file` and `span`; project source remapping supplies the real imported file rather
than only the entry path. A driver error uses JSON `null` for both fields. `help` is either a string
or `null`. All strings use complete JSON escaping, including control characters and Windows path
separators.

Field names, nullability, coordinate rules, category codes, and the meaning of `severity=error`
are compatibility promises for schema v1. New optional fields may be added. A breaking envelope
change requires a new schema name and an explicit CLI selection mechanism.

## Failure and security behavior

JSON diagnostics never go to stdout. Successful commands retain their ordinary stdout and emit no
diagnostic. Machine output does not expose internal host objects or weaken source/resource limits.
Malformed source and hostile nesting use the same bounded compiler path as human diagnostics.
Selecting JSON changes presentation only; it cannot change whether a program is accepted.
