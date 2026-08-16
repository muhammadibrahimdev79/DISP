# DISP ownership, loans, lifetimes, and destruction model

This document defines the Candidate 1 operational ownership model. The model is executable in
the bootstrap ownership analysis and is linked to `compiler/tests/ownership_model.rs`. It is not
a claim of a machine-checked mathematical proof; it is the normative state machine that such a
proof must refine without weakening.

## State

For each program point, ownership state is `S = (Γ, I, L, O)`:

- `Γ` maps each live local identity to its fixed type, mutability, declaration scope, and origin.
- `I` maps an owned local to `Uninitialized`, `Initialized`, `Moved(at)`, or
  `Partial({top-level field -> move site})`.
- `L` is the set of active loans `(place, shared|mutable, borrower, start)`.
- `O` maps references and borrowed views to the place whose storage they require.

A place is `(root, projections)`. Projections include fields, safe dereferences, checked dynamic
indices, and checked subslices. Two places overlap when they may denote any common byte. Distinct
known struct fields and provably disjoint constant index/range regions do not overlap; uncertain
dynamic regions conservatively overlap.

## Transitions

Declaration without a value creates `Uninitialized`; initialization or whole-place assignment
creates `Initialized`. Reading requires an initialized place and no overlapping mutable loan.
Consuming a non-`Copy` whole place changes it to `Moved(at)`. Consuming a field records that field
in `Partial`; initialized sibling fields remain usable, but the whole aggregate and moved field do
not. Reassigning every moved field restores `Initialized`.

Creating a shared loan requires initialized storage and no overlapping mutable loan. Creating a
mutable loan additionally requires a mutable root and no overlapping loan of either kind. Mutation
or movement requires no overlapping loan. A non-`Copy` value cannot move through a reference.

Loans end after their last use, at borrower scope exit, or on a control-flow edge where the
borrower is no longer live. At a branch join the initialized state is conservative: a place is
fully initialized only when every predecessor proves it; moved/partial facts are retained from
any predecessor. Active loans are unioned. Loop entry is checked against both the zero-iteration
and back-edge states.

## Origins and lifetimes

Every safe reference, slice, `str`, `CStr`, and capturing callable carries an origin. A value may
flow only to storage whose lifetime is no longer than every captured origin. Borrowed values may
cross a function return only when one borrowed input unambiguously supplies the return origin.
References to locals cannot escape through `Option`, `Result`, structs, collections, closures,
tasks, threads, or any other aggregate.

Raw pointers do not extend lifetimes. Their construction and storage remain type checked, and
dereference requires `unsafe`; the pointer's external validity contract is not converted into a
safe loan.

## Destruction

Every initialized non-`Copy` local has one logical drop obligation. Obligations execute in reverse
lexical declaration order on scope exit, return, break, continue, failed `?` propagation, and
structured cancellation. A moved place has no obligation. A partially moved aggregate destroys
only its initialized remainder. Reinitialization recreates the corresponding obligation.

MIR makes these decisions explicit with storage statements, drop flags, field drops, and cleanup
blocks. The return value or propagated error is moved into the return place before other locals
are destroyed. Interpreter and native execution must implement the same observable ownership and
resource-release behavior.

## Required safety invariants

At every accepted program point:

1. no read observes uninitialized or moved storage;
2. no two live loans overlap when either is mutable;
3. no mutation or move overlaps a live loan;
4. no safe reference outlives its origin;
5. every owned value is moved once or destroyed once, never both or neither; and
6. every control-flow join and loop back edge preserves the first five invariants.
