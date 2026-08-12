# DISP

**DISP — Data Intelligence System Page** is an evolving programming language
designed around a simple goal: make ordinary programming easy, safe by default,
and capable of high-performance native execution.

The repository contains the Rust compiler, native backend, interpreter,
examples, fuzz targets, test suites, and the evolving design material.

## Current implementation

The current compiler includes static typing, ownership and borrowing, native
code generation, algebraic data types, generics and traits, strings, slices,
lists, maps, sets, iteration, paths, filesystem operations, and time
foundations. The implementation remains under active development and should not
yet be treated as a stable production language.

The compiler and its tests are the authority for currently implemented
behavior. See the [documentation index](docs/README.md) for verified compiler
documentation and clearly separated design drafts.

## Build and test

The compiler is a Rust crate in `compiler/`:

```sh
cd compiler
cargo build
cargo test -- --test-threads=1
```

Run a DISP example through native compilation:

```sh
cargo run -- run examples/easy_disp.disp
```

Run the same program through the interpreter:

```sh
cargo run -- interpret examples/easy_disp.disp
```
