# DISP documentation

DISP documentation is separated by reliability so readers can distinguish
working behavior from future design ideas.

## Verified implementation documentation

These documents are maintained against the compiler and its tests:

- [Compiler status and usage](compiler/README.md)
- [Implemented surface syntax](language/SURFACE_SYNTAX.md)
- [Native benchmark baselines](performance/NATIVE_BASELINES.md)
- [0.1 developer-preview release notes](releases/RELEASE_NOTES_0.1.md)
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
