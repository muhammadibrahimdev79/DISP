# DISP initialization, representation, pinning, and aliasing model

This document defines the Candidate 1 rules that refine the operational ownership model in
`OWNERSHIP.md`. Safety is provided by a deliberately bounded surface, not by exposing every
low-level representation and attempting to repair it afterward.

## Initialization

Uninitialized storage is a compiler state, never a DISP value. A typed local may be declared
without an initializer, but it cannot be read, moved, borrowed, captured, projected, passed, or
returned until every incoming control-flow edge proves it initialized. Struct and enum
construction initializes every active field exactly once. DISP exposes no safe `MaybeUninit<T>`
or assumed-initialized conversion in Candidate 1.

Partial movement changes which fields carry drop obligations; it does not create readable
uninitialized bytes. Reinitializing every moved field restores the complete aggregate state.

## Union representations

The safe union is `enum`: a compiler-owned discriminant identifies the one initialized variant,
and payload access requires a matching pattern. Native layout may use a C union internally only
behind that checked discriminant. Source-level untagged `union` declarations are reserved and
rejected. Foreign untagged unions require an audited external accessor or raw-memory handling in
an explicit unsafe region; they never acquire safe enum semantics by declaration.

## Pinning

Candidate 1 exposes no address-sensitive safe type, self-reference constructor, public `Pin<T>`,
or safe operation that promises a stable address. Consequently safe code cannot observe a move by
address or create a value whose validity depends on its location. Async scheduling may pin storage
inside the compiler-owned runtime, but that implementation detail does not create a source-level
pinning contract. `Pin<T>` fails as an unknown type until a later edition specifies construction,
projection, destruction, and `Unpin` coherence together.

## Interior mutability

Mutation through a shared reference is available only through compiler-recognized synchronization
types. Candidate 1 provides `AtomicInt` for atomic integer operations and `Mutex<T>` with a scoped,
non-transferable `MutexGuard<T>`. These types are explicit and non-`Copy`; their methods do not
turn ordinary `&T` into mutable access. Hidden `Cell<T>`, `UnsafeCell<T>`, and equivalent escape
hatches are not core types and fail closed.

## Aliasing

Ordinary `&T` permits reads and blocks overlapping mutation or movement. `&mut T` is exclusive and
blocks every overlapping access except operations through that same loan. Place overlap is proven
for known fields and constant index/range regions and assumed when dynamic identity is uncertain.
Raw pointers never become safe aliases: dereference remains unsafe, requires the `RawMemory`
capability in an explicit bounded region, and unsafe context does not disable type,
initialization, ownership, effect, or destruction checking. See `UNSAFE.md` for region contracts.

## Checked memory pointers

`MemoryPtr<T>` and `MemoryMutPtr<T>` are bounded, provenance-carrying views into an owned `Memory`
allocation. `Memory.as_ptr()` produces `MemoryPtr<u8>` and `Memory.as_mut_ptr()` produces
`MemoryMutPtr<u8>`. The pointer representation carries its current address, allocation base and
byte extent, and element size and alignment. Native offset and access helpers validate that
metadata before pointer arithmetic or dereference; a one-past value may be formed but not read or
written.

The source allocation is the pointer's lifetime token. Origins propagate through bindings,
offsets, supported aggregates, assignment, returns from identity-like direct calls, and call
arguments. While a checked pointer remains live, the owner cannot be moved, destroyed, or accessed
in conflict with its shared or exclusive loan. A pointer derived from local `Memory` cannot be
returned, stored into a longer-lived place, or sent to another thread. Non-lexical analysis ends
the loan after the pointer's last use.

These checked views are not C pointers. Thin `ptr<T>` and `mut ptr<T>` remain separate ABI types
for foreign addresses whose allocation extent and lifetime DISP cannot derive. No implicit
conversion erases checked metadata or ownership provenance.

Native sanitizer builds (`disp build --sanitize`) compile and link generated C with address and
undefined-behavior instrumentation. Sanitizers are a regression detector, not a replacement for
the static rules above.
