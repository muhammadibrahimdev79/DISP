# DISP Core Specification 1.0 Candidate 1

Status: normative candidate for the implemented core, dated August 15, 2026.

This document specifies the behavior that a conforming implementation of the current DISP core
must provide. It is not the final DISP 1.0 stability promise. Rules can change until the 1.0
compatibility freeze, but they cannot be described as implemented without executable evidence.
The Rust bootstrap compiler and the tests referenced here are the current reference implementation.

The historical documents under `docs/drafts` are non-normative. If they disagree with this
document, this document governs the implemented core. Standard-library APIs and the DataStore
file format are documented separately; this specification governs their language-facing types,
ownership, and expressions.

## 1. Conformance and terminology

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative requirements. A program is:

- lexically valid if it produces tokens without a lexer diagnostic;
- syntactically valid if those tokens produce a program tree without a parser diagnostic;
- well resolved if every referenced name has one unambiguous declaration and visibility permits it;
- well typed if every expression and statement satisfies the static type rules;
- ownership valid if moves, borrows, initialization, escape, and destruction checks succeed;
- executable only after all preceding stages and HIR/MIR validation succeed.

No interpreter or backend may execute a program rejected by an earlier stage. Diagnostics are
observable compiler behavior. Category codes, spans, files, help nullability, and the versioned
machine envelope are stable; ordinary English wording remains eligible for clarity improvements.

**DISP-CORE-0001 — ordered validation.** Every execution path MUST perform lexical, syntactic,
resolution, type, ownership, HIR, MIR, and control-flow validation before evaluation or native
execution.

## 2. Source text and lexical structure

Source is UTF-8 text. Positions are one-based line and Unicode-scalar columns; spans are
end-exclusive. An individual source is bounded to 16 MiB. Project loading applies additional
file-count, module-depth, dependency-depth, and aggregate-source limits.

Identifiers use Unicode XID start/continue characters plus `_` and MUST already be in NFC form.
The compiler rejects a canonically equivalent non-NFC spelling rather than silently changing its
identity.

**DISP-CORE-0002 — Unicode identity.** Identifier equality is equality of accepted NFC Unicode
spellings. A non-NFC identifier MUST be rejected with its source span.

Whitespace separates tokens and is otherwise insignificant, with one implemented disambiguation:
after an expression, a `*place = value` form beginning on a later source line is parsed as a new
dereference assignment rather than multiplication. Semicolons are optional statement terminators.

Line comments begin with `//`. Block comments begin with `/*`, end with `*/`, may nest, and MUST
terminate before end of input. Comment contents do not produce tokens.

Integer literals contain decimal ASCII digits and may contain `_` only between digits. Float
literals have a decimal fraction, exponent, or both. The lexer preserves integer magnitude up to
`u128`; static context selects its actual numeric type. Strings use double quotes and characters
use single quotes. Supported escapes are `\n`, `\r`, `\t`, `\0`, `\\`, `\"`, and `\'`. String
literals cannot cross a source line; a character literal contains exactly one Unicode scalar.

**DISP-CORE-0003 — deterministic lexing.** Malformed numbers, strings, characters, comments,
escapes, unsupported characters, and oversized inputs MUST return a bounded diagnostic and MUST
NOT panic.

The recognized punctuation and operators are:

```text
( ) { } [ ] , . : ;
+ - * / % = == ! != < <= > >= & && | || ^ ~
+= -= *= /= << >> -> => ? .. ..=
```

`^`, `~`, `<<`, and `>>` are tokenized for future bit-operation grammar but have no core
expression semantics in Candidate 1. As with reserved future words, an implementation MUST NOT
invent behavior for them.

The reserved words are:

```text
let var const fn return if else match for in while loop break continue
struct enum trait impl type module use as pub async await spawn parallel
move mut unsafe extern export data transaction page component style state route
comptime true false
```

`parallel`, `transaction`, `page`, `component`, `style`, `state`, `route`, and `comptime` are
reserved for future grammar and have no core construct in Candidate 1. A conforming compiler MUST
reject them where an identifier or implemented declaration is required.

**DISP-CORE-0004 — reserved means unavailable.** Reserved future words MUST NOT be assigned an
implementation-specific meaning and MUST NOT be accepted as identifiers.

## 3. Grammar notation

The grammar below is extended BNF. Quoted text is literal, `x?` is optional, `x*` is zero or more,
`x+` is one or more, and `(a | b)` is a choice. `IDENT`, `INT`, `FLOAT`, `STRING`, and `CHAR` are
lexer tokens. Some static restrictions appear after the grammar.

```ebnf
program       = module-decl? top-item* EOF ;
module-decl   = "module" path ";"? ;
top-item      = "pub"? (use-decl | struct-decl | data-decl | enum-decl |
                         function | extern-block | trait-decl) | impl-decl
              | export-function ;
path          = IDENT ("." IDENT)* ;
use-decl      = "use" path ("." "{" import-item ("," import-item)* "}")? ";"? ;
import-item   = IDENT ("as" IDENT)? ;

generic-list  = "<" generic ("," generic)* ">" ;
generic       = IDENT (":" type ("+" type)*)? ;
struct-decl   = "struct" IDENT generic-list? "{" field* "}" ;
data-decl     = "data" IDENT generic-list? "{" data-field* "}" ;
field         = IDENT ":" type ","? ;
data-field    = IDENT ":" type ("primary")? ","? ;
enum-decl     = "enum" IDENT generic-list? "{" variant* "}" ;
variant       = IDENT ("(" type ("," type)* ")")? ","? ;

trait-decl    = "trait" IDENT generic-list? "{" (assoc-decl | signature)* "}" ;
assoc-decl    = "type" IDENT ";"? ;
signature     = "async"? "fn" IDENT generic-list? parameters return-type? effect-clause?
                (";" | ",")* ;
impl-decl     = "impl" generic-list? type ("for" type)?
                "{" (assoc-def | function)* "}" ;
assoc-def     = "type" IDENT "=" type ";"? ;

function      = "async"? "fn" IDENT generic-list? parameters return-type?
                effect-clause? (block | "=" expression ";"?) ;
export-function = "export" "C" "fn" IDENT parameters return-type?
                  "uses" "Pure" (block | "=" expression ";"?) ;
parameters    = "(" (parameter ("," parameter)*)? ")" ;
parameter     = IDENT ":" type | "self" | "&" "mut"? "self" ;
return-type   = "->" type ;
effect-clause = "uses" ("Pure" | capability ("," capability)*) ;
capability    = "FileSystem" | "Network" | "Process" | "Foreign" | "RawMemory"
              | "DeviceIo" | "Timer" | "Random" | "Gpu" | "Ui" ;
extern-block  = "extern" "C" ("(" STRING ")")? "{" extern-function+ "}" ;
extern-function = "fn" IDENT parameters return-type? ("as" STRING)? ";"? ;

type          = "&" "mut"? type
              | "mut" "ptr" "<" type ">"
              | "ptr" "<" type ">"
              | "[" type "]"
              | "[" type ";" INT "]"
              | "fn" "(" (type ("," type)*)? ")" "->" type
              | IDENT ("<" type ("," type)* ">")? ;

block         = "{" statement* "}" ;
statement     = binding | return | conditional | while-loop | for-loop | loop |
                unsafe-block | "break" ";"? | "continue" ";"? |
                expression assignment-op expression ";"? | expression ";"? ;
binding       = ("let" | "var" | "const") IDENT (":" type)? ("=" expression)? ";"? ;
return        = "return" expression? ";"? ;
conditional   = "if" expression block ("else" (conditional | block))? ;
while-loop    = "while" expression block ;
for-loop      = "for" IDENT "in" expression ((".." | "..=") expression)? block ;
loop          = "loop" block ;
unsafe-block  = "unsafe" effect-clause? block ;
assignment-op = "=" | "+=" | "-=" | "*=" | "/=" ;

expression    = logical-or ;
logical-or    = logical-and ("||" logical-and)* ;
logical-and   = equality ("&&" equality)* ;
equality      = comparison (("==" | "!=") comparison)* ;
comparison    = addition (("<" | "<=" | ">" | ">=") addition)* ;
addition      = product (("+" | "-") product)* ;
product       = unary (("*" | "/" | "%") unary)* ;
unary         = ("await" | "spawn" | "move" | "&" "mut"? | "*" | "-" | "!") unary
              | postfix ;
postfix       = primary (arguments | "." IDENT | "[" expression "]" |
                         "[" expression ".." expression "]" | "?")* ;
arguments     = "(" (expression ("," expression)*)? ")" ;
primary       = INT | FLOAT | STRING | CHAR | "true" | "false" | IDENT |
                array | construct | closure | match | data-expression |
                "(" expression ")" ;
array         = "[" (expression ("," expression)*)? "]" ;
construct     = TYPE_IDENT "{" (IDENT (":" expression)? ","?)* "}" ;
closure       = "move"? ("||" | "|" closure-parameter ("," closure-parameter)* "|")
                return-type? (expression | block) ;
closure-parameter = IDENT ":" type ;
match         = "match" expression "{" match-arm* "}" ;
match-arm     = pattern ("if" expression)? "=>" expression ","? ;
pattern       = pattern-atom ("|" pattern-atom)* ;
pattern-atom  = "-"? INT | STRING | CHAR | "true" | "false" | "_" | IDENT |
                (TYPE_IDENT ".")? TYPE_IDENT ("(" pattern ("," pattern)* ")")? |
                TYPE_IDENT "{" struct-pattern-field ("," struct-pattern-field)* ","? "}" ;
struct-pattern-field = IDENT (":" pattern)? | ".." ;

data-expression = "data" ("memory" | "open" expression |
                  ("add" | "save") expression "in" expression |
                  "find" IDENT "in" expression ("where" expression)?
                    ("order" expression ("ascending" | "descending")?)?
                    ("limit" expression)? |
                  "remove" IDENT "in" expression "where" expression) ;
```

An expression chain is limited to 256 binary operators and 256 postfix calls. Parser recursion is
limited to 32. These are fail-closed compiler resource limits, not semantic values observable by a
valid program.

**DISP-CORE-0005 — bounded parsing.** Pathological nesting or chaining MUST terminate with a
diagnostic before exceeding the reference limits; an implementation MAY use lower documented
limits but MUST NOT accept input it cannot process safely.

## 4. Names, modules, and visibility

A lexical scope begins at each function and block. A declaration is unique within the namespace
where it is introduced. A use before a local declaration does not resolve to that later local.
Functions and top-level types are available according to module loading and visibility rules.
Shadowing in a nested lexical scope is permitted.

A source may declare one dotted module path. The project loader maps module paths to deterministic
source paths and rejects declaration/path disagreement, cycles, ambiguous imports, private access,
escaping project entries, and invalid manifests. `pub use` re-exports selected or whole modules.
Imported nominal types retain their defining module identity even when aliased.

**DISP-CORE-0006 — deterministic resolution.** Unknown, duplicate, private, cyclic, and ambiguous
names MUST be rejected; a compiler MUST NOT guess between candidates or filesystem roots.

## 5. Types

DISP is statically typed and has no implicit `any`. The core numeric types are `int`, `uint`,
`i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, `u128`, `f32`, and `f64`.
Other core types are `bool`, `char`, `Unit`, `String`, borrowed `str`, fixed `[T; N]`, borrowed
`[T]`, `List<T>`, `Map<K,V>`, `Set<T>`, `Option<T>`, `Result<T,E>`, `fn(A...) -> R`, shared
`&T`, exclusive `&mut T`, `ptr<T>`, and `mut ptr<T>`. Nominal structs/enums and generic
instantiations are also types. Runtime/standard types such as `Path`, `Future<T>`, `Task<T>`,
`Thread<T>`, `Mutex<T>`, and domain resource types participate in the same ownership rules.

**DISP-CORE-0007 — no implicit weakening.** Assignments, arguments, returns, fields, branches,
patterns, and operators MUST have compatible static types. Nominal types with equal fields are
not interchangeable.

Integer and float literals are context-sensitive. Arithmetic is checked by default. Lossless
numeric widening is implicit only where the complete source range is representable by the target.
Narrowing uses an explicit conversion and returns a typed error where failure is possible.
Wrapping and saturating operations are explicit methods. `&&` and `||` require booleans and
short-circuit left to right.

**DISP-CORE-0008 — checked numerics.** Default integer arithmetic, negation, and conversions MUST
not wrap silently. Overflow, division by zero, invalid shift, and failed narrowing MUST produce the
specified diagnostic or typed failure.

## 6. Bindings and control flow

`let` creates an immutable initialized binding. `var` creates a mutable binding. `const` requires
a compile-time constant expression. A binding may omit its type when initialized; an uninitialized
binding requires a type and MUST be definitely initialized before use. For ergonomic inferred
assignment, an assignment to an otherwise unknown simple local name declares a mutable inferred
local; subsequent assignments are type checked.

**DISP-CORE-0009 — mutability and initialization.** Mutation of immutable storage and any use
before definite initialization MUST be rejected across all control-flow joins and loop back edges.

`if` and `while` conditions are boolean. `break` and `continue` are valid only in loops. A range
loop uses exclusive `..` or inclusive `..=` bounds. A collection loop obtains a shared view of
non-Copy elements for the iteration, preventing conflicting mutation or movement.

**DISP-CORE-0010 — structured control flow.** Return, break, continue, branching, loops, and
short-circuit operations MUST preserve cleanup and MUST have matching interpreter/native results.

## 7. Functions, closures, and algebraic data

Functions have typed parameters and a `Unit` result when `->` is omitted. A non-Unit function
must return on every reachable path. A concise `fn name(...) -> T = expression` requires an
explicit result type. Functions are first-class values with type `fn(A...) -> R`.

Closures require typed parameters. A block-bodied closure requires an explicit result type.
Ordinary closures infer shared, mutable, or consuming captures from use; `move` forces owned
capture. Capture loans live as long as the closure value, including through aggregates and
control-flow joins.

**DISP-CORE-0011 — callable safety.** Arity, parameter/result types, capture ownership, escaping
borrows, reassignment, and destruction of callable environments MUST be statically checked.

Structs and enums are nominal. Construction supplies every struct field exactly once and no
unknown field; `{ name }` is shorthand for `{ name: name }`. Enum variants may have zero or more
typed payloads. `match` is an expression, arms join to one compatible result type, and payload
patterns bind their refined types. Exhaustiveness and reachability are defined recursively over
the complete pattern matrix, including nested finite payloads; open scalar domains require a
catch-all. Struct patterns use `Type { field, other: pattern }`; every field is explicit unless
one final `..` ignores the remainder. Arms are tested in source order in both interpreter and
native execution. A guard is a boolean read-only expression evaluated after its pattern matches.
Guarded arms do not contribute to exhaustiveness because the guard may be false. Pattern-bound
and outer local values cannot be moved or mutably borrowed while evaluating a guard, so failed
guards preserve ownership for later arms.
Pattern alternatives use `left | right`, including inside payload and struct patterns. Every
alternative binds exactly the same names with the same types. Alternatives are expanded as
bounded parsed patterns before resolution; one match may produce at most 4,096 alternatives and
never performs textual substitution.

`Option<T>` has `Some(T)` and `None`; `Result<T,E>` has `Ok(T)` and `Err(E)`. Postfix `?` is valid
only in a function/closure whose result can propagate the corresponding absence or error. It
evaluates its operand once. Success transfers the payload; failure returns the unchanged carrier.
`Result` propagation requires the exact same error type and never guesses a conversion.

**DISP-CORE-0012 — algebraic totality.** Invalid construction, variant payload mismatch,
non-exhaustive match, unreachable pattern, arm-type mismatch, and invalid `?` propagation MUST be
rejected before execution.

**DISP-CORE-0023 — recursive pattern soundness.** Reachability and exhaustiveness MUST be proven
over nested patterns. `|` alternatives MUST have one bounded, identical binding contract. Guards
MUST be boolean and read-only, MUST NOT prove coverage, and MUST preserve the matched value when
false. Interpreter and native execution MUST test patterns and guards in source order.

**DISP-CORE-0024 — explicit typed failure.** Recoverable failure MUST use `Result<T,E>` or
`Option<T>`. `?` MUST preserve the carrier and exact error type, evaluate its operand exactly once,
and MUST NOT introduce a hidden exception channel or implicit conversion. Fatal invariant traps
are not recoverable errors and cannot be caught.

**DISP-CORE-0025 — propagation cleanup.** Before a failed `?` returns, the carrier MUST move into
the return place and every other initialized owned local MUST be destroyed exactly once in reverse
lexical order. Moved storage MUST NOT be destroyed again. Interpreter and native execution MUST
have equivalent propagation and cleanup behavior.

## 8. Generics and traits

Functions, structs, enums, traits, and implementations may declare generic parameters. A
parameter may have multiple `+`-separated trait constraints. Concrete substitutions must be
consistent and satisfy every constraint. Code generation monomorphizes concrete uses.

Traits contain typed method signatures and associated type declarations. An implementation names
`Trait for Target`, defines every required associated type and method, and must be coherent with
all other implementations. Candidate 1 supports `Self.Name` associated-type projections in trait
signatures; the selected implementation resolves them before execution. Broader dependent
projections such as `T.Name` and inherent implementation blocks are rejected until their selection
rules are normative. Dispatch is static. Ambiguous, overlapping, missing, duplicate, or
signature-incompatible implementations are rejected.

Implementation methods must match trait asyncness, alpha-equivalent method generic arity and
constraints, parameters, result after associated-type resolution, and the exact `uses` capability
contract. Every implementation generic must appear in its target or trait arguments. Constraints
on implementation generics and trait arguments are proven against the complete implementation set,
independent of declaration order; cyclic proof attempts fail rather than recurse indefinitely.

`Copy` is a compiler-recognized marker. User aggregate Copy implementations are valid only if all
stored data is Copy for every permitted instantiation. A generic Copy implementation MUST cover
the aggregate's complete permitted generic domain: its target parameters are one-to-one generic
parameters and its constraints exactly mirror the aggregate declaration. Conditional or
specialized Copy implementations are rejected so ownership analysis cannot classify another
instantiation as Copy.

**DISP-CORE-0013 — generic coherence.** Inference MUST have one consistent substitution,
constraints MUST be proven, and trait selection MUST have exactly one coherent implementation.

**DISP-CORE-0022 — exact trait contracts.** Trait implementations MUST define each required
associated type and method exactly once. `Self.Name` projections MUST resolve through the selected
implementation. Method generics, constraints, asyncness, parameter/result types, and capability
contracts MUST match exactly up to generic-parameter renaming. Constraint selection MUST be
declaration-order independent and cycle safe.

## 9. Ownership, borrowing, and destruction

Every non-Copy value has one logical owner. Consuming use moves it; later use before
reinitialization is rejected. Moving one field makes that place unavailable without invalidating
unmoved independent fields. Copy values are duplicated without invalidating the source.

`&T` is a shared loan and `&mut T` is an exclusive loan. While an exclusive loan is live no other
loan or access may conflict; while shared loans are live, conflicting mutation or movement is
rejected. Loan lifetimes are inferred from use (non-lexical) and cannot outlive the referent.
References cannot be returned from locals or hidden in escaping callables/aggregates. A returned
borrow may be elided only when one borrowed input unambiguously supplies its origin.

Owned resources are destroyed exactly once on normal scope exit and on every early return,
propagated error, break, continue, cancellation, and rejected partial path represented in MIR.

**DISP-CORE-0014 — memory safety.** Safe code MUST prevent use-after-move, use-before-init,
double destruction, dangling references, conflicting aliasing, and resource leaks across structured
control flow.

The operational state is `S = (Γ, I, L, O)` as defined in `OWNERSHIP.md`: live typed locals,
initialization/move state, active place-sensitive loans, and borrowed origins. Generic and nominal
aggregates preserve every borrowed origin they contain; destructuring transfers those origins to
the corresponding bindings. At control-flow joins, initialization and loans merge conservatively.
Every structured exit carries exactly the destruction obligations of its initialized, unmoved
storage.

**DISP-CORE-0028 — explicit ownership state.** Ownership checking MUST implement the documented
initialization, move, partial-move, loan, origin, join, and destruction transitions. Borrowed
origins MUST survive generic aggregates, enum payloads, collections, callables, and pattern
destructuring; no carrier may extend an origin's lifetime or hide a conflicting live loan.

Uninitialized storage is not a value and cannot be read, borrowed, moved, captured, projected,
passed, or returned. The only safe union representation is a discriminated `enum`; source-level
untagged unions are unsupported. Candidate 1 exposes no address-sensitive safe type, public
pinning contract, `MaybeUninit`, or hidden interior-mutability primitive. Shared mutation is
available only through the explicit compiler-recognized `AtomicInt` and `Mutex<T>` types. The
complete representation and aliasing rules are defined in `MEMORY_SAFETY.md`.

**DISP-CORE-0029 — bounded safe representation.** Safe code MUST operate only on definitely
initialized active fields, MUST discriminate union payloads before access, and MUST NOT observe a
move by address or mutate ordinary storage through a shared reference. Unsupported uninitialized,
untagged, pinning, or hidden-interior-mutation facilities MUST fail closed. Sanitized native builds
MUST use real address/undefined-behavior instrumentation and include every required runtime
artifact; requesting sanitizers MUST NOT silently produce an uninstrumented executable.

## 10. Unsafe, raw pointers, and FFI

An `unsafe { ... }` block marks operations whose external contract cannot be proven by the safe
core. Entering it does not disable lexical, type, ownership, effect, ABI, or cleanup checking. Raw
pointer dereference and offset/read/write require `RawMemory`; calls to `extern` functions require
`Foreign`. `unsafe uses RawMemory, Foreign { ... }` gives the region an explicit maximum contract.
Every explicit enclosing unsafe contract must contain the capability required by an operation, so
a nested region cannot widen its parent. A bare `unsafe { ... }` remains accepted in edition 1 for
source compatibility, but grants no ambient authority. Safe `Memory` methods remain bounds checked.

Candidate 1 supports `extern C` blocks with an optional library string, portable C aliases, and
optional external link names. Only ABI-safe scalar, pointer, and explicitly supported view types
cross the boundary. Owned DISP aggregates do not cross implicitly. `CString` owns validated
NUL-terminated bytes; `CStr` is a borrowed view and cannot escape its owner.

**DISP-CORE-0015 — contained unsafe.** Unsafe operations MUST be explicit and source-spanned;
unsafe context MUST NOT make an otherwise ill-typed, ownership-invalid, or ABI-invalid program
valid.

**DISP-CORE-0030 — capability-bounded unsafe.** An explicit unsafe-region `uses` clause MUST be a
checked maximum across every lexically enclosing explicit unsafe region. Raw-pointer operations
MUST require `RawMemory`; external calls MUST require `Foreign`; hardware port operations MUST
require an explicit `DeviceIo` contract; none of these capabilities may waive any
other static check or grant unrelated ambient authority. Explicit unsafe capabilities MUST enter
the containing function's effect contract and propagate transitively through direct call chains.
Malformed, duplicate, unknown, or widening region contracts MUST fail before execution.

`Memory.as_ptr()` and `Memory.as_mut_ptr()` produce the distinct checked types `MemoryPtr<u8>` and
`MemoryMutPtr<u8>`. They are provenance-carrying fat pointers rather than C ABI pointers. Their
source allocation remains loaned for every live derived pointer, including copies and offsets, and
that origin cannot be hidden by an aggregate, assignment, or direct call. Thin `ptr<T>` and
`mut ptr<T>` remain separate trusted foreign-address types; no implicit conversion discards the
checked pointer's extent or origin.

**DISP-CORE-0031 — checked memory pointers.** A checked memory-pointer offset MUST remain within
the source allocation or its one-past position. A checked read or write MUST validate provenance,
complete bounds, element size, and alignment before native pointer arithmetic or access. Checked
pointers MUST NOT outlive, move, or conflict with their source allocation, escape a local owner,
cross a thread or C ABI boundary, or be dereferenced through syntax that bypasses these checks.
Interpreter and native controlled-failure behavior MUST agree for invalid offsets and accesses.

## 11. Concurrency and asynchronous execution

`spawn function(arguments)` starts an OS thread and transfers owned Send-compatible values. The
result is `Thread<T>` and consuming `join()` returns `T`. References, raw pointers, borrowed views,
function values, and guards are not Send. A live thread leaving scope is joined during cleanup.

`async fn` calls are lazy and return owned `Future<T>`. `await` is valid only within async context
and consumes its future. `Async.spawn(future)` creates a structured `Task<T>`; every task is
awaited or cancelled and cleaned before its async scope exits. Suspension MUST resume after the
suspension point without repeating prior side effects. `async fn main` is driven automatically.

**DISP-CORE-0016 — structured concurrency.** Work MUST NOT outlive the resources its lexical
scope owns. Thread transfer and async suspension MUST preserve ownership, cancellation, and
deterministic cleanup.

`Mutex<T>` is explicitly shared and recursive for its owning thread. Unlock-release and
lock-acquire establish synchronization, every nested guard contributes one recursion level, and a
guard cannot cross a thread boundary. `AtomicInt` defaults to sequential consistency and exposes
only operation-valid relaxed, acquire, release, acquire-release, and sequentially consistent
method spellings; invalid order/operation pairs do not exist. Checked atomic addition MUST leave
the value unchanged on overflow.

`Channel<T>` is a bounded, owned, explicitly shared MPMC queue. Construction MUST reject zero or
unrepresentable capacity recoverably. Send consumes one `T`, blocks while an open queue is full,
and release-synchronizes with the receive that removes it. Receive blocks while an open queue is
empty, preserves FIFO removal order, drains buffered messages after closure, and returns `None`
only for a closed empty queue. Close MUST be idempotent and wake blocked operations. Final cleanup
MUST destroy every still-queued message exactly once.

**DISP-CORE-0032 — race-safe structured communication.** Thread arguments and results MUST obey
Send-compatible ownership transfer; live handles MUST join during cleanup. Mutex, atomic, and
channel operations MUST implement their documented synchronization order identically in
interpreter and native execution. Unsupported atomic orders, borrowed or pointer-bearing thread
payloads, and invalid channel capacities MUST fail without data races, memory corruption, hidden
detachment, or guessed semantics.

`Task.cancel()` consumes its task handle and MUST prevent every not-yet-executed poll or side
effect in that task tree, release pending operation state, and destroy owned inputs or an unclaimed
ready result exactly once before returning. `Task.is_finished()` borrows the handle, reports
whether its result is ready, and MUST NOT consume or alter that result. A cancelled task handle is
moved and cannot subsequently be inspected or awaited. Operation deadlines begin on first poll,
not future construction, and expiration MUST be a typed domain failure.

**DISP-CORE-0033 — deterministic async cancellation.** Lazy futures and structured tasks MUST
have one linear completion or cancellation path. Explicit cancellation, implicit scope
cancellation, nested-task cancellation, ready-result destruction, deadline expiry, and resource
closure MUST preserve ownership and release operation state without repeating completed side
effects. Interpreter and native execution MUST agree at the documented scheduler boundaries.

## 12. DISP Data expressions

A `data` declaration is a nominal schema. It has exactly one `primary` field whose supported type
is a non-optional signed integer or `String`. `data memory` creates an ephemeral `DataStore` and
`data open path` opens a durable one. `data add`, `data save`, `data find`, and guarded
`data remove ... where ...` are compiler-owned typed expressions. Schema fields, store, predicate,
order key, limit, and written value are checked before lowering to HIR/MIR plans. `remove` without
`where` is syntactically invalid.

The language does not translate these plans into SQL. Interpreter and native execution use the
same DISP-owned logical plans and durable format. The separate `Database` compatibility type does
not become a `DataStore` and cannot inject SQL methods into DISP Data expressions.

**DISP-CORE-0017 — typed Data plans.** A Data expression MUST lower to a typed plan with nominal
schema identity and evaluated-once external values; invalid schema, field, predicate, ordering,
limit, store, or unguarded removal MUST fail before mutation.

## 13. Editions, feature gates, and compatibility

Package manifests select a language edition. Candidate 1 supports edition `1`; missing edition
metadata in a legacy package and standalone `.disp` files mean edition `1`. Explicit edition
selection never flows across a dependency boundary. Accepted source retains its meaning within an
edition, and a breaking change requires a new opt-in edition.

Package feature sets are explicit, bounded, unique lowercase ASCII names. Candidate 1 has no
unstable opt-in features, so the only accepted set is empty. Unknown editions and non-empty feature
sets fail before package source is parsed. `disp migrate` pins legacy manifests to edition `1` and
an empty feature set without rewriting source; `--check` performs the analysis without mutation.

**DISP-CORE-0026 — edition stability.** A compiler MUST preserve accepted syntax and semantics
within edition `1`, interpret legacy or standalone source as edition `1`, isolate dependency
edition selection, and reject unsupported editions without guessing.

**DISP-CORE-0027 — explicit evolution.** Feature requests MUST be explicit, bounded, package-local,
and fail closed when unsupported. Deprecation removal MUST require a later edition and a
deterministic migration. Migration MUST be idempotent, MUST offer a non-mutating check, and MUST
NOT rewrite source whose meaning is unchanged.

## 14. Compiler limits and failure behavior

All accepted source is processed under explicit resource bounds. Malformed input, hostile nesting,
integer extremes, module cycles, corrupt durable data, invalid UTF-8 at external boundaries, and
OS failures produce diagnostics or typed runtime errors. They MUST NOT be assigned guessed meaning.
Runtime bounds violations fail before out-of-bounds memory access or unbounded allocation.

The reference execution pipeline lowers one validated program to HIR and ownership-explicit MIR,
constructs control-flow graphs, and then either interprets the validated AST or builds native code.
Both engines must agree for behavior supported by both. Backend-only facilities must fail clearly
in the semantic oracle rather than simulate an invented result.

**DISP-CORE-0018 — fail closed.** Unsupported, malformed, ambiguous, corrupt, exhausted, and
backend-unavailable operations MUST fail explicitly without running stale output or silently
weakening safety.

**DISP-CORE-0019 — stable diagnostics.** Every controlled failure MUST have a stable category
code and stage. `--diagnostic-format=json` MUST emit one valid `disp.diagnostic.v1` object on
stderr containing severity, message, source file, end-exclusive span, and help; unavailable source
information MUST be explicit JSON `null`. Selecting a presentation MUST NOT alter acceptance,
execution, or program arguments after `--`.

**DISP-CORE-0020 — visible authority.** Every function MUST have an inferred or explicit
capability contract. An explicit `uses` clause is a checked maximum; ambient acquisition and
transitive calls MUST fit inside it. `unsafe` MUST NOT grant authority. Capability-bearing work
MUST NOT be erased into a callable type that lacks effects. Implementations MUST expose a
deterministic effect manifest, and unsupported effect polymorphism MUST fail closed.

`const` initializers are evaluated during checking without ambient authority and under fixed
step, depth, materialized-value, and string limits. Candidate 1 structured generation consists of
`Meta.repeat(count, value)` and `Meta.map(count, |index: int| expression)`. Both expand parsed AST
nodes before resolution; they do not perform textual substitution or execute native plugins.
`Meta.map` substitutes only references bound by its explicit mapper parameter. Call-site names are
preserved, nested binders shadow hygienically, and generated syntax is subject to per-expansion
count, nesting-depth, and compilation-unit node limits. Expansion and evaluated constants have
deterministic manifests.

**DISP-CORE-0021 — bounded compile-time generation.** Compile-time evaluation and structured
generation MUST be deterministic, authority-free, source-spanned, and resource-bounded. Mapper
substitution MUST preserve lexical binding and MUST NOT capture call-site identifiers. Exhaustion,
invalid expansion shapes, and unsupported compile-time operations MUST fail before resolution or
runtime execution.

Candidate 1 package manifests do not admit build scripts, procedural macros, compiler plugins, or
equivalent compiler extensions. Names that request those mechanisms MUST be rejected explicitly
before package source is parsed. The compiler MUST NOT load or execute package-provided extension
code in its own process. A later edition may introduce an extension mechanism only through an
explicit out-of-process containment profile with declared capabilities, bounded resources, and
deterministic, validated inputs and outputs. Trusted in-process C ABI calls remain ordinary runtime
foreign calls governed by `unsafe` and `Foreign`; they are not a compiler-extension mechanism, and
untrusted foreign components require a contained process boundary.

**DISP-CORE-0035 — compiler extension isolation.** Package-provided build scripts, procedural
macros, compiler plugins, and equivalent extension requests MUST fail closed before package source
is parsed. An implementation MUST NOT dynamically load or execute such code in the compiler
process. Any future extension host MUST be explicitly out of process, capability-scoped,
resource-bounded, and deterministic at its compiler-facing boundary.

The Candidate 1 out-of-process component transport uses one `DISPCMP1` request frame and one
response frame. Each frame contains the eight-byte magic, an unsigned big-endian 64-bit payload
length, and exactly that many payload bytes. Payloads are bounded to 8 MiB. Components run under a
dedicated finite process-tree profile with a cleared environment and receive only the
`DISP_COMPONENT_PROTOCOL=disp.component.v1` marker. This transport is resource containment; it
does not claim filesystem or network isolation.

**DISP-CORE-0036 — exact out-of-process components.** Untrusted foreign components MUST NOT share
the compiler or runtime address space. A Candidate 1 component invocation MUST clear ambient
environment values, apply finite memory, CPU, process-count, wall-time, and aggregate-output
bounds before launch, and accept only one exact bounded request and response frame. Invalid policy,
wrong magic, truncation, oversized length, trailing data, unsuccessful exit, output overflow, and
deadline expiry MUST fail explicitly and MUST terminate the contained process tree when it remains
live.

An implementation may report a stronger component authority profile only for authorities it
actually denies. The Candidate 1 Linux component profile is networkless: before component
`execve`, it closes every non-standard inherited descriptor and installs a seccomp layer denying
socket creation and use, legacy multiplexed socket calls, and io_uring setup. The direct and hard
cgroup launch paths MUST enforce the same denial. An unavailable or incompatible hard helper MUST
fail before execution rather than silently selecting a weaker profile when hard mode is required.

**DISP-CORE-0037 — truthful component authority denial.** A component profile MUST NOT claim to
deny an authority unless every launch path enforces that denial before target code executes.
Candidate 1 Linux networkless components MUST be unable to create or operate network sockets and
MUST NOT receive undeclared inherited sockets. Unsupported platform authority guarantees MUST be
reported as unavailable or weaker, never silently emulated or mislabeled as isolation.

Candidate 1 Windows components use path-separated LPAC profiles with no capability SIDs.
The AppContainer SID, exact inherited-handle list, Job Object membership, resource limits, and Job
Object UI restrictions MUST be established as process-creation inputs before component code can
execute. UI restrictions deny external USER handles, clipboard access, desktop switching, global
atoms, display/system changes, and session exit operations. The environment MUST remain cleared
apart from the protocol marker and the minimum Windows profile/bootstrap values.

**DISP-CORE-0038 — Windows AppContainer component authority.** Windows component creation MUST fail
closed if the implementation cannot create or derive the component AppContainer identity, grant zero
capability SIDs, apply the component Job Object and UI limits atomically, or restrict inherited
handles to the declared standard streams. The resulting child MUST demonstrate AppContainer and Low
integrity identity, the bounded enabled-privilege set, denial of both reads and writes to a
parent-owned host object, and network unavailability before this profile is reported.

**DISP-CORE-0039 — LPAC ambient-package authority removal.** Windows components MUST opt out of
`ALL_APPLICATION_PACKAGES` in the same process-creation attribute list that supplies the
AppContainer identity. The resulting child MUST NOT have enabled `ALL_APPLICATION_PACKAGES` token
membership. Implementations MAY additionally verify `TokenIsLessPrivilegedAppContainer` when the
host supports that information class, but an unavailable convenience query MUST NOT replace the
membership proof or select a weaker regular AppContainer.

**DISP-CORE-0040 — capability-checked operating-system randomness.**
`Crypto.random_bytes(length)` MUST have type `Result<List<u8>, CryptoError>`, MUST require or infer
the `Random` capability, and MUST obtain bytes exclusively from a cryptographically secure
operating-system provider. `length` MUST be in the inclusive range 1 through 1,048,576. Invalid
lengths and provider failure MUST produce `Err` before returning partial output. An implementation
MUST NOT fall back to time, process state, a deterministic generator, or a non-cryptographic random
function. Interpreter and native execution MUST enforce the same contract.

**DISP-CORE-0041 — opaque source-level secret ownership.**
`Crypto.random_secret(length)` MUST have type `Result<SecretBytes, CryptoError>`, MUST require or
infer the `Random` capability, MUST use the provider and inclusive bounds required by
DISP-CORE-0040, and MUST fail before exposing partial output. `SecretBytes` MUST be an opaque,
owned, non-Copy type with no source-level extraction, indexing, serialization, ordinary equality,
or direct formatting operation, and MUST NOT cross a spawned-thread boundary. It MUST expose only
`len`, `is_empty`, and explicit `constant_time_equals` inspection. Any nested diagnostic display
MUST redact contents. Final interpreter ownership release MUST invoke a zeroizing secret owner;
native release MUST zeroize the complete allocation before deallocation. Interpreter and native
execution MUST enforce the same observable contract.

**DISP-CORE-0042 — provider-backed hashing and message authentication.**
`Crypto.import_secret(bytes)` MUST consume a `List<u8>` and return
`Result<SecretBytes, CryptoError>`. A successful import MUST transfer ownership without exposing a
reverse extraction operation; a rejected import MUST zeroize the consumed allocation before
release. `Crypto.sha256(message)` and `Crypto.hmac_sha256(key, message)` MUST return
`Result<List<u8>, CryptoError>` containing exactly 32 bytes on success.
`Crypto.hmac_sha256_verify(key, message, expected)` MUST return `Result<bool, CryptoError>` and MUST
compare a freshly computed authenticator through an explicit content-independent comparison for
the fixed 32-byte authenticator length. All three hash/authentication operations MUST be Pure, MUST
borrow rather than consume their inputs, and MUST reject messages larger than 16,777,216 bytes.
Native execution MUST delegate SHA-256/HMAC-SHA-256 to an operating-system cryptographic provider;
it MUST NOT introduce a handwritten hash primitive into the compiler runtime. Provider failure
MUST return `Err`. Interpreter and native execution MUST pass the same published SHA-256 and RFC
4231 known answers.

**DISP-CORE-0043 — bounded zeroizing HKDF-SHA-256.**
`Crypto.hkdf_sha256(salt, input, info, output_length)` MUST have type
`Result<SecretBytes, CryptoError>`, MUST be Pure, and MUST borrow rather than consume its
`List<u8>` salt, `SecretBytes` input key material, and `List<u8>` info. An empty salt MUST select the
RFC 5869 SHA-256 default salt. Salt and info MUST each be no larger than 1,048,576 bytes;
`output_length` MUST be in the inclusive range 1 through 8,160. Invalid lengths and provider
failure MUST return `Err` before exposing partial output. Native extract and expand steps MUST use
the operating-system HMAC-SHA-256 provider required by DISP-CORE-0042. The pseudorandom key,
expansion block, assembled HMAC message, and any rejected partial output MUST be zeroized before
release; successful output MUST inherit `SecretBytes` zeroizing ownership. Interpreter and native
execution MUST pass RFC 5869 case one byte-for-byte.

**DISP-CORE-0044 — versioned bundled native cryptographic boundary.**
Cryptographic primitives not supplied with the required semantics by an operating-system provider
MUST NOT be silently replaced by a different primitive. The bundled native boundary MUST expose an
explicit ABI version and stable status codes, MUST contain panics within the boundary, and MUST NOT
transfer allocator ownership across it. Its AES-256-GCM-SIV seal operation MUST accept exactly a
32-byte key, generate every 12-byte nonce internally from operating-system entropy, and require a
caller-owned ciphertext buffer of plaintext length plus 16 bytes. Open MUST accept exactly that
nonce/key shape and MUST NOT modify caller-visible plaintext storage or its reported length until
authentication succeeds. All pointers, lengths, capacities, and one-megabyte input bounds MUST be
validated before creating slices or writing output. Altered ciphertext, associated data, nonce, or
key MUST fail authentication without releasing plaintext.

**DISP-CORE-0045 — opaque source-level authenticated encryption.**
`AeadEnvelope` MUST be an opaque, owned, non-Copy source type. The operation
`Crypto.aes256_gcm_siv_seal(SecretBytes, SecretBytes, List<u8>)` MUST borrow its key, plaintext,
and associated data and return `Result<AeadEnvelope, CryptoError>`. The operation
`Crypto.aes256_gcm_siv_open(SecretBytes, AeadEnvelope, List<u8>)` MUST borrow all inputs and return
`Result<SecretBytes, CryptoError>`. Both operations MUST use the versioned boundary in
DISP-CORE-0044 for native programs, and interpreter/native behavior MUST agree. Generated programs
using either operation MUST link the exact companion bundled with the running compiler, MUST stage
a byte-identical companion beside the executable, and MUST fail closed if the companion or ABI
version is unavailable. Programs not using these operations MUST NOT acquire that dependency.
Wrong keys, altered associated data, and malformed or altered envelopes MUST return `Err` without
exposing plaintext.

**DISP-CORE-0046 — opaque Ed25519 signatures.**
`Ed25519SigningKey` MUST be an opaque, owned, non-Copy, non-comparable, non-serializable source
type whose formatting is prohibited and whose storage is zeroized before release. The operation
`Crypto.ed25519_generate()` MUST require the `Random` capability, MUST obtain its 32-byte secret
seed only from operating-system entropy, and MUST return `Result<Ed25519SigningKey, CryptoError>`.
Public-key derivation and signing MUST borrow rather than consume the signing key. Public keys MUST
be 32 public bytes and signatures MUST be 64 public bytes. Signing and strict verification MUST
accept messages no larger than 16,777,216 bytes. Verification MUST return `Ok(false)` for malformed
keys/signatures, altered messages, and invalid signatures; provider or resource failure MUST return
`Err`. Native execution MUST use the exact versioned companion required by DISP-CORE-0044, keep
secret-key allocation under the generated program's ownership, and agree with the interpreter.

**DISP-CORE-0047 — fixed-policy Argon2id password hashing.**
`Crypto.argon2id_hash_password(SecretBytes)` MUST borrow its password, require the `Random`
capability, generate a fresh 128-bit salt from operating-system entropy, and return a PHC `String`
using Argon2id version 19, 19,456 KiB memory, two iterations, parallelism one, and a 32-byte output.
Passwords MUST contain 1 through 1,024 bytes and encoded hashes MUST contain 1 through 1,024 bytes.
`Crypto.argon2id_verify_password(SecretBytes, String|str)` MUST be Pure, borrow both inputs, return
`Ok(false)` for a wrong password, and return `Err` for malformed or noncanonical hashes.
Verification MUST validate the algorithm, version, exact parameters, 128-bit salt encoding, and
32-byte output before invoking Argon2 so attacker-selected resource costs are never honored.
Native execution MUST use caller-owned buffers through the versioned companion and MUST agree with
the interpreter on successful and negative verification.

**DISP-CORE-0048 — canonical versioned authenticated-envelope encoding.**
`Crypto.encode_aead_envelope(AeadEnvelope)` MUST borrow the envelope and return a canonical public
`List<u8>` beginning with ASCII `DISP`, format version one, AES-256-GCM-SIV algorithm identifier
one, nonce length 12, tag length 16, an unsigned 64-bit big-endian ciphertext length, the nonce,
and the ciphertext including its tag. `Crypto.decode_aead_envelope(List<u8>)` MUST borrow its input
and return an opaque `AeadEnvelope`. Decoding MUST reject unknown versions or algorithms, incorrect
nonce/tag identifiers, ciphertext shorter than the tag, lengths beyond the one-megabyte plaintext
limit, integer overflow, truncation, and trailing bytes. Decoding MUST NOT authenticate or expose
plaintext; authentication remains exclusively the responsibility of AES-256-GCM-SIV open.
Interpreter and native encoding MUST be byte-for-byte identical.

**DISP-CORE-0049 — typed versioned Ed25519 public records.**
`Crypto.encode_ed25519_public_key` and `Crypto.encode_ed25519_signature` MUST accept exactly 32
and 64 public bytes respectively. Their canonical encodings MUST begin with ASCII `DISP`, format
version one, a distinct record kind (two for a public key and three for a signature), Ed25519
algorithm identifier one, the one-byte payload length, and the unchanged payload. The corresponding
decode operations MUST require the exact total length and MUST reject wrong magic, version, kind,
algorithm, declared length, truncation, and trailing bytes. A public-key record MUST never decode as
a signature record or conversely. All four operations MUST be Pure, borrow their inputs, return
typed `Result` failures, and produce byte-for-byte identical interpreter/native output.

**DISP-CORE-0050 — stable Ed25519 key identity.**
`Crypto.ed25519_key_id(List<u8>)` MUST be Pure, borrow its input, require exactly one valid,
non-weak 32-byte Ed25519 public key, and return exactly 32 public bytes. The identifier MUST be
SHA-256 over the ASCII domain separator `DISP Ed25519 key identifier v1` followed by one NUL byte
and the raw public key. Malformed and weak keys MUST return `Err`; the operation MUST never accept
secret key material or expose it. The identifier MUST be deterministic across interpreter/native
execution and stable across storage records, enabling rotation and revocation systems to refer to
keys without depending on mutable labels.

**DISP-CORE-0051 — identity-bound signature verification.**
`Crypto.ed25519_verify_keyed(expected_key_id, public_key, message, signature)` MUST be Pure, borrow
all four public `List<u8>` inputs, require exactly a 32-byte expected identifier, validate and derive
the actual identifier according to DISP-CORE-0050, compare the two identifiers without
content-dependent early exit, and perform strict Ed25519 verification only for the approved key.
A valid signature made by any different key MUST return `Ok(false)`. A matching approved key with
an altered message or invalid signature MUST also return `Ok(false)`. Malformed identifiers or
public keys and resource/provider failure MUST return `Err`. Messages MUST remain bounded by the
16 MiB Ed25519 limit, and interpreter/native outcomes MUST agree.

**DISP-CORE-0052 — deterministic key lifecycle enforcement.**
`Crypto.ed25519_verify_lifecycle` MUST extend DISP-CORE-0051 with unsigned `valid_from`,
`valid_until`, an explicit `revoked` state, and an unsigned caller-supplied `evaluation_time`.
The operation MUST be Pure and MUST NOT read an ambient wall clock. A window with
`valid_from > valid_until` MUST return `Err`. The expected identifier and public key MUST still be
structurally validated. A revoked key or an evaluation time before activation or after expiry MUST
return `Ok(false)` without performing signature verification. Both endpoints MUST be inclusive.
An active, unrevoked key MUST proceed through identity-bound strict verification. Interpreter and
native results MUST agree for active, premature, expired, revoked, malformed-window, wrong-key,
altered-message, and invalid-signature cases.

**DISP-CORE-0053 — external Ed25519 key-provider boundary.**
An external or hardware-backed Ed25519 signing key MUST be represented inside the host by a
non-cloning, debug-redacted, zeroizing opaque handle and an expected DISP-CORE-0050 key identifier;
private key bytes MUST NOT occur in the provider request or response protocol. Providers MUST run
out of process through the bounded `disp.component.v1` transport with a cleared environment. The
nested `disp.keystore.v1` protocol MUST bind every frame to its operation, use explicit big-endian
lengths, reject malformed reserved fields, truncation, trailing bytes, unknown operations, and
nonempty rejection payloads, and bound handles to 1,024 bytes and signing messages to the component
ceiling. The provider executable's content digest MUST be captured when the handle is opened and
rechecked immediately before each invocation. Public-key responses MUST contain exactly 32 bytes,
validate as a non-weak Ed25519 key, and match the pinned identifier without content-dependent early
exit. Signature responses MUST contain exactly 64 bytes and pass identity-bound strict verification
over the requested message before the signature is released. All ambiguity, mutation, provider
failure, identity mismatch, and invalid output MUST fail closed.
The provider SDK MUST expose only public-key lookup and signing callbacks whose inputs contain the
opaque handle and, for signing, the bounded public message. Provider failures MUST use a nonzero
status and MUST NOT carry diagnostic or secret payload bytes on the protocol output stream.

**DISP-CORE-0054 — fail-closed dependency advisory gate.**
Every committed Rust dependency graph used by the compiler, runtime, or cryptographic companion
MUST be locked and scanned against a freshly fetched RustSec advisory database on dependency
changes, pull requests that modify dependency policy, explicit dispatch, and a daily schedule. The
scanner version MUST be pinned and installed from its own lockfile. Vulnerabilities at low severity
or higher, informational unmaintained/unsound/notice advisories, yanked packages, stale advisory
data, scanner failure, and malformed policy MUST fail the gate. The committed policy MUST contain
no ignored advisory IDs. A release verdict MUST identify the exact lockfile and database state it
scanned; a prior clean result MUST NOT be treated as proof that the graph remains safe later.

**DISP-CORE-0055 — continuous trust-boundary fuzzing.**
The lexer, complete source frontend, canonical cryptographic record decoders, component frame
decoder, and external-keystore request/response decoders MUST have coverage-guided libFuzzer
targets. The fuzz tool version and fuzz dependency graph MUST be pinned. Linux CI MUST run every
target with compiler sanitizer instrumentation, a finite per-input timeout, and finite campaign
duration on dependency/security changes, pull requests, explicit dispatch, and the daily security
schedule. Security-frame campaigns MUST include committed protocol tokens or seeds so structured
headers are reachable without waiting for blind mutation. Every decoder MUST accept arbitrary byte
sequences without panic, undefined behavior, unbounded allocation, or unauthenticated secret
release; malformed data MUST return a controlled rejection.

**DISP-CORE-0056 — auditable release-binary dependency provenance.**
Release builds of the DISP compiler and bundled native cryptographic companion MUST embed a
machine-readable inventory of the exact Rust packages incorporated into each produced artifact.
The embedding tool and its own dependency graph MUST be version-pinned and locked. CI MUST build
release artifacts from the committed lockfile and scan each produced executable/shared library
with the same fail-closed advisory policy as DISP-CORE-0054. A lockfile-only scan MUST NOT substitute
for artifact scanning, and a release MUST fail if provenance is absent, malformed, inconsistent,
vulnerable, stale, or cannot be extracted. Native system libraries not represented by Rust package
metadata remain a separate SBOM obligation and MUST NOT be claimed as covered by this rule.

**DISP-CORE-0057 — security governance and unsafe-boundary inventory.**
The repository MUST publish a private vulnerability-reporting route, supported-version statement,
severity-based acknowledgement/containment/remediation targets, coordinated-disclosure process,
and explicit release-blocking vulnerability classes. It MUST maintain a versioned threat model
covering protected assets, adversaries and trust assumptions, every implemented external trust
boundary, current controls, residual risks, abuse cases, and release-candidate review cadence.
Compiler/runtime Rust unsafe code MUST remain confined to an explicit path inventory with local
`SAFETY:` rationale; CI MUST reject unsafe constructs outside that inventory and any suppression of
unsafe-code review. Inventory membership scopes mandatory review and MUST NOT be represented as
proof that a listed block is correct. Critical/high exploitable memory-safety, sandbox/capability,
cryptographic-authentication, signing-key, or in-process package-execution failures MUST block a
release until remediated or the affected artifact is withdrawn.

**DISP-CORE-0058 — sanitizer-backed Rust boundary regression gate.**
Linux CI MUST compile and execute the compiler library plus cryptographic, native-ABI, hostile-frame,
and security-governance regression suites with Rust AddressSanitizer instrumentation and leak
detection enabled. The nightly toolchain MUST be date-pinned, the committed lockfile MUST be
enforced, and all features MUST be checked for the explicit Linux target. Sanitizer findings,
memory leaks, aborts, toolchain/instrumentation unavailability, test failures, and timeouts MUST fail
the gate; CI MUST NOT silently retry without instrumentation. This gate complements rather than
replaces generated-C sanitizers, coverage-guided fuzzing, platform sandbox probes, or review of
unsafe boundary code.

**DISP-CORE-0059 — locked Rust and resolved-native release SBOMs.**
Linux release CI MUST generate deterministic CycloneDX 1.6 SBOMs for the compiler, native
cryptographic companion, and fuzzing tool graph directly from each committed Cargo lockfile using
`cargo metadata --locked`. Registry package archive checksums and the complete dependency graph
MUST be recorded. For each produced compiler/companion artifact, the generator MUST inspect the
actual dynamic-loader resolution, fail on unresolved libraries, record every resolved native
library with its SHA-256 file hash and available distribution package/version, and attach those
components to the root artifact dependency graph. Verification MUST reject missing/duplicate
references, dangling dependency edges, malformed hashes, absent native inventory, or host-path
leakage. Verified SBOMs MUST be retained as CI artifacts; Rust-only inventory MUST NOT be described
as complete native coverage. Other release platforms require equivalent native resolution before
their SBOMs can satisfy this rule.
Windows release CI MUST parse both normal and delay-load PE import directories without invoking the
artifact, resolve concrete imports from the artifact directory, System32, or the controlled runner
path, and attach file versions plus SHA-256 hashes. Windows API-set contracts that intentionally
have no standalone file MUST be typed as operating-system contracts and tied to the runner OS
build; any other unresolved import MUST fail. Windows compiler and companion artifacts MUST also
carry and pass advisory scanning of embedded Rust dependency provenance before SBOM publication.
macOS release CI MUST parse thin and universal Mach-O load commands without invoking the artifact,
cover ordinary, weak, lazy, re-exported, and upward dylib loads, expand controlled `@rpath`,
`@loader_path`, and `@executable_path` locations, and hash every resolved file. System install names
present only through Apple's dyld shared cache MUST be represented as operating-system components
tied to the macOS version and recorded Mach-O load version; unresolved non-system install names
MUST fail. Cross-platform synthetic parser tests MUST reject truncated command tables and verify
versioned dylib/rpath decoding before any platform SBOM is published.

**DISP-CORE-0060 — signed build provenance and SBOM attestation.**
Every non-pull-request desktop release-security job MUST request OIDC signing authority only at job
scope and MUST create Sigstore-backed GitHub attestations using the current `actions/attest` major
for: (a) the compiler, cryptographic companion, and published SBOM files as SLSA build-provenance
subjects; (b) the compiler bound to its verified platform CycloneDX SBOM predicate; and (c) the
companion bound to its verified platform SBOM predicate. Signing permissions MUST consist only of
read-only repository contents plus `id-token`, `attestations`, and `artifact-metadata` writes and
MUST NOT be granted to fuzzing, sanitizer, or pull-request execution. Attestation failure MUST fail
the publishing job. Consumers MUST verify artifact identity, repository, commit, and signing
workflow against GitHub's trust root; an attestation proves provenance, not artifact security.

**DISP-CORE-0061 — direct, fail-closed freestanding boot image.**
The `build --freestanding` target MUST validate source through the ordinary language safety pipeline
and then enforce its documented freestanding subset before emitting an artifact. The x86 BIOS
profile MUST require exactly one plain `fn main()` entry; capabilities, async, external ABIs,
unsupported declarations or values, and all other hosted constructs MUST fail at compile time and
MUST NOT trigger hosted fallback. A successful build
MUST directly and deterministically emit a sector-aligned disk image whose first 512-byte boot sector
has the `55 aa` signature, without
invoking a C compiler, assembler, linker, package manager, build script, OS runtime, allocator, or
libc for the produced program. Payload and encoding overflow MUST be rejected before artifact
mutation. Artifact replacement MUST stage and synchronize the complete image and preserve an older
destination if installation fails. CI MUST boot the example image in an emulator and verify its
exact observable output before Pass 021 can be completed.

**DISP-CORE-0062 — allocation-free freestanding integer execution.**
The initial computational freestanding profile MUST directly lower explicitly typed `u16` locals,
lexical lookup, assignment, arithmetic, comparisons, boolean short-circuiting, `if`, and `while` to
machine instructions without heap allocation or hosted calls. It MUST provide allocation-free
decimal `u16` output. Addition, subtraction, and multiplication overflow or underflow and division
or remainder by zero MUST enter a deterministic controlled-failure path before a wrapped or invalid
result is observed. Locals MUST have a finite compile-time count and fixed non-overlapping machine
storage. Numeric widths outside the current exact-width freestanding rules, unsupported statements,
heap-backed values, and unsupported dynamic calls MUST fail compilation rather than be narrowed,
interpreted, or linked to a hosted implementation.

**DISP-CORE-0063 — bounded deterministic freestanding stage loading.**
A freestanding program fitting the boot payload MUST remain a single-sector image. A larger program
MUST use a deterministic first-stage loader and a separately relocated, sector-padded machine-code
stage. The loader MUST preserve the firmware boot-drive identifier, use a structurally valid aligned
INT 13h extended-read packet starting at LBA 1, load the exact encoded sector count into a fixed
non-overlapping address, transfer through an explicit normalized far jump, and visibly fail then halt
if the read fails. The compiler MUST reject a stage whose rounded extent could cross its documented
real-mode address ceiling. CI emulator evidence MUST execute a genuinely multi-sector fixture and
verify output produced by runtime computation in the loaded stage.

**DISP-CORE-0064 — exact-width freestanding numeric execution.**
Freestanding locals MAY be explicitly typed `u16`, `u32`, `i32`, or `bool`; their machine storage
MUST match the declared width, remain aligned and non-overlapping, and share finite local-count and
byte limits. Unsigned addition/subtraction MUST reject carry/borrow, multiplication MUST reject a
nonzero high half, and division/remainder MUST reject zero. Signed addition/subtraction/negation MUST
reject overflow; signed multiplication MUST validate that the high half is the exact sign extension;
signed division/remainder MUST reject zero and `i32::MIN / -1` before executing the instruction.
Ordered comparisons MUST select signed or unsigned conditions from the operand type. The direct
backend MUST NOT implicitly mix widths or signedness, and unsupported widths MUST fail compilation.
Allocation-free output MUST correctly render the entire `u32` and `i32` domains, including zero and
`i32::MIN`, and booleans as `true` or `false`.

**DISP-CORE-0065 — allocation-free scalar function ABI.**
Freestanding helper functions MAY accept and return `u16`, `u32`, `i32`, and `bool`, or return
`Unit`; they MUST be synchronous, non-generic, capability-free, and implemented in DISP. Every
function label and parameter slot MUST be deterministic, aligned, fixed, and included in the shared
finite local-storage limits. Arguments MUST evaluate left-to-right into width-correct temporary stack
values and MUST be committed to callee slots only after all argument evaluation completes, in a way
that preserves pending outer-call arguments across nested calls. Calls MUST transfer directly and
scalar returns MUST use the width-correct accumulator. Forward calls MUST work. Calls to the `main`
entry MUST be rejected; calls MUST NOT silently allocate heap frames or delegate to a hosted runtime.

**DISP-CORE-0066 — structured freestanding loop control.**
The freestanding backend MUST lower `while` and indefinite `loop` bodies to deterministic structured
branches. `break` and `continue` MUST bind to the innermost lexical loop. A `while` continue edge MUST
re-evaluate its condition, an indefinite-loop continue edge MUST target its body head, and a break
edge MUST target the unique continuation after that loop. Nested loops MUST maintain independent
targets. Invalid loop control MUST fail during the ordinary safety pipeline; safe source MUST NOT
gain arbitrary labels, computed jumps, or a way to branch into another lexical scope.

**DISP-CORE-0067 — guarded recursive freestanding frames.**
Before a scalar helper call commits arguments, the backend MUST preserve every parameter and local
slot owned by that callee on the machine stack, and after return MUST restore those slots in reverse
order without changing the width-correct return value. This rule MUST hold for direct recursion,
mutual recursion, forward calls, nested argument calls, and lexical locals in conditional or loop
blocks. Before reserving a frame, the generated program MUST compare the live stack pointer against
a deterministic lower bound that includes the callee snapshot, pending arguments, return address,
and an expression reserve. Exhaustion MUST enter a defined diagnostic-and-halt path before crossing
the protected stack floor; it MUST NOT corrupt fixed local storage or silently wrap the stack.

**DISP-CORE-0068 — exact byte-width freestanding execution.**
The freestanding backend MUST accept `u8` literals, explicitly typed locals, parameters, and return
values. A `u8` fixed-memory slot MUST occupy exactly one byte, while temporary argument and frame
snapshot values MUST use a width supported by the target machine stack and MUST be accounted at that
actual stack width by the exhaustion guard. Loads MUST zero-extend before computation. Stores MUST
write exactly one byte. Addition, subtraction, multiplication, division, and remainder MUST reject
overflow, underflow, and zero divisors before an invalid result escapes. Comparisons, calls, returns,
and decimal output MUST preserve the unsigned byte value without implicit mixing with other widths.

**DISP-CORE-0069 — direct flat 32-bit protected-mode bootstrap.**
`build --freestanding32` MUST run the ordinary DISP safety pipeline and then enforce its documented
initial protected-mode subset without falling back to hosted code generation. It MUST directly emit
a deterministic signed BIOS boot sector without invoking C, an assembler, linker, operating system,
allocator, libc, or language runtime. The bootstrap MUST normalize real-mode segments, request A20,
verify A20 with a reversible low/high alias probe, load a deterministic GDT containing null plus
flat 4 GiB 32-bit code and data descriptors, set
`CR0.PE`, and enter the code selector through a far control transfer. Protected code MUST load every
data segment from the flat data selector and initialize a 32-bit stack below the VGA/firmware region.
After entering protected mode it MUST NOT invoke BIOS services. Initial constant ASCII output MUST
write directly to VGA text memory and mirror exact CRLF output to debug port `0xe9`. Unsupported
source MUST fail compilation, and direct-payload overflow MUST follow DISP-CORE-0070 rather than
truncate code. Output installation MUST retain the transactional freestanding write guarantee.

**DISP-CORE-0070 — bounded relocated protected-mode stages.**
A protected32 program fitting 510 payload bytes MUST remain exactly one signed boot sector. A larger
program MUST use the same bounded deterministic EDD first-stage contract as the freestanding target,
load contiguous sectors at `0x7e00`, and regenerate every protected-mode absolute address for that
stage origin, including the GDTR operand, GDT base, and protected far-transfer target. The disk image
MUST be sector-padded and deterministic. The compiler MUST reject a stage exceeding 64 sectors before
artifact replacement so neither the loaded stage nor its descriptors can cross the safe real-mode
address ceiling. Loader failure MUST remain visible and halted; it MUST NOT enter a partial stage.

**DISP-CORE-0071 — checked protected32 scalar execution.**
The protected32 backend MUST directly lower explicitly typed `u32` and `bool` locals into bounded,
deterministic four-byte slots beginning at physical address `0x100000`, beyond the real-mode address
ceiling. It MUST verify A20 before any such access and MUST halt visibly without entering protected
code if the verification fails; the alias probe MUST restore both touched bytes on every outcome.
Loads, stores, literals, mutation, boolean negation and short circuiting, unsigned comparisons,
`if`/`else`, `while`, indefinite `loop`, lexical `break`/`continue`, and empty return MUST execute in
the flat 32-bit code segment. Unsigned addition/subtraction MUST reject carry/borrow, multiplication
MUST reject a nonzero high half, and division/remainder MUST reject zero before executing. Runtime
unsigned integers and booleans MUST render through allocation-free 32-bit routines to both VGA and
port `0xe9`, with exact CRLF line endings. Unsupported widths and statements MUST fail compilation;
checked operations MUST NOT silently wrap or delegate to a hosted runtime.

**DISP-CORE-0072 — compact and signed protected32 widths.**
The protected32 backend MUST extend exact scalar execution to `u8`, `u16`, and `i32`. Fixed `u8`
slots MUST occupy one byte, `u16` slots MUST occupy two bytes at two-byte alignment, and `i32` slots
MUST occupy four bytes at four-byte alignment; mixed declarations MUST NOT be inflated to uniform
words or overlap. Narrow loads MUST zero-extend before computation and narrow stores MUST write only
their declared width. Unsigned narrow arithmetic MUST reject any result above the type maximum and
MUST reject subtraction underflow and zero divisors. Signed addition/subtraction MUST reject signed
overflow; multiplication MUST verify that the high half exactly sign-extends the low result; division
and remainder MUST reject zero and `i32::MIN / -1` before execution. Ordered comparisons MUST select
signed or unsigned conditions from the operand type. Signed output MUST render zero, both signs, and
the complete `i32` domain including `i32::MIN` without allocation or undefined negation. Protected32
MUST NOT implicitly mix widths or signedness.

**DISP-CORE-0073 — guarded recursive protected32 scalar functions.**
Protected32 helper functions MAY accept and return `u8`, `u16`, `u32`, `i32`, and `bool`, or return
`Unit`; they MUST be synchronous, non-generic, capability-free, directly implemented in DISP, and
bounded by the documented function/local limits. The compiler MUST assign deterministic forward
labels and pre-inventory every parameter and lexical local in each complete function frame before
emission. At a call, it MUST guard `ESP`, snapshot every fixed callee slot as a 32-bit machine-stack
word, evaluate arguments left-to-right into 32-bit pending words, commit parameters in reverse, and
transfer with a direct relative call. After return it MUST preserve the scalar result, restore every
callee slot in reverse, and then reinstate the result. This contract MUST preserve nested arguments,
forward calls, direct recursion, mutual recursion, and compact scalar values without heap allocation.
The guard MUST include the callee snapshot, arguments, four-byte return address, and expression
reserve, and MUST prevent `ESP` from crossing `0x80000`. Exhaustion MUST print
`protected32 stack limit exceeded` and halt before corruption. Calls to `main` MUST fail compilation.

**DISP-CORE-0074 — bounded protected32 fixed arrays.**
Protected32 local bindings MAY use fixed arrays whose element type is `u8`, `u16`, `u32`, `i32`, or
`bool`. Their length MUST be a compile-time constant, every declaration MUST have an exact-length
array-literal initializer, and element storage MUST be contiguous at the element type's alignment.
Each element MUST consume one bounded local slot and MUST participate independently in complete
callee-frame snapshots, including direct and mutual recursion. Array parameters and returns are not
part of this rule. Reads, direct writes, and checked compound writes MUST accept integer indices.
Before scaling the index or performing any memory access, generated code MUST compare it against the
declared length and branch on every value greater than or equal to that length. This unsigned check
MUST also reject negative `i32` indices. Failure MUST print `protected32 index out of bounds` and halt
without accessing the array. Element loads and stores MUST preserve the exact compact width, and
compound arithmetic MUST retain the element type's existing overflow, underflow, division, and
signedness rules.

**DISP-CORE-0075 — capability-gated protected32 port I/O.**
`DeviceIo` MUST be a distinct capability identity and MUST NOT be implied by `RawMemory`, `Foreign`,
or a bare unsafe region. Protected32 MAY expose `Port.read_u8(port: u16) -> u8` and
`Port.write_u8(port: u16, value: u8) -> Unit`; every such operation MUST be lexically contained by
at least one explicit `unsafe uses DeviceIo` contract, and every explicit enclosing unsafe contract
MUST contain `DeviceIo`. The type checker MUST reject other widths, arities, operations, and absent
or unrelated authority before lowering. Direct use and explicit unsafe contracts MUST contribute
`DeviceIo` to the containing function's effect contract, and ordinary direct-call propagation MUST
prevent callers from hiding that authority. Protected32 lowering MUST evaluate arguments
left-to-right, place the exact `u16` port in `DX`, use x86 variable-port `IN`/`OUT` byte instructions,
and zero-extend input bytes to the exact DISP `u8` value representation. No hosted runtime, BIOS
service, implicit port allowlist, or ambient authority is permitted. Hosted native compilation and
interpreter execution MUST reject port intrinsics before code generation or program execution.

**DISP-CORE-0076 — protected32 exception-table foundation.**
Before transferring to `main`, protected32 MUST construct and load an IDT covering architectural
exception vectors 0 through 31. The table MUST occupy a fixed bounded region disjoint from the
complete local arena and machine stack. Each entry MUST be a present DPL0 32-bit interrupt gate
using the protected code selector and a deterministic absolute handler address. The IDTR limit MUST
cover exactly those 32 entries. The initial common handler MUST NOT return: it MUST disable maskable
interrupts, restore known flat data/stack segments, reset the stack and output cursor to documented
addresses, print `protected32 CPU exception`, and halt. It MUST be valid for both exceptions that do
and do not push an error code because it never consumes or returns through the interrupted frame.
Protected user code MUST begin only after `LIDT`. This rule does not enable external interrupts,
define interrupt-controller acknowledgement, or expose user handlers.

**DISP-CORE-0077 — bounded protected32 paging foundation.**
After loading the exception IDT and before executing user code, protected32 MUST zero a page-aligned
page directory and first page table in fixed regions disjoint from locals, IDT, and stack. Exactly
PDE 0 MUST be present initially. PTE 0 MUST remain non-present; PTEs 1 through 1023 MUST identity-map
linear/physical pages `0x1000` through `0x3ff000` as supervisor pages. The complete possible relocated
loader/stage envelope from `0x7000` through `0xffff` MUST be read-only; other initially mapped pages
MAY remain writable. All linear addresses
at or above 4 MiB MUST therefore remain non-present. This mapping MUST cover the relocated boot stage,
GDT/IDTR data, VGA text memory, guarded stack, exact local arena, IDT, directory, and first table.
The backend MUST load the directory through `CR3`, set `CR0.PG` and `CR0.WP` without clearing protected mode, and
serialize instruction fetch before entering `main`. The IDT MUST already be live so a paging fault
enters the defined exception handler. This foundation does not yet claim execute-disable protection,
user privilege, demand paging, per-component address spaces, or PAE.

**DISP-CORE-0078 — direct bounded x86-64 long-mode foundation.**
`build --freestanding64` MUST run the ordinary DISP safety pipeline, enforce its documented initial
source subset, and transactionally emit a deterministic signed BIOS disk image without invoking a C
compiler, assembler, linker, hosted runtime, OS service, allocator, or BIOS after protected-mode
entry. The bootstrap MUST reversibly verify A20, prove CPUID availability through EFLAGS.ID, verify
the extended-leaf ceiling and long-mode feature bit, and visibly halt with `L` when unsupported. It
MUST zero all four complete page-hierarchy pages before use, leave page zero absent, identity-map only
the documented first-2-MiB range with 4 KiB pages, and mark the complete loader/stage envelope
read-only. It MUST set `CR4.PAE`, load `CR3`, set `IA32_EFER.LME`, then set `CR0.PG|CR0.WP` before a
far transfer through a GDT descriptor with `L=1,D=0`. Long-mode code MUST establish a known stack,
construct and load exactly 32 present DPL0 sixteen-byte exception gates, then enter `main`. Its
initial handler MUST report `x86-64 CPU exception` and halt without returning. Output MUST use direct
VGA/debug-port access and exact CRLF endings. This foundation alone is not evidence of wider
protected32 parity; later rules define added source features, and unsupported constructs MUST fail
compilation explicitly.

**DISP-CORE-0079 — checked x86-64 scalar execution.**
The x86-64 profile MUST support explicitly typed `u8`, `u16`, `u32`, `i32`, and `bool` locals in a
plain `main`, initialized bindings, direct and compound assignment, checked arithmetic, comparisons,
short-circuit Boolean operations, structured `if`/`while`/`loop` control flow, `break`, `continue`,
empty return, and typed output. Locals MUST be confined to the documented writable 4096-byte arena at
`0x105000`; allocation beyond that arena MUST fail compilation. Generated long-mode memory operands
MUST use unambiguous absolute encodings, and expression temporaries MUST use balanced 64-bit stack
operations. Overflow, invalid signed division, and division by zero MUST branch to the non-returning
`x86-64 arithmetic failure` path instead of invoking machine undefined behavior. Unsupported inferred
locals, values, calls, aggregates, and statements MUST fail explicitly. Linux QEMU CI MUST boot a
representative scalar artifact and compare its complete debug-port output byte-for-byte.

**DISP-CORE-0080 — guarded x86-64 scalar functions.**
The x86-64 profile MUST accept a bounded set of synchronous, non-generic, non-external,
capability-free functions whose parameters and returns are exact supported scalars or `Unit`.
Forward, nested, recursive, and mutually recursive direct calls MUST preserve isolated parameter and
local values. Before evaluating arguments, each call MUST compare `RSP` against the documented stack
floor plus expression reserve and the complete pending frame requirement. It MUST snapshot all
callee slots as 64-bit stack words, evaluate arguments left-to-right, commit parameters in reverse,
call a fixed relative target, preserve a scalar return, and restore all prior slots in reverse.
Every push MUST have a matching pop on the returning path. A call that would cross the stack floor
MUST report `x86-64 stack limit exceeded` and halt before committing the frame. Calling `main`, using
an unsupported signature, or exceeding the function/local bounds MUST fail compilation. Linux QEMU
CI MUST verify exact output for nested recursion, mutual recursion, and deliberate stack exhaustion.

**DISP-CORE-0081 — checked x86-64 fixed arrays.**
The x86-64 profile MUST support bounded fixed arrays of its exact scalar types with literal
initialization, indexed reads, direct element assignment, and checked numeric compound assignment.
Element storage MUST use exactly one, two, or four bytes according to its scalar type and MUST remain
inside the shared 4096-byte local arena. Every dynamic index MUST be evaluated once, compared
unsigned against the declared length, and scaled by the exact element width before any access.
Out-of-range access MUST report `x86-64 index out of bounds` and halt before reading or writing the
element. Indexed long-mode operands MUST combine only the checked offset and compiler-assigned array
base. Every array element MUST participate in function frame snapshot/restore so recursive calls
cannot corrupt a caller's array. Linux QEMU CI MUST verify both recursive array computation and the
deliberate bounds-failure path byte-for-byte.

**DISP-CORE-0082 — capability-gated x86-64 device I/O.**
The x86-64 profile MUST expose byte port input and output only through `Port.read_u8(u16) -> u8` and
`Port.write_u8(u16, u8) -> Unit`. Each operation MUST require a lexically enclosing explicit
`unsafe uses DeviceIo` contract, and the containing function's effect contract MUST include the
distinct `DeviceIo` capability. Legacy implicit unsafe blocks, missing authority, wrong authority,
wrong widths, and unknown port operations MUST fail before image generation. Authorized reads and
writes MUST lower directly to `in al,dx` and `out dx,al` with balanced 64-bit temporary preservation;
they MUST NOT call BIOS or a hosted runtime. Linux QEMU CI MUST verify the exact debug-port output of
an authorized input/output fixture.

**DISP-CORE-0083 — x86-64 execute-whitelist paging.**
The x86-64 bootstrap MUST require both the long-mode and execute-disable CPUID feature bits before
constructing its paging state. Every present 4 KiB leaf PTE MUST initially carry NX. Only the complete
bounded loader/stage envelope from `0x7000` through `0xffff` MAY have NX cleared, and those executable
pages MUST remain supervisor read-only. The null page MUST remain non-present; stack, VGA, paging
hierarchy, IDT, and local-arena pages MUST remain NX. The bootstrap MUST set
`IA32_EFER.LME|IA32_EFER.NXE` before enabling `CR0.PG|CR0.WP`, and it MUST visibly halt through the
unsupported-machine path if NX is absent. Generated images MUST therefore enforce a bounded
write-or-execute policy rather than relying on source validation alone. Structural tests MUST verify
the CPUID mask, NX-by-default PTE high words, executable-stage overrides, and EFER bits; Linux QEMU
boot gates MUST continue proving that whitelisted code executes successfully.

**DISP-CORE-0084 — differentiated x86-64 critical-fault routing.**
The x86-64 profile MUST install distinct present DPL0 sixteen-byte interrupt gates for invalid opcode
(vector 6), general protection (vector 13), and page fault (vector 14) before loading its IDTR and
entering user code. Each gate MUST select the 64-bit code segment and use interrupt-gate attributes.
Every dedicated handler MUST disable interrupts, restore the documented compiler-owned stack and
output cursor, emit respectively `x86-64 invalid opcode`, `x86-64 general protection`, or
`x86-64 page fault`, and halt without returning through or consuming the interrupted frame. All
remaining entries in the bounded first-32-vector table MUST retain the common non-returning handler.
This foundation does not enable external interrupts or expose user handlers. Structural evidence
MUST verify the three exact IDT slots, gate construction,
distinct stable diagnostics, and IDTR load; existing Linux QEMU boot gates MUST continue proving the
table does not disrupt valid execution.

**DISP-CORE-0085 — quarantined x86-64 legacy interrupt controller.**
The x86-64 profile MUST keep the interrupt flag clear and extend its IDT with present DPL0 interrupt
gates for vectors 32 through 47 before entering user code. After loading the complete 48-entry IDTR,
compiler-owned bootstrap code MUST initialize both legacy 8259 PICs in 8086 mode, remap the master to
vectors 32–39 and slave to vectors 40–47, declare the IRQ2 cascade relationship, and mask every line
by writing `0xff` to both data ports. PIC command/data writes MUST use exact architectural byte-output
instructions with deterministic I/O delays. All 16 IRQ gates MUST target one known-state handler that
disables interrupts, restores the documented stack and output cursor, issues non-specific EOI to the
slave and then master, reports `x86-64 unexpected hardware interrupt`, and halts without returning.
Except for the capability-controlled extension in DISP-CORE-0086, the profile MUST NOT execute
`STI`, selectively unmask a line, or expose source-level interrupt handlers. Structural evidence MUST decode the IRQ gate block and IDTR limit/base, match the complete
PIC initialization sequence, verify the handler acknowledgement prefix and diagnostic, and prove
artifact determinism. Linux QEMU exact-output gates MUST continue proving ordinary execution after
controller quarantine. This rule does not specify APIC operation.

**DISP-CORE-0086 — capability-controlled fixed-rate timer.**
`Timer` MUST be a distinct capability identity. `Time.ticks()` MUST take no arguments, return a
wrapping `u32` monotonic counter in fixed 10 millisecond units, and contribute `Timer` to whole-program
effect inference and explicit-contract checking. Hosted interpreter and native execution MUST use a
monotonic provider. The x86-64 freestanding backend MUST additionally require the function directly
containing `Time.ticks()` to declare `uses Timer`; inferred authority alone MUST fail before image
generation. If no such declaration exists, DISP-CORE-0085 remains unchanged. If at least one exists,
the bootstrap MUST clear one naturally aligned counter on a writable NX page, replace vector 32 with
a dedicated DPL0 interrupt gate, program PIT channel 0 in square-wave low/high-byte mode with divisor
11932, unmask only master IRQ0, keep the complete slave PIC masked, and execute `STI` only after the
IDT, PIT, counter, and masks are ready. The timer handler MUST preserve every general register it
touches, increment the counter exactly once, acknowledge only the master PIC, and return with
`iretq`; interrupt-gate semantics MUST prevent nested maskable IRQ delivery. `Time.ticks()` MUST lower
to one aligned 32-bit load. Structural tests MUST decode the vector override, PIT setup, counter
initialization/load, masks, `STI`, ISR, and deterministic image. Linux QEMU CI MUST boot a fixture
that waits for the counter to advance before printing a fixed success line. This rule exposes neither
arbitrary source handlers nor any IRQ other than IRQ0 and does not specify APIC timers.

**DISP-CORE-0087 — direct deterministic AArch64 virt image.**
`build --freestanding-aarch64` MUST run the ordinary DISP safety pipeline and initially accept one
plain `fn main()` containing bounded literal-string print statements. It MUST accept UTF-8, reject
embedded NUL and non-literal output, cap combined text plus line endings at 65,536 bytes, and reject
every unsupported construct without invoking a hosted backend. The output MUST be a deterministic
little-endian Arm64 Image for the versioned QEMU `virt-8.2`/`cortex-a53` contract. Its 64-byte header
MUST begin with a direct A64 branch, use text offset `0x80000`, encode its exact padded image size,
select little-endian 4 KiB placement, and contain magic `0x644d5241`. The payload MUST directly encode
PC-relative message/literal acquisition, post-increment byte reads, PL011 UARTFR TX-full polling,
UARTDR writes through the pinned `0x09000000` UART0 mapping, and a non-returning `wfi` loop. It MUST
not invoke an assembler, linker, C compiler, firmware service, OS, allocator, libc, or language
runtime. Encoding helpers MUST check alignment, signed displacement range, and arithmetic overflow;
artifact installation MUST use the transactional writer. Unit evidence MUST verify header fields,
instruction words, device literal, deterministic UTF-8 data, and fail-closed profile/size validation.
CLI evidence MUST verify the single named artifact. Linux CI MUST boot the artifact with
`qemu-system-aarch64 -machine virt-8.2 -cpu cortex-a53` and compare exact serial bytes. This rule does
not claim the unversioned QEMU board, DTB device discovery, physical hardware, MMU/exception setup,
or general AArch64 computation except where a later DISP-CORE rule explicitly extends the profile.

**DISP-CORE-0088 — checked AArch64 `u32`/`bool` computation and structured control flow.**
The direct AArch64 profile MUST extend DISP-CORE-0087 with initialized, explicitly annotated, owned
`u32` and `bool` locals held only in compiler-assigned static slots. It MUST lower lexical bindings,
simple and compound assignment, valueless main return, `if`/`else`, `while`, indefinite `loop`,
`break`, and `continue`; loop transfers MUST bind to the innermost enclosing loop. It MUST lower
`u32` addition, subtraction, multiplication, division, remainder, all unsigned comparisons, boolean
equality/inequality and negation, and short-circuit `&&`/`||`. Addition overflow, subtraction
underflow, a nonzero high half of widened multiplication, and zero division/remainder MUST branch to
one known-state diagnostic path that prints `[DISP arithmetic fault]` and never returns. Comparisons
and boolean operators MUST produce canonical zero-or-one values. Scalar data accesses MUST be
32-bit aligned loads/stores relative to a PC-derived image-local base; at most 4,096 slots, 65,536
bytes of terminated static strings, and a 262,144-byte complete image may be emitted. Branch, ADR,
and literal-load fixups MUST validate alignment and signed reach before artifact creation. The
backend MUST remain independent of a stack, assembler, linker, OS, allocator, libc, and language
runtime. Non-literal printing, signed/compact widths, calls, arrays, and every other unsupported
construct MUST fail closed unless a later DISP-CORE rule explicitly extends it. Unit evidence MUST identify the widened multiply, high-half check,
checked subtraction, comparisons, deterministic header/data, and rejection bounds. Linux QEMU CI
MUST execute a scalar/control fixture to its exact success line and an overflowing fixture to the
exact diagnostic line.

**DISP-CORE-0089 — exact compact/signed AArch64 scalars and direct scalar output.**
The AArch64 profile MUST extend DISP-CORE-0088 with `u8`, `u16`, and `i32`, giving all five exact
scalar types the same binding, assignment, expression, comparison, and control-flow surface. `u8`
and `u16` locals and temporaries MUST use aligned one- and two-byte image-local storage and zero-
extending loads; `u32`, `i32`, and `bool` remain aligned four-byte values. Total scalar storage MUST
not exceed 4,096 bytes so every fixed-base unsigned-immediate access is encodable. Unsigned compact
addition and multiplication MUST reject results above the exact type maximum, and subtraction MUST
reject borrow. Signed addition, subtraction, and unary negation MUST reject V-flag overflow. Signed
multiplication MUST widen to 64 bits and reject unless the complete result equals the sign extension
of its low 32 bits. Signed division and remainder MUST reject zero and the `i32::MIN / -1` pair before
executing `SDIV`; all signed ordered comparisons MUST use signed A64 conditions. Integer literals
MUST be range-checked against their contextual exact type, including acceptance of `-2147483648`.
`print` MUST accept all exact scalars: booleans emit canonical `true`/`false`, unsigned values emit
base-10 digits, and signed values emit one leading minus followed by the unsigned magnitude. Decimal
conversion MUST use a single bounded 16-byte image-local buffer, handle zero and the full `i32`
range, append CRLF, and perform output only through the existing polled PL011 path. This scalar-output
machinery MUST require no stack; later rules MAY add an independently guarded stack. No allocator,
external tool, OS, or hidden runtime may be introduced. Structural evidence MUST identify
compact load/store encodings, signed widen/sign-extension checks, signed division, and decimal
conversion. Linux QEMU CI MUST compare the exact output of a fixture covering maximum compact values,
negative and minimum signed values, and boolean output; the arithmetic-fault fixture MUST exercise
signed overflow.

**DISP-CORE-0090 — guarded AArch64 functions and recursive frame preservation.**
The direct AArch64 profile MUST accept multiple plain, non-generic, synchronous, capability-free,
non-external functions with exact-scalar parameters and exact-scalar or `Unit` results. `main` MUST
remain the unique uncallable parameterless `Unit` entry point. Direct calls MUST validate existence,
arity, argument types, result use, and return shape before emission. The image entry MUST derive a
16-byte-aligned stack top and floor with PC-relative addresses, install `sp` explicitly, and reserve
exactly 16 KiB of zero-initialized image-owned stack storage. Before every push, generated code MUST
compare `sp` against the floor and branch without mutating memory when no complete 16-byte slot
remains. Stack exhaustion MUST print `[DISP stack exhausted]` through the stack-independent PL011
path and halt without returning. Non-main functions MUST save and restore `x30` around direct
`BL`/`RET`. Before a call, every compiler-assigned parameter/local slot belonging to the callee MUST
be snapshotted in 16-byte stack slots; arguments MUST then be evaluated left-to-right, installed in
their exact-width parameter slots, and the prior frame restored in reverse order after return.
Expression temporaries MUST also use guarded stack slots so nested and recursive calls cannot
overwrite them. A scalar result MUST survive frame restoration. Fallthrough from a value-returning
function MUST fail closed; a `Unit` function MAY return implicitly. Structural evidence MUST identify
stack installation/floor comparison, guarded pre-index stores, post-index restores, `BL`, saved link
register, and `RET`. Deterministic runtime evidence MUST execute recursive factorial with mixed-width
parameters/`Unit` calls and MUST drive unbounded recursion to the exact stack-exhaustion diagnostic.
Linux QEMU CI MUST compare both outputs byte-for-byte. This rule does not add indirect calls,
closures, aggregate parameters/results, tail-call guarantees, or concurrency.

**DISP-CORE-0091 — bounded AArch64 fixed arrays and checked indexing.**
The direct AArch64 profile MUST accept local fixed arrays whose owned element type is exactly `u8`,
`u16`, `u32`, `i32`, or `bool`. Every binding MUST have an explicit fixed length and an array-literal
initializer with exactly that many elements of the annotated type. Elements MUST occupy contiguous,
naturally aligned, exact-width compiler-owned storage, and all elements together with scalar locals
MUST remain within the existing 4,096-byte limit. Every element of every callee-local array MUST be
included in the guarded frame snapshot required by DISP-CORE-0090 so recursion cannot overwrite the
caller's array state. Only direct local arrays MAY be indexed; indexing scalars, array parameters or
results, slices, nested aggregates, and indirect places MUST fail closed.

Each index MUST be evaluated as an exact integer and compared unsigned with the declared length
before element-address calculation, memory access, or assignment right-hand-side evaluation. This
comparison MUST reject both `index == length` and values above the length, including a negative
`i32` interpreted as unsigned. A valid element address MUST be the compiler-owned local base plus the
index scaled by the element width: zero, one, or two address-shift bits for one-, two-, or four-byte
elements. Loads and stores MUST use byte, halfword, or word instructions matching that width. Simple
indexed assignment and indexed compound assignment MUST be supported; compound arithmetic MUST use
the same exact-type overflow, underflow, division, and remainder checks as scalar assignment and
MUST preserve a once-evaluated index across right-hand-side evaluation. An invalid index MUST print
exactly `[DISP index out of bounds]\r\n` through the stack-independent PL011 path and enter the
non-returning halt loop before any invalid access or right-hand-side effect. Structural evidence MUST
identify the unsigned length branch, scaled address forms, exact-width loads/stores, guarded recursive
array snapshots, and deterministic image. Runtime evidence MUST cover all five element types,
dynamic indexed compound assignment, recursive array preservation, and the exact bounds diagnostic.
Linux QEMU CI MUST compare the success and bounds-failure fixtures byte-for-byte.

**DISP-CORE-0092 — current-level AArch64 exception containment.**
The direct AArch64 image MUST mask debug, system-error, IRQ, and FIQ exceptions before initializing
compiler state. It MUST read `CurrentEL`, accept execution at EL1 or EL2, install the image-owned
vector-table address into the matching `VBAR_EL1` or `VBAR_EL2`, execute an instruction-synchronization
barrier, and fail closed with exactly `[DISP unsupported exception level]\r\n` at any other exception
level. The table MUST be aligned to 2,048 bytes in the complete loaded image and MUST contain all
sixteen architectural entries. Each entry MUST occupy exactly 128 bytes and begin with a direct branch;
the four origin groups MUST separately route synchronous, IRQ, FIQ, and system-error classes to four
distinct handlers.

Every exception handler MUST be stack-independent, print exactly one class-specific diagnostic through
the already initialized polled PL011 path, and enter the common non-returning `wfi` loop. The exact
diagnostics MUST be `[DISP synchronous exception]\r\n`, `[DISP IRQ exception]\r\n`,
`[DISP FIQ exception]\r\n`, and `[DISP system error exception]\r\n`. No handler may resume execution,
execute `ERET`, depend on a valid interrupted stack, or silently conflate an architectural exception
with a checked language fault. The compiler MUST reserve a deterministic NOP fault-injection checkpoint
at complete-image byte offset 128 immediately after VBAR installation. Structural evidence MUST verify
DAIF masking, EL1/EL2 selection, both VBAR writes, `ISB`, complete table alignment/stride, four distinct
branch targets, deterministic output, and all diagnostics. Linux QEMU CI MUST boot the unmodified
fixture to its exact readiness line, replace only the checkpoint in a copied artifact with `BRK`, and
compare the resulting synchronous-exception diagnostic byte-for-byte. This rule does not enable
interrupt sources, exception recovery, syndrome/context reporting, MMU protection, or writable-table
protection; later rules MUST add those independently.

**DISP-CORE-0093 — sparse AArch64 stage-1 W^X translation.**
For the pinned QEMU `virt-8.2` Linux-Image placement at physical address `0x40080000`, the direct
AArch64 target MUST emit five image-owned 4 KiB translation tables implementing a bounded 32-bit,
three-level, identity-mapped TTBR0 regime. Before DISP-CORE-0094 runtime discovery, the level-1 root
MUST contain only the image branch, leading through sparse level-2/level-3 tables to exactly the pages
occupied by the complete DISP image; the reserved device level-2/level-3 tables MUST be empty. A later
rule MAY add one validated device branch before translation activation. Every other entry MUST remain
invalid. The complete image, its executable/data boundary, the page-table
region, every translation table, and the final artifact MUST be 4 KiB aligned where required; the
image MUST remain inside one bounded 2 MiB level-3 window and the existing 256 KiB image limit.

`MAIR_ELx` MUST define Attr0 as normal write-back/write-allocate memory and Attr1 as Device-nGnRnE.
The EL1 path MUST program a 4 KiB, inner-shareable, 32-bit TTBR0 regime with TTBR1 disabled; the EL2
path MUST program the corresponding 4 KiB, inner-shareable, 32-bit regime. Both paths MUST install
the root in `TTBR0_ELx`, issue full barriers and the current-level all-entry TLB invalidation, preserve
the existing `SCTLR_ELx` value, set `M`, `C`, `I`, and `WXN`, and execute an `ISB` after enabling
translation. The currently executing physical addresses MUST remain identical valid virtual addresses
through the transition.

All header, generated-code, and exception-vector pages MUST be privileged read-only executable normal
memory. Compiler data, static strings, exact locals, decimal storage, padding, and the guarded stack
MUST be privileged read-write execute-never normal memory. All five page-table pages MUST be privileged
read-only execute-never normal memory after activation. Any PL011 page added under DISP-CORE-0094 MUST
be privileged read-write, execute-never Device-nGnRnE memory. No valid page may be both writable and
executable. A synchronous
exception with current-level data-abort class `0x25` MUST print exactly
`[DISP memory protection fault]\r\n` and halt without returning; other synchronous exceptions retain
the DISP-CORE-0092 diagnostic.

The image MUST reserve a NOP at complete-image byte offset 280 after translation activation while
`x28` holds the address of an executable page. Structural evidence MUST decode the root-table ADR,
walk all emitted descriptors, prove the exact sparse mappings and page attributes, identify both
EL1/EL2 MAIR/TCR/TTBR/TLBI/SCTLR sequences, and verify deterministic output. Linux QEMU CI MUST boot
the unmodified fixture to its exact readiness line, then replace only that checkpoint in a copied
artifact with `STR WZR,[X28]` and compare the memory-protection diagnostic byte-for-byte. This rule
does not add virtual relocation, address randomization, EL0, demand paging, heap allocation, DTB
discovery, SMP coordination, or page-permission mutation.

**DISP-CORE-0094 — bounded AArch64 boot-hardware discovery.**
This rule supersedes DISP-CORE-0087's fixed UART literal and pre-mapped UART requirements.
Before installing `VBAR_ELx`, enabling translation, or performing any MMIO, the direct AArch64 target
MUST consume the flattened device tree pointer supplied in `x0`. The pointer MUST be nonzero and
eight-byte aligned. The parser MUST accept only a complete big-endian FDT with magic `0xd00dfeed`, a
total size from 40 bytes through 2 MiB, version at least 17, last-compatible version at most 17, and
structure/string blocks whose overflow-checked ranges lie completely within that total size. Every
structure token, aligned value range, node name, property-name string, and compatible-list string
MUST be bounded before it is read. Unknown well-formed properties and `FDT_NOP` tokens MAY be skipped;
unknown structure tokens, unterminated strings, arithmetic wrap, an unbalanced tree, or depth beyond
64 MUST fail closed.

This initial board schema MUST require root `#address-cells = <2>` and `#size-cells = <2>` and inspect
only direct root children. A PL011 candidate MUST contain a NUL-separated `compatible` value with one
exact `arm,pl011` member and a `reg` value of at least one 16-byte 64-bit-address/64-bit-size tuple. A
RAM candidate MUST have the exact seven-byte `device_type = "memory"` value and the same `reg` tuple
shape. The first tuple is authoritative for this bounded profile. Exactly one PL011 candidate MUST
resolve to a nonzero 4 KiB-aligned address no greater than `0xffffffff`, with at least 4 KiB of extent,
and it MUST NOT occupy the level-1 root entry reserved for the image. At least one RAM candidate MUST
contain every byte of the complete loaded DISP image; range addition MUST not wrap. Duplicate valid
PL011 candidates, missing required candidates, malformed recognized properties, unsupported cell
counts, or incomplete image containment MUST fail closed.

After validation and while the MMU remains disabled, the prelude MUST place the discovered PL011 base
in `x20` and patch the three reserved sparse descriptors selected by its level-1, level-2, and level-3
indices. The leaf MUST be privileged read-write, execute-never Device-nGnRnE; all other mappings and
W^X invariants from DISP-CORE-0093 remain unchanged. Static artifact construction MUST NOT embed or
pre-map the historical QEMU UART base. Failure before a UART has been authenticated MUST enter a
non-returning `wfi` loop without attempting a diagnostic through unverified MMIO. The existing byte
offset 128 exception checkpoint and byte offset 280 MMU checkpoint MUST remain unchanged.

Structural evidence MUST prove deterministic output, preserved checkpoints, absence of the fixed UART
address, empty static device tables, and emitted indexed descriptor stores. Independent execution
evidence MUST supply a synthetic valid FDT whose PL011 address differs from the historical QEMU base,
observe output at only that discovered address, inspect the three runtime-patched descriptors, simulate
the current-level data-abort route, and observe silent fail-closed behavior for invalid FDTs. Linux QEMU CI MUST boot the
normal versioned-machine FDT and a checked-in independently compiled valid DTB, comparing exact serial
bytes; its copied-image write probe remains the architectural protection oracle. This rule does not implement arbitrary bus `ranges`, phandles, interrupt-controller discovery,
multiple UART selection policy, arbitrary physical boards, or fault recovery from an inaccessible
bootloader pointer.

**DISP-CORE-0095 — capability-bounded AArch64 volatile MMIO.**
The direct AArch64 profile MAY expose `Mmio.read_u8/u16/u32(offset: u16)` and matching
`Mmio.write_u8/u16/u32(offset: u16, value)` operations. `DeviceIo` MUST remain distinct from
`RawMemory`, `Foreign`, and bare unsafe syntax. Every MMIO call MUST be lexically enclosed by an
explicit `unsafe uses DeviceIo` contract, every explicit outer unsafe contract MUST also contain
`DeviceIo`, and direct/call-chain effect analysis MUST expose that authority. Unsupported methods,
arities, offset types, or value widths MUST fail during checking. Hosted interpreters and native
process targets MUST NOT execute these intrinsics as ambient memory access.

The offset is relative only to the unique PL011 page authenticated by DISP-CORE-0094 and retained in
`x20`; source code MUST NOT supply or derive an absolute MMIO address. Before evaluating a write value
or performing any device access, generated code MUST prove `offset <= 4096 - width` and natural
alignment for the selected one-, two-, or four-byte width. Failure MUST access no requested register,
print exactly `[DISP device access fault]\r\n` through the already authenticated UART path, and enter
the non-returning halt loop. Adding the validated offset to the validated page base MUST be the only
device-address construction, keeping every operation inside the single Device-nGnRnE, RW-NX leaf.

Each successful operation MUST emit one direct A64 `LDRB`/`LDRH`/`LDR` or
`STRB`/`STRH`/`STR` of the declared exact width, surrounded by `DMB OSH` barriers. The compiler MUST
not merge, elide, widen, invent, or move another device access across those barriers. Write arguments
MUST retain left-to-right source evaluation and preserve the once-validated offset across value
evaluation. Structural evidence MUST identify all six exact access instructions, width-specific page
bounds, alignment branches, ordered barriers, relative address formation from `x20`, deterministic
images, authority rejection, and absence of a fixed UART address. Independent alternate-address
emulation MUST execute a real register read and write plus an out-of-page failure. Linux QEMU CI MUST
compare exact output from a capability-authorized PL011 status/read-and-data/write fixture and an
unaligned end-of-page rejection fixture. This rule does not claim register-level ownership,
device-specific semantic validation, atomic MMIO transactions, concurrent driver synchronization,
interrupt control, DMA safety, or access to any device other than the authenticated PL011 page.

**DISP-CORE-0096 — deterministic versioned C import header.**
Given a checked complete program, `disp header` MUST generate one deterministic C header describing
every `extern C` import in source order. File input MUST select the same path with extension `.h`;
project input MUST select `disp_ffi_v1.h` inside the project. The artifact MUST identify itself as
ABI version 1, use a fixed collision-resistant include guard, require eight-bit bytes and one-byte C
booleans, include the standard fixed-width/size declarations it consumes, and retain valid C11 and
C++17 linkage syntax. Zero-argument declarations MUST use `(void)`.

Checked exact integers and floats MUST map to exact C types; pointer-width integers MUST map to
`intptr_t`/`uintptr_t`; `CStr` parameters MUST map to `const char *`. Raw pointers MUST receive
deterministic aliases that retain pointee constness and nested pointer structure. Any signature type
without a stable public C representation MUST fail header generation rather than expose a private
runtime layout. Library metadata MUST be emitted only as inert validated comments and MUST NOT add
compiler/linker directives to the header. Output MUST be bounded to 16 MiB and installed through the
transactional artifact writer. Evidence MUST prove byte determinism, exact declarations, fail-closed
unsupported layouts, direct C11 and C++17 compilation, and the CLI artifact path. This rule describes
trusted C imports only. Exported DISP functions, shared-library production, and panic containment
are governed independently by DISP-CORE-0097; callbacks are governed by later Pass 22 rules and
explicit aggregate ABI stability is governed by DISP-CORE-0108.

**DISP-CORE-0097 — contained C-callable DISP shared libraries.**
`export C fn` MUST declare a synchronous, non-generic function with a portable, stable C ABI v1
signature, a non-reserved ASCII symbol, and an explicit `uses Pure` or `uses Foreign` contract.
`Foreign` is permitted only for typed context-free callback invocation; no other authority may enter
the in-process export boundary. It MUST NOT export `main`.
`disp build --library` MUST reject programs with no exports and produce the target shared-library
kind plus the same deterministic ABI header generated by `disp header`. Exported DISP functions
MUST return a status code. A non-`Unit` result MUST be written through a final non-null output
pointer only after successful completion; invalid pointers MUST fail before executing the body.

No checked panic or runtime failure MAY unwind across the C boundary, terminate the C host, or
partially write the result. The per-thread `disp_c_last_error()` view MUST be empty after success
and contain a bounded diagnostic after failure. Nested export entry during failure containment
MUST fail closed. Until cleanup-aware containment is implemented, an exported function's complete
direct-call graph MUST be synchronous and allocation-free, contain no cleanup-bearing local, and
perform no indirect DISP call, intrinsic runtime operation, data operation, spawn, or await. A
signature-checked `CFunction` call is permitted only under the export's exact `Foreign` contract.
Direct DISP helpers and recursion are permitted only when the same proof succeeds transitively. Evidence
MUST compile and dynamically load a real shared library from C, resolve its exact symbols, observe
success, null-output rejection, contained failure through a helper, unchanged failed output,
recursive helper execution, error reset, and syntax/transitive-subset rejection. This rule does not
yet stabilize owned callback contexts; explicit stable records are governed by DISP-CORE-0108. It
also does not stabilize cross-target calling conventions beyond the host C compiler or concurrent
entry from other threads.

**DISP-CORE-0098 — versioned C-to-DISP callback types.**
For every `export C fn name`, the generated ABI-v1 header MUST declare an exact function-pointer
type named `disp_c_callback_name`. Its parameters, status result, out-result position, linkage, and
failure behavior MUST be identical to the corresponding exported symbol. A C host MUST be able to
store a resolved DISP export in that type, pass it through ordinary C code, and invoke it indirectly
without bypassing DISP-CORE-0097 containment. Callback typedef order and spelling MUST be
deterministic, and no closure environment or private DISP runtime layout may enter the public type.
Evidence MUST invoke successful, recursive, and checked-failure DISP paths through generated callback
types from a real C consumer. This rule covers C-to-DISP callbacks only; passing arbitrary C callback
pointers into DISP, callback context ownership, deregistration, and callback thread attachment remain
subsequent Pass 22 work. Same-thread nested re-entry is governed by DISP-CORE-0100.

**DISP-CORE-0099 — typed context-free DISP-to-C callbacks.**
`CFunction<fn(P...) -> R>` MUST represent one thin C function pointer whose parameter and result
types each have a stable C ABI representation. It MUST NOT use the DISP closure code/environment/drop
layout, capture DISP storage, erase a signature, or implicitly convert an ordinary DISP function or
closure. A named non-generic `extern C` function with the exact signature MAY become a `CFunction`
value. The value is Copy but MUST remain thread-affine until callback-provider lifetime and thread
attachment contracts are expressible.

Invoking a `CFunction` MUST require an explicit `unsafe uses Foreign` region, preserve source argument
order and the declared C calling convention, and check for a null pointer before the machine call.
The generated native C and public header MUST use deterministic signature-specific typedefs, define
raw-pointer dependencies before callback aliases, and compile as strict C11 and C++17. Effect
analysis MUST retain `Foreign` when an imported function becomes a value. Evidence MUST prove syntax
rejection, authority rejection, HIR/MIR identity, deterministic header declarations, interpreter
behavior, native invocation of a real imported C symbol, and the emitted null guard. This rule covers
context-free synchronous callbacks only. Context pointers, owned registration handles,
deregistration, provider-declared thread safety, and asynchronous callbacks remain subsequent Pass 22
work. Same-thread nested re-entry is governed by DISP-CORE-0100.

**DISP-CORE-0100 — fail-closed same-thread foreign re-entry.**
While a C-callable DISP export has an active checked-failure target, any C callback that attempts to
enter another DISP export on the same thread MUST be denied before its DISP body executes. The nested
wrapper MUST return `DISP_C_STATUS_INVALID_ARGUMENT`, MUST NOT modify its output pointer, and MUST NOT
replace or disarm the outer containment target. After the callback returns, the outer export MUST
either complete normally under its original target or report its own contained failure. Sequential
entry after the outer call returns MUST remain available and clear stale error text on success.
Evidence MUST dynamically load real DISP exports from C, pass a C callback into an exported DISP
function, re-enter a second export from that callback, observe the exact denial status through the C
shim, and then prove subsequent entry still succeeds. Concurrent entry from explicitly attached
foreign threads is governed by DISP-CORE-0102. This rule does not define callback registration
lifetime or asynchronous re-entry.

**DISP-CORE-0101 — linear owned C callback registration.**
`CRegistration.adopt(context, release)` MUST consume a mutable opaque C context pointer and an exact
`CFunction<fn(mut ptr<Unit>) -> Unit>` release function under an explicit `unsafe uses Foreign`
region. A null release function MUST fail before ownership is adopted. `CRegistration` MUST be
non-Copy, non-exportable, and thread-affine. `is_active()` MAY borrow the registration; `close()`
MUST consume it. Normal native scope cleanup MUST close any still-active registration in reverse
ownership order.

Closing MUST first mark the registration inactive and clear its stored context and release pointer,
then invoke the release function exactly once with the adopted context. Repeated internal cleanup
after consumption MUST be harmless, and source use after `close()` or move MUST be rejected. The
compiler MUST represent `ptr<Unit>` as C `void *`, preserving an opaque context ABI without exposing
DISP storage. Evidence MUST cover missing authority, signature mismatch, use-after-close, attempted
copy, emitted deactivate-before-release ordering, explicit native close, and native scope-exit close.
This rule owns deregistration cleanup only; it does not permit asynchronous provider calls, attach
foreign threads, or make a registration callable from C.

**DISP-CORE-0102 — explicit foreign-thread attachment.**
Every operating-system thread MUST begin unattached to a loaded DISP shared library. The generated
ABI-v1 header and library MUST expose `disp_c_thread_attach()` and `disp_c_thread_detach()` returning
the versioned C status codes. Successful attach MUST enroll only the calling thread and clear its
thread-local error. Attaching an already attached thread MUST return
`DISP_C_STATUS_THREAD_ALREADY_ATTACHED`. Detaching an unattached thread MUST return
`DISP_C_STATUS_THREAD_NOT_ATTACHED`; detaching while that thread is inside a DISP export MUST return
`DISP_C_STATUS_THREAD_BUSY`. Successful detach MUST clear the calling thread's attachment and error.

Every exported DISP wrapper MUST reject an unattached caller with
`DISP_C_STATUS_THREAD_NOT_ATTACHED` before validating output pointers or executing the DISP body,
and MUST leave output storage unchanged. Attachment state, checked-failure targets, and
`disp_c_last_error()` text MUST be thread-local. Distinct attached threads MAY enter allocation-free
exports concurrently; same-thread nested entry remains denied by DISP-CORE-0100. Evidence MUST load
the shared library from a real C host, cover every attachment transition and exact status, deny
detach from a callback during active entry, and run at least two foreign threads concurrently through
repeated attach/export/detach cycles. Attachment does not make a thread-affine `CFunction` or
`CRegistration` transferable and does not authorize asynchronous callbacks.

**DISP-CORE-0103 — quiescent asynchronous C registration shutdown.**
`CRegistration.adopt_async(context, quiesce, release)` MUST consume a mutable opaque context and two
exact `CFunction<fn(mut ptr<Unit>) -> Unit>` callbacks under explicit `unsafe uses Foreign`.
`quiesce` is the provider contract that no new callback invocation can begin and all in-flight
invocations have returned before it returns. Both callbacks MUST be non-null before ownership is
adopted. The unsafe region is an explicit assertion of that provider behavior; the compiler cannot
derive it from foreign code.

Consuming close or native scope cleanup MUST first mark the handle inactive and clear all stored
pointers, then call `quiesce(context)` exactly once, then call `release(context)` exactly once.
Release MUST never race the provider worker represented by the registration. Static evidence MUST
reject an inexact quiesce signature and emitted C MUST prove deactivate-before-quiesce-before-release
ordering. Dynamic evidence MUST use a real C provider that starts an operating-system thread, joins
that thread in quiesce, rejects premature release in the fixture, and observes quiescence before
release. This rule supplies owned asynchronous-provider shutdown; callable DISP closure
contexts/trampolines and provider-initiated DISP callback delivery remain subsequent work.

**DISP-CORE-0104 — typed checked export callback handles.**
`CExport.callback(name)` MUST accept exactly one named, non-generic `export C fn`; ordinary DISP
functions, closures, expressions, imports, and non-exported names MUST be rejected. For an export
`fn(P...) -> R`, the result MUST be a thin `CFunction<fn(P..., mut ptr<R>) -> CInt>` when `R` is
non-Unit, or `CFunction<fn(P...) -> CInt>` for Unit. The pointer MUST target the public checked export
wrapper, never its unchecked internal implementation. Taking the handle is pure; passing or invoking
it through foreign code remains governed by explicit `Foreign` authority.

Generated native C MUST declare every exported wrapper before any internal function can take its
address. Calls through the handle MUST retain the wrapper's thread-attachment, null-out-pointer,
status, output-commit, checked-failure, last-error, and same-thread re-entry contracts from
DISP-CORE-0097, 0100, and 0102. Dynamic evidence MUST pass at least one success wrapper and one
checked-failure wrapper from DISP into a real C provider, invoke both on a provider-created attached
thread, preserve the failure output, inspect its thread-local error, detach, and join successfully.
This rule provides context-free provider-initiated DISP callback delivery. Captured DISP closure
environments and generated context trampolines remain subsequent work; a provider retaining a
callback MUST quiesce its work before library unload or related owned context release.

**DISP-CORE-0105 — atomically owned captured C callbacks.**
`CRegistration.register_async(handler, register, quiesce, release)` MUST atomically move a direct
named synchronous DISP function or `move` closure into a stable callback context, call the exact C
provider register function with a signature-specific checked trampoline and that context, and return
one linear `CRegistration`. For handler `fn(P...) -> R`, the trampoline parameter MUST be
`CFunction<fn(mut ptr<Unit>, P..., mut ptr<R>) -> CInt>` for non-Unit `R`, or omit the out pointer for
Unit. The register function MUST have the exact type
`CFunction<fn(trampoline, mut ptr<Unit>) -> mut ptr<Unit>>`; quiesce and release MUST each be
`CFunction<fn(mut ptr<Unit>) -> Unit>`.

The operation MUST require explicit `unsafe uses Foreign`. Borrowing closures and indirect handler
variables MUST be rejected. The initial profile MUST accept only moved ABI scalar captures that are
safe for immutable foreign-thread access. The handler's complete direct DISP call graph MUST be
synchronous, allocation-free, and cleanup-free so checked callback failure cannot skip owned
cleanup. A null register, quiesce, or release function MUST fail before registration; a null provider
context MUST drop the newly owned callback context before failing.

The generated trampoline MUST require thread attachment, validate context and out-result pointers,
contain checked failures, commit output only on success, and maintain thread-local error state. The
provider contract asserted by the unsafe region MUST retain the trampoline/context pair only until
quiesce returns and MUST never invoke it afterward. Close or scope cleanup MUST clear the registration,
quiesce and join provider work, drop the captured DISP environment, then release the provider context,
each exactly once. Dynamic evidence MUST move a scalar into a closure, invoke it through a real
provider-created attached thread, observe its captured result, join, and prove
quiesce-before-capture-drop-before-release ordering. Broader owned Send captures remain subsequent
work.

**DISP-CORE-0106 — resource-owning Send callback environments.**
`CRegistration.register_async` MUST accept moved captures precisely when their complete type is
Send-compatible under the same structural rule used by `spawn`. References, borrowed views, raw or
checked pointers, function values, guards, registrations, and secret-bearing cryptographic values
MUST be rejected before registration. Generic or otherwise unproved capture types MUST fail closed.

The closure environment MUST remain the sole owner of every capture. Each callback invocation MUST
borrow its capture slots without consuming or destroying them; reusable closures MUST continue to
reject moving a non-Copy capture out of their body. The provider may invoke the handler repeatedly
or concurrently only through immutable callback access. Cleanup-bearing handler locals and
allocation remain outside this profile, but read-only, non-allocating inspection of owned captures
MAY be admitted by an explicit intrinsic allowlist. After quiescence proves that no invocation is
live or can begin, closure drop glue MUST recursively destroy every captured resource exactly once
before provider release. Dynamic evidence MUST move a heap-owning `String`, inspect it on a real
provider thread across repeated calls, and prove quiesce-before-capture-drop-before-release.

**DISP-CORE-0107 — transactional owned cleanup for C exports.**
Each checked `export C fn` entry MUST begin a thread-local allocation transaction after installing
its containment target. Every managed allocation created inside the complete direct export graph,
including nested aggregate allocations and reallocations, MUST join that transaction. Normal DISP
destruction MUST remove released allocations from it. On contained failure, the wrapper MUST reclaim
every allocation still owned by the transaction and restore abandoned thread-local call-depth state
before clearing the containment target and returning `DISP_C_STATUS_PANIC`. The caller's out-result
MUST remain unchanged and the original checked-failure diagnostic MUST remain available.

The admitted cleanup profile MAY contain structurally nested heap-only owned values whose abort
cleanup is satisfied by managed allocation rollback. Intrinsics in that graph MUST come from an
explicit non-authoritative, rollback-safe allowlist. Handles, secrets requiring zeroization, tasks,
threads, function environments, registrations, or any value with semantic release side effects MUST
remain rejected until a typed rollback hook exists. A successful export MUST finish with balanced
call depth and no live transaction allocation; violation MUST fail closed. Dynamic evidence MUST run
at least one thousand allocate-then-fail calls under a memory ceiling too small to tolerate a leak,
preserve the original error and output on every call, and then execute the same export successfully.

**DISP-CORE-0108 — explicit stable C ABI records.**
`export C struct Name { fields... }` MUST opt one non-generic, non-empty value record into the
versioned C ABI. The record name and every field name MUST be safe, non-reserved ASCII C
identifiers. Each field MUST be a C ABI scalar, an explicit raw pointer whose complete pointee
description has a stable public representation, or another non-generic `export C struct`.
Owned runtime values, borrowed `CStr` fields, ordinary private structs, enums, containers,
function environments, handles, secrets, and generic fields MUST fail closed. Recursive by-value
record cycles MUST be rejected.

The generated header MUST declare a deterministic `disp_c_Name` C type, preserve source field
order and names, and emit compile-time checks for every field offset, total size, and alignment.
Those checks MUST compile under strict C11 and C++17. `extern C` and `export C fn`
signatures MAY pass or return the record by value; checked DISP exports MUST continue to return
status separately and commit a record result only through the final output pointer on success. The
native compiler representation and public declaration MUST use the same target layout. Evidence
MUST compile the header as both C and C++, dynamically pass a nested Outernet-style packet record
from a real C host into a DISP shared library, return a transformed record, verify all fields, and
reject unstable record declarations.

**DISP-CORE-0109 — cross-width C record calling conventions.**
The same deterministic C ABI v1 header for a fixed-width exported record MUST compile without
source changes for both Windows x86-64 and Windows i686 C targets. Evidence MUST compile a real
record-taking and record-returning function to target assembly in both modes, not merely preprocess
or parse it. The x86-64 artifact MUST demonstrate the target's integer-register aggregate argument
and return convention; the i686 artifact MUST demonstrate its stack argument and split
`EDX:EAX` aggregate return convention. Both artifacts MUST retain the header's field offset,
size, and alignment assertions. This evidence stabilizes the declared fixed-width C record contract
on those two target ABIs; it does not claim a native DISP runtime or shared-library release for
i686, AArch64, Linux, or any other platform.

**DISP-CORE-0110 — typed handle rollback across contained C failures.**
An admitted handle-bearing value in a checked `export C fn` graph MUST install a type-specific
rollback hook immediately after acquiring external ownership. Normal close or lexical destruction
MUST unlink that hook before performing the ordinary typed release. Contained failure MUST invoke
all remaining hooks in strict reverse acquisition order before reclaiming managed allocations and
returning to C. Each hook MUST perform the resource type's complete semantic cleanup exactly once;
an asynchronous registration therefore quiesces its provider before destroying callback state and
releasing provider context.

The rollback ledger MUST be thread-local, MUST reject a successful export that retains a live hook,
and MUST NOT admit a resource merely because its wrapper storage can be freed. `CRegistration` is
the first admitted handle type; all other handles, tasks, threads, callable environments, and
zeroizing secrets remain rejected until their own typed hooks are implemented and evidenced.
Dynamic evidence MUST execute at least one thousand acquire-then-fail calls, preserve the caller's
output and checked diagnostic, and prove exactly-once reverse-order provider release. It MUST also
exercise the successful typed-cleanup path against the same resource graph.

**DISP-CORE-0034 — deterministic resource quotas.** An implementation MUST apply finite,
overflow-checked bounds to compiler work and runtime execution. Candidate 1 native execution MUST
meter managed live memory, execution work, synchronous call depth, and printed output across the
process. The interpreter MUST meter execution work and printed output across spawned work and MUST
bound function and closure recursion. Explicit `Memory` allocations in both engines MUST share a
live-memory quota and return their permits after final ownership release. Both engines MUST bound simultaneously live tasks and
runtime threads and release their permits exactly once. The operation that would cross a quota MUST fail before its
rejected allocation or output is committed. Missing configuration MUST select documented finite
defaults; invalid configuration MUST fail closed and MUST NOT mean unlimited execution. Process
launch attempts MUST consume a shared finite budget before operating-system process creation.
Synchronous and asynchronous file writes and copies MUST validate the requested byte ceiling before
opening, truncating, appending to, or otherwise mutating the destination.

## 15. Conformance evidence matrix

The evidence below is executable and checked by `compiler/tests/specification.rs`. A referenced
test demonstrates the rule but does not freeze incidental implementation details.

| Rule | Primary evidence file | Test symbol |
|---|---|---|
| DISP-CORE-0001 | `compiler/tests/cli.rs` | `run_and_check_commands_use_the_full_pipeline` |
| DISP-CORE-0002 | `compiler/src/lexer.rs` | `lexes_keywords_identifiers_and_unicode` |
| DISP-CORE-0003 | `compiler/tests/fuzz_smoke.rs` | `complete_frontend_fuzz_smoke_never_panics` |
| DISP-CORE-0004 | `compiler/tests/specification.rs` | `reserved_future_words_have_no_core_grammar` |
| DISP-CORE-0005 | `compiler/src/parser.rs` | `rejects_pathological_nesting_without_panicking` |
| DISP-CORE-0006 | `compiler/tests/modules.rs` | `conflicting_wildcard_imports_fail_instead_of_guessing` |
| DISP-CORE-0007 | `compiler/tests/adts.rs` | `nominal_types_do_not_unify_structurally` |
| DISP-CORE-0008 | `compiler/tests/backend.rs` | `native_checked_overflow_has_controlled_failure` |
| DISP-CORE-0009 | `compiler/tests/ownership.rs` | `definite_initialization_merges_control_flow` |
| DISP-CORE-0010 | `compiler/src/interpreter.rs` | `return_break_and_continue_propagate_correctly` |
| DISP-CORE-0011 | `compiler/tests/callables.rs` | `closures_capture_shared_mutable_and_moved_state_differentially` |
| DISP-CORE-0012 | `compiler/tests/patterns.rs` | `nested_finite_patterns_are_proven_exhaustive` |
| DISP-CORE-0013 | `compiler/tests/generics.rs` | `generic_trait_implementations_instantiate_and_overlap_is_rejected` |
| DISP-CORE-0014 | `compiler/tests/dynamic_places.rs` | `indexed_loans_block_owner_mutation_moves_and_lifetime_escape` |
| DISP-CORE-0015 | `compiler/tests/ffi.rs` | `ffi_calls_require_unsafe_and_signatures_reject_owned_or_borrowed_abi_values` |
| DISP-CORE-0016 | `compiler/tests/async.rs` | `suspension_resumes_without_repeating_prior_side_effects` |
| DISP-CORE-0017 | `compiler/tests/data.rs` | `data_schemas_are_nominal_and_reach_hir_mir` |
| DISP-CORE-0018 | `compiler/tests/cli.rs` | `native_run_cache_reuses_unchanged_builds_and_tracks_imports` |
| DISP-CORE-0019 | `compiler/tests/diagnostics.rs` | `compiler_stages_have_stable_codes_and_exact_spans` |
| DISP-CORE-0020 | `compiler/tests/effects.rs` | `explicit_and_inferred_effect_contracts_are_checked_and_reported` |
| DISP-CORE-0021 | `compiler/tests/expansion.rs` | `map_substitution_is_hygienic_and_preserves_call_site_names` |
| DISP-CORE-0022 | `compiler/tests/generics.rs` | `associated_type_projection_is_resolved_by_the_selected_implementation` |
| DISP-CORE-0023 | `compiler/tests/patterns.rs` | `or_patterns_expand_nested_alternatives_with_one_binding_contract` |
| DISP-CORE-0024 | `compiler/tests/errors.rs` | `typed_result_propagation_is_exact_and_differential` |
| DISP-CORE-0025 | `compiler/tests/errors.rs` | `mir_failure_edge_moves_error_before_exactly_once_cleanup` |
| DISP-CORE-0026 | `compiler/tests/compatibility.rs` | `legacy_and_explicit_edition_one_have_identical_behavior` |
| DISP-CORE-0027 | `compiler/tests/compatibility.rs` | `migration_pins_compatibility_without_rewriting_source` |
| DISP-CORE-0028 | `compiler/tests/ownership_model.rs` | `aggregate_borrows_preserve_safe_input_origins_and_active_loans` |
| DISP-CORE-0029 | `compiler/tests/memory_safety_model.rs` | `representative_safe_memory_program_passes_native_sanitizers` |
| DISP-CORE-0030 | `compiler/tests/unsafe_containment.rs` | `nested_blocks_cannot_widen_an_enclosing_contract` |
| DISP-CORE-0031 | `compiler/tests/raw_pointer_safety.rs` | `checked_offsets_and_accesses_fail_before_native_undefined_behavior` |
| DISP-CORE-0032 | `compiler/tests/concurrency.rs` | `bounded_channels_stress_multiple_producers_and_drain_after_close` |
| DISP-CORE-0033 | `compiler/tests/tasks.rs` | `cancelling_a_parent_cancels_pending_nested_tasks_before_later_side_effects` |
| DISP-CORE-0034 | `compiler/tests/resource_limits.rs` | `native_runtime_threads_share_one_process_quota` |
| DISP-CORE-0035 | `compiler/tests/modules.rs` | `compiler_extension_manifests_fail_closed_with_security_guidance` |
| DISP-CORE-0036 | `compiler/tests/components.rs` | `component_profile_limits_fail_closed_in_isolated_children` |
| DISP-CORE-0037 | `compiler/tests/components.rs` | `linux_component_profile_denies_network_syscalls` |
| DISP-CORE-0038 | `compiler/tests/components.rs` | `windows_component_is_appcontainer_and_denies_network_and_host_files` |
| DISP-CORE-0039 | `compiler/tests/components.rs` | `windows_component_is_appcontainer_and_denies_network_and_host_files` |
| DISP-CORE-0040 | `compiler/tests/crypto_language.rs` | `native_randomness_matches_interpreter_and_uses_the_os_provider` |
| DISP-CORE-0041 | `compiler/tests/crypto_language.rs` | `native_secret_bytes_match_interpreter_and_zeroize_before_release` |
| DISP-CORE-0042 | `compiler/tests/crypto_language.rs` | `native_sha256_and_hmac_use_platform_providers_and_match_the_interpreter` |
| DISP-CORE-0043 | `compiler/tests/crypto_language.rs` | `native_hkdf_sha256_matches_rfc5869_and_zeroizes_intermediates` |
| DISP-CORE-0044 | `compiler/tests/crypto_native_abi.rs` | `native_crypto_abi_seals_opens_and_authenticates_before_output` |
| DISP-CORE-0045 | `compiler/tests/crypto_language.rs` | `interpreter_authenticated_encryption_is_opaque_and_fail_closed` |
| DISP-CORE-0046 | `compiler/tests/crypto_language.rs` | `ed25519_signatures_are_opaque_strict_and_native_differential` |
| DISP-CORE-0047 | `compiler/tests/crypto_language.rs` | `argon2id_password_hashing_is_fixed_policy_and_native_differential` |
| DISP-CORE-0048 | `compiler/tests/crypto_language.rs` | `aead_envelope_format_is_versioned_canonical_and_native_differential` |
| DISP-CORE-0049 | `compiler/tests/crypto_language.rs` | `ed25519_signatures_are_opaque_strict_and_native_differential` |
| DISP-CORE-0050 | `compiler/tests/crypto_native_abi.rs` | `native_crypto_abi_generates_signs_and_strictly_verifies_ed25519` |
| DISP-CORE-0051 | `compiler/tests/crypto_language.rs` | `ed25519_signatures_are_opaque_strict_and_native_differential` |
| DISP-CORE-0052 | `compiler/tests/crypto_language.rs` | `ed25519_lifecycle_enforces_activation_expiry_and_revocation_differentially` |
| DISP-CORE-0053 | `compiler/src/crypto_keystore.rs` | `provider_sdk_dispatches_only_opaque_handles_and_messages` |
| DISP-CORE-0054 | `compiler/tests/supply_chain.rs` | `dependency_audit_is_pinned_scheduled_and_fail_closed` |
| DISP-CORE-0055 | `compiler/tests/supply_chain.rs` | `libfuzzer_targets_are_pinned_and_continuously_exercised` |
| DISP-CORE-0056 | `compiler/tests/supply_chain.rs` | `release_binaries_embed_and_verify_locked_dependency_provenance` |
| DISP-CORE-0057 | `compiler/tests/security_governance.rs` | `compiler_unsafe_code_stays_inside_the_audited_boundary_inventory` |
| DISP-CORE-0058 | `compiler/tests/supply_chain.rs` | `rust_asan_regressions_are_pinned_and_fail_closed` |
| DISP-CORE-0059 | `compiler/tests/supply_chain.rs` | `release_sboms_cover_locked_native_graphs_on_every_desktop_platform` |
| DISP-CORE-0060 | `compiler/tests/supply_chain.rs` | `signed_release_provenance_is_scoped_and_binds_each_sbom` |
| DISP-CORE-0061 | `compiler/tests/freestanding.rs` | `direct_freestanding_build_is_runtime_free_deterministic_and_fail_closed` |
| DISP-CORE-0062 | `compiler/src/freestanding.rs` | `allocation_free_u16_control_flow_emits_checked_machine_code` |
| DISP-CORE-0063 | `compiler/src/freestanding.rs` | `encoding_and_multisector_capacity_fail_closed` |
| DISP-CORE-0064 | `compiler/src/freestanding.rs` | `wider_signed_and_boolean_values_have_exact_checked_codegen` |
| DISP-CORE-0065 | `compiler/src/freestanding.rs` | `scalar_functions_preserve_nested_arguments_returns_and_recursive_frames` |
| DISP-CORE-0066 | `compiler/src/freestanding.rs` | `structured_loops_bind_break_and_continue_to_the_innermost_scope` |
| DISP-CORE-0067 | `compiler/src/freestanding.rs` | `scalar_functions_preserve_nested_arguments_returns_and_recursive_frames` |
| DISP-CORE-0068 | `compiler/src/freestanding.rs` | `exact_u8_values_use_compact_storage_checked_math_and_safe_calls` |
| DISP-CORE-0069 | `compiler/src/freestanding32.rs` | `protected32_bootstrap_is_flat_deterministic_and_fail_closed` |
| DISP-CORE-0070 | `compiler/src/freestanding32.rs` | `protected32_bootstrap_is_flat_deterministic_and_fail_closed` |
| DISP-CORE-0071 | `compiler/src/freestanding32.rs` | `protected32_u32_control_flow_is_checked_and_uses_memory_above_one_mibibyte` |
| DISP-CORE-0072 | `compiler/src/freestanding32.rs` | `protected32_exact_compact_and_signed_widths_fail_closed` |
| DISP-CORE-0073 | `compiler/src/freestanding32.rs` | `protected32_functions_preserve_nested_and_recursive_frames` |
| DISP-CORE-0074 | `compiler/src/freestanding32.rs` | `protected32_fixed_arrays_use_exact_storage_checked_indices_and_recursive_frames` |
| DISP-CORE-0075 | `compiler/src/freestanding32.rs` | `protected32_device_io_requires_explicit_authority_and_emits_exact_port_instructions` |
| DISP-CORE-0076 | `compiler/src/freestanding32.rs` | `protected32_installs_bounded_exception_gates_and_a_known_state_handler` |
| DISP-CORE-0077 | `compiler/src/freestanding32.rs` | `protected32_enables_bounded_identity_paging_with_an_unmapped_null_page` |
| DISP-CORE-0078 | `compiler/src/freestanding64.rs` | `x86_64_bootstrap_checks_cpu_builds_four_level_paging_and_enters_long_mode` |
| DISP-CORE-0079 | `compiler/src/freestanding64.rs` | `x86_64_checked_scalars_use_bounded_absolute_locals_and_long_mode_stack_encodings` |
| DISP-CORE-0080 | `compiler/src/freestanding64.rs` | `x86_64_functions_snapshot_guard_and_restore_recursive_scalar_frames` |
| DISP-CORE-0081 | `compiler/src/freestanding64.rs` | `x86_64_fixed_arrays_use_exact_storage_checked_indices_and_recursive_frames` |
| DISP-CORE-0082 | `compiler/src/freestanding64.rs` | `x86_64_device_io_requires_explicit_authority_and_emits_exact_port_instructions` |
| DISP-CORE-0083 | `compiler/src/freestanding64.rs` | `x86_64_nx_paging_whitelists_only_the_bounded_stage_for_execution` |
| DISP-CORE-0084 | `compiler/src/freestanding64.rs` | `x86_64_idt_routes_security_critical_faults_to_distinct_fail_closed_handlers` |
| DISP-CORE-0085 | `compiler/src/freestanding64.rs` | `x86_64_pic_is_remapped_fully_masked_and_routed_before_user_code` |
| DISP-CORE-0086 | `compiler/src/freestanding64.rs` | `x86_64_timer_capability_installs_one_bounded_100_hz_irq_source` |
| DISP-CORE-0087 | `compiler/src/freestanding_aarch64.rs` | `aarch64_image_has_checked_scalar_control_and_deterministic_utf8_data` |
| DISP-CORE-0088 | `compiler/src/freestanding_aarch64.rs` | `aarch64_scalar_profile_rejects_unsupported_or_unbounded_programs` |
| DISP-CORE-0089 | `compiler/src/freestanding_aarch64.rs` | `aarch64_exact_widths_use_compact_storage_checked_signed_math_and_scalar_output` |
| DISP-CORE-0090 | `compiler/src/freestanding_aarch64.rs` | `aarch64_functions_preserve_recursive_exact_frames_and_guard_the_stack` |
| DISP-CORE-0091 | `compiler/src/freestanding_aarch64.rs` | `aarch64_fixed_arrays_use_exact_storage_checked_indices_and_recursive_frames` |
| DISP-CORE-0092 | `compiler/src/freestanding_aarch64.rs` | `aarch64_exception_vectors_cover_current_el_classes_and_fail_closed` |
| DISP-CORE-0093 | `compiler/src/freestanding_aarch64.rs` | `aarch64_sparse_page_tables_enforce_wx_and_protect_translation_state` |
| DISP-CORE-0094 | `compiler/src/freestanding_aarch64.rs` | `aarch64_dtb_prelude_discovers_pl011_validates_ram_and_patches_sparse_tables` |
| DISP-CORE-0095 | `compiler/src/freestanding_aarch64.rs` | `aarch64_mmio_requires_device_authority_bounds_offsets_and_orders_volatile_access` |
| DISP-CORE-0096 | `compiler/tests/c_header.rs` | `generated_header_compiles_as_c_and_cpp_and_is_written_transactionally` |
| DISP-CORE-0097 | `compiler/tests/c_exports.rs` | `c_consumer_calls_shared_disp_library_and_observes_contained_failure` |
| DISP-CORE-0098 | `compiler/tests/c_exports.rs` | `c_consumer_calls_shared_disp_library_and_observes_contained_failure` |
| DISP-CORE-0099 | `compiler/tests/c_callbacks.rs` | `disp_invokes_an_imported_c_symbol_through_a_typed_callback` |
| DISP-CORE-0100 | `compiler/tests/c_exports.rs` | `c_consumer_calls_shared_disp_library_and_observes_contained_failure` |
| DISP-CORE-0101 | `compiler/tests/c_registration.rs` | `native_registration_closes_explicitly_or_at_scope_exit_exactly_once` |
| DISP-CORE-0102 | `compiler/tests/c_exports.rs` | `c_consumer_calls_shared_disp_library_and_observes_contained_failure` |
| DISP-CORE-0103 | `compiler/tests/c_registration.rs` | `threaded_provider_is_joined_before_its_context_is_released` |
| DISP-CORE-0104 | `compiler/tests/c_exports.rs` | `disp_passes_checked_export_callbacks_to_a_threaded_c_provider` |
| DISP-CORE-0105 | `compiler/tests/c_registration.rs` | `captured_disp_handler_runs_on_provider_thread_and_drops_after_quiescence` |
| DISP-CORE-0106 | `compiler/tests/c_registration.rs` | `captured_disp_handler_runs_on_provider_thread_and_drops_after_quiescence` |
| DISP-CORE-0107 | `compiler/tests/c_exports.rs` | `c_consumer_calls_shared_disp_library_and_observes_contained_failure` |
| DISP-CORE-0108 | `compiler/tests/c_exports.rs` | `c_consumer_calls_shared_disp_library_and_observes_contained_failure` |
| DISP-CORE-0109 | `compiler/tests/c_header.rs` | `generated_header_compiles_as_c_and_cpp_and_is_written_transactionally` |
| DISP-CORE-0110 | `compiler/tests/c_exports.rs` | `contained_export_failure_rolls_back_handle_resources_in_reverse_order` |

## 16. Explicitly non-normative or incomplete areas

Candidate 1 does not specify Page/component grammar, tensors/autodiff/GPU semantics, a stable
package registry protocol, reflection, user-defined macros, resumable exceptions, stable enum or
container ABIs beyond explicit C records, non-Windows native targets, or
self-hosting. Recoverable typed
failure is fully specified by `Result`, `Option`, and `?`; hidden exception effects are not part of
the core. The two bounded `Meta`
operations above are the complete current generation surface; draft macro proposals do not
constitute implementation. Other areas become normative only through later passes with
conformance evidence.

This boundary is essential: DISP grows toward all computing domains without allowing a proposal,
token, or mock API to masquerade as a completed language feature.
