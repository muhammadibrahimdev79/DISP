# DISP 0.1.0 Developer Preview

DISP 0.1 is the first public developer preview of the Data · Intelligence · System ·
Page language. It is intended for experimentation, examples, compiler feedback, and
early native programs. It is not a stable production release.

## Included

- One Windows x64 installer containing `disp.exe` and its private native toolchain.
- Direct `disp file.disp`, `disp run`, `disp build`, `disp check`, and
  `disp interpret` workflows.
- Functions, recursion, lexical scopes, mutable and immutable bindings, checked
  numerics, control flow, structs, enums, exhaustive matching, `Option`, `Result`, and
  `?` propagation.
- Ownership, moves, borrowing, non-lexical loan checking, dynamic indexed/subslice
  places, deterministic drops, slices, borrowed UTF-8 `str`, and generic collections.
- HIR, MIR, CFG validation, concrete native layouts, monomorphization, C ABI/FFI, and
  native executable generation.
- Files, paths, time, processes, threads, mutexes, atomics, networking, TLS, HTTP, URL,
  JSON, and SQLite compatibility foundations.
- Nominal DISP Data schemas and compiler-owned add/save/find/remove plans. `data
  memory` executes on the first DISP-owned native row engine without translating plans
  to SQL.

## Important preview limits

- Language and library compatibility may change before DISP 1.0.
- This installer targets 64-bit Windows. Other operating systems and architectures are
  not included in this artifact.
- Intelligence and Page are early compiler domains, not complete AI or application UI
  platforms yet.
- The persistent DISP-native page/journal/recovery engine is under development;
  durable `data open` currently uses the hidden SQLite compatibility provider.
- The package manager, debugger, formatter, language server, self-hosting compiler,
  freestanding OS target, GPU toolchain, and full Page renderer are not complete.
- The installer is not yet code-signed, so Windows may show an unknown-publisher
  warning. Verify the published SHA-256 before running it.

## First program

Create `hello.disp`:

```disp
fn main() {
    print("Hello from DISP 0.1")
}
```

Run it from a new terminal:

```text
disp hello.disp
```

