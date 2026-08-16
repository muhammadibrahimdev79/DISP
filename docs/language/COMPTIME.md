# DISP deterministic compile-time model

Status: Pass 006 active, constant evaluator and bounded structured expansion implemented.

DISP treats compiler execution as a security boundary. Candidate 1 evaluates every local `const`
initializer during checking in a deterministic environment with no filesystem, network, process,
environment, FFI, GPU, or UI authority. A constant failure is a compile-time diagnostic; it is not
deferred until the program runs.

After the original program passes resolution, typing, ownership, effects, and constant evaluation,
the evaluated values are folded into the AST before HIR/MIR lowering. Runtime IR therefore receives
the final value rather than re-executing the constant arithmetic. Folding happens after validation,
so it cannot erase an error from the source program.

```disp
fn main() {
    const base = 7
    const answer = base * 6
    print(answer)
}
```

`disp check --dump-constants app.disp` prints the evaluated manifest:

```text
main::base = 7
main::answer = 42
```

## Current constant language

Candidate 1 currently evaluates literals, arrays, struct construction and field selection, prior
lexical constants, unary operations, checked integer and floating-point arithmetic, comparisons,
short-circuit Boolean operations, and constant `match` expressions. Runtime function calls,
closures, async work, resource access, borrowing, mutation, and Data operations are rejected in a
constant initializer.

Integer arithmetic is checked over the full signed-magnitude range needed to represent positive
`u128` values. Overflow and division by zero are diagnostics with source spans. Struct fields are
rendered in stable name order, so manifests do not depend on hash-map iteration or host behavior.

## Mandatory resource limits

The default evaluator limits are part of the Candidate 1 implementation contract:

| Resource | Limit |
|---|---:|
| Evaluated expression steps | 100,000 |
| Expression recursion depth | 128 |
| Materialized value nodes | 65,536 |
| Materialized string bytes | 1 MiB |

The counters cover the whole compilation unit and use fail-closed diagnostics. Tests also run the
same evaluator with deliberately small limits to prove exhaustion is deterministic.

## Structured expansion

DISP does not use textual preprocessing or unrestricted native compiler plugins. Candidate 1 has
two compiler-owned structured operations:

```disp
let defaults = Meta.repeat(3, 0)
let squares = Meta.map(5, |index: int| index * index)
```

`Meta.repeat` clones a parsed expression. `Meta.map` substitutes only references bound by its
explicit one-parameter expression closure, so call-site names remain call-site names and a nested
binder with the same spelling shadows normally. Neither operation introduces an identifier.
`Meta` is a reserved compiler-owned namespace and cannot be shadowed by a local, parameter,
function, or type declaration.

Each count is a non-negative constant integer expression capped at 4,096. Expansion depth is
capped at 64 and total generated syntax at 65,536 nodes per compilation unit. The expansion runs
before resolution, reports source-spanned failures, records a deterministic trace through
`disp check --dump-expansions`, and has no ambient authority.

DISP intentionally has no user-defined macro syntax yet. The implemented `Meta` surface provides
bounded generation without promising a general plugin model or capture-prone text substitution.
