# DISP documentation

DISP documentation is separated by reliability so readers can distinguish
working behavior from future design ideas.

## Verified implementation documentation

These documents live beside the compiler and are maintained against its tests:

- [Compiler status and usage](../compiler/README.md)
- [Implemented surface syntax](../compiler/SURFACE_SYNTAX.md)
- [Native benchmark baselines](../compiler/NATIVE_BASELINES.md)
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
