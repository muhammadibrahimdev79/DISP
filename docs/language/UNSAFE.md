# DISP capability-bounded unsafe execution

Status: Candidate 1 normative companion to `SPECIFICATION_1.md`.

DISP keeps operations the compiler cannot prove inside lexically small unsafe regions. The
preferred form states the exact unsafe capabilities the region may exercise:

```disp
fn read(value: ptr<int>) -> int uses RawMemory {
    unsafe uses RawMemory {
        return *value
    }
}

extern C { fn abs(value: CInt) -> CInt }

fn absolute(value: CInt) -> CInt uses Foreign {
    unsafe uses Foreign {
        return abs(value)
    }
}
```

`RawMemory` authorizes thin raw-pointer dereference and pointer `offset`, `read`, and `write`,
including the checked operations on `MemoryPtr<T>` and `MemoryMutPtr<T>`.
`Foreign` authorizes calls declared by `extern C`. The capability clause is a maximum, not a
request to acquire authority. `DeviceIo` authorizes target-specific hardware-port instructions and
bounded AArch64 MMIO relative to a boot-authenticated device page; unlike legacy unsafe operations,
each access requires an explicit enclosing `unsafe uses DeviceIo` contract. Port operations take a
`u16` port number. `Mmio.*` takes a `u16` page offset and validates width, alignment, and the complete
access range before touching the device. Filesystem, network, process, GPU, and UI operations retain their
normal function effect requirements inside unsafe code.

## Lexical containment

Every explicit unsafe contract enclosing an operation must contain its required capability. A
nested block can reduce authority but cannot widen its parent:

```disp
unsafe uses RawMemory {
    unsafe uses Foreign {
        abs(value) // rejected: Foreign is absent from the outer contract
    }
}
```

Unknown or duplicate capabilities and `Pure` combined with another capability are parser errors.
A bare `unsafe { ... }` remains valid in edition 1 for existing source. New code should use an
explicit contract because it creates a reviewable boundary and contributes its capabilities to
the containing function's inferred effect contract.

## Call-chain containment

An explicit unsafe capability becomes a direct effect of its containing function. The effect
analyzer propagates it through named direct calls until a fixed point. Therefore a `uses Pure`
wrapper cannot call an inferred or declared `RawMemory`/`Foreign`/`DeviceIo` helper, while an inferred wrapper
is reported with the propagated capability by `disp check --dump-effects`.

Capability-bearing functions and closures cannot be erased into effect-free function values in
Candidate 1. This prevents an unsafe call chain from disappearing behind a callable type that has
no effect row.

## What unsafe never disables

Inside every unsafe region DISP still checks name resolution, types, definite initialization,
moves, loans, alias conflicts, mutability, cleanup, effect contracts, and external ABI types.
Unsafe code cannot construct unsupported `MaybeUninit`, union, pinning, or hidden
interior-mutability facilities. Runtime sanitizers remain verification evidence, not language
semantics.

Checked `MemoryPtr<T>` and `MemoryMutPtr<T>` operations retain allocation provenance and are guarded
by static lifetime/loan rules plus native bounds, element-size, and alignment checks. This makes
unsafe syntax a visible authority boundary without turning memory derived from `Memory` into an
unchecked address. Thin `ptr<T>` and `mut ptr<T>` still describe foreign addresses whose allocation
contract DISP cannot prove; operations on those values remain explicitly trusted and should stay
minimal.
