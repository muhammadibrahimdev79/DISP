# DISP public core conformance corpus

This directory contains implementation-independent source fixtures for the normative rules in
`docs/language/SPECIFICATION_1.md`. `manifest.tsv` is deliberately plain TSV so another DISP
implementation can consume it without linking the Rust bootstrap compiler.

Columns are `rule`, `case`, `mode`, `stage`, `expected`, and `source`:

- `check` means the program must pass the complete static pipeline.
- `reject` means it must fail at the named stage and its diagnostic must contain `expected`.
- `run` means it must pass, execute, and produce the `\n`-escaped output in `expected`.
- `diagnostic` means it must fail with the stable diagnostic category code in `expected`.
- `project-check` means the `source` directory and its `DISP.toml` must pass the complete static pipeline.
- `project-reject` means that project must fail at the named stage with the expected diagnostic fragment.

From the bootstrap compiler, run:

```text
cd compiler
cargo test --test conformance -- --test-threads=1
```

Run cases are checked through the interpreter and native backend. If Windows application-control
policy returns OS error 4551 for a newly generated executable, the Rust harness records the native
launch as unavailable rather than confusing host policy with a language failure. A release gate
must run the same corpus on an environment that permits every native artifact.

Set `DISP_REQUIRE_NATIVE_CONFORMANCE=1` to turn any policy-blocked native launch into a hard
failure. Release verification always uses this setting.
