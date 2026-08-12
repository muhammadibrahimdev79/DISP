# DISP Type System

> **Design draft:** GPT-generated and not authoritative. See [the documentation index](../README.md) for current, test-backed behavior.

## 0. Status

This document defines the initial type-system design for DISP.

The type system is experimental until explicitly marked stable.

The DISP type system must be:

- statically checked
- strongly typed
- memory-safe
- concurrency-aware
- expressive
- predictable
- easy to use
- suitable for both high-level and low-level programming

---

# 1. Core Principle

> Infer what is obvious. Require what is important. Reject what is unsafe.

DISP should reduce unnecessary type annotations without sacrificing compile-time guarantees.

---

# 2. Static Typing

Every DISP expression has a type known to the compiler.

Example:

```disp
let age = 20
let name = "DISP"
let active = true
```

The compiler infers:

```text
age     -> integer type
name    -> string type
active  -> bool
```

Dynamic typing must not silently replace static guarantees.

---

# 3. Explicit Types

Types may be written explicitly:

```disp
let age: i32 = 20
let score: f64 = 98.5
let active: bool = true
```

Explicit annotations are required when inference is ambiguous or when representation matters.

---

# 4. Primitive Types

Initial primitive types include:

```text
bool

i8
i16
i32
i64
i128

u8
u16
u32
u64
u128

int
uint

f32
f64

char
str
```

---

# 5. Integer Defaults

Integer literals should use a predictable default type.

Initial proposal:

```text
integer literal -> int
```

Example:

```disp
let x = 10
```

The compiler treats `x` as:

```text
int
```

unless context requires another type.

---

# 6. Floating-Point Defaults

Floating-point literals should default predictably.

Initial proposal:

```text
floating literal -> f64
```

Example:

```disp
let x = 3.14
```

The inferred type is:

```text
f64
```

---

# 7. Type Inference

DISP should infer types whenever the result is unambiguous.

Example:

```disp
let x = 10
let y = x + 20
```

The compiler infers compatible types for both.

Function return inference may be allowed where obvious:

```disp
fn double(x: i32) {
    x * 2
}
```

However, public APIs may require explicit return types for stability and clarity.

---

# 8. No Implicit Dangerous Conversions

DISP must not silently perform potentially dangerous conversions.

Invalid:

```disp
let big: i64 = 100000
let small: i8 = big
```

Explicit conversion:

```disp
let small = i8.try_from(big)?
```

---

# 9. Safe Numeric Promotion

Only conversions proven lossless may happen automatically.

Example:

```disp
let small: i8 = 10
let large: i64 = small
```

This conversion may be allowed because every `i8` value fits inside `i64`.

Conversions that may:

- overflow
- truncate
- lose precision
- change signedness unsafely

must require explicit syntax.

---

# 10. Strong Typing

Unrelated types are not interchangeable.

Example:

```disp
type UserID(u64)
type AccountID(u64)
```

Even though both contain `u64`, this must fail:

```disp
let user: UserID = AccountID(5)
```

Strong types prevent accidental semantic mixing.

---

# 11. Type Aliases

Aliases create alternate names for the same type.

```disp
type UserID = u64
```

This is not a distinct type.

---

# 12. Newtypes

Newtypes create distinct types.

```disp
type UserID(u64)
```

Example:

```disp
let id = UserID(10)
```

`UserID` is not implicitly interchangeable with `u64`.

---

# 13. Structures

Structures define product types.

```disp
struct User {
    id: UserID
    name: String
    active: bool
}
```

Fields have fixed static types.

---

# 14. Enums

Enums define sum types.

```disp
enum Status {
    Active
    Disabled
    Deleted
}
```

Enums may contain data:

```disp
enum Message {
    Text(String)
    Number(i64)
    Quit
}
```

---

# 15. Algebraic Data Types

DISP should support algebraic data types naturally.

Example:

```disp
enum Result<T, E> {
    Ok(T)
    Err(E)
}
```

These types enable explicit modeling of valid program states.

---

# 16. Exhaustive Matching

Pattern matching over closed types must be exhaustive.

Example:

```disp
match status {
    Active => ...
    Disabled => ...
    Deleted => ...
}
```

If another variant exists and is not handled, the compiler must reject the match unless a wildcard is present.

---

# 17. Option Types

DISP does not use null as the default representation of missing values.

Optional values use:

```disp
Option<T>
```

Variants:

```disp
Some(T)
None
```

Example:

```disp
let user: Option<User>
```

---

# 18. Non-Null References

Normal references cannot be null.

```disp
let user: &User
```

means:

```text
a valid reference to User
```

Optional references require:

```disp
Option<&User>
```

---

# 19. Result Types

Recoverable failures use:

```disp
Result<T, E>
```

Example:

```disp
fn load(path: str) -> Result<File, IOError> {
    ...
}
```

This makes failure part of the function's type.

---

# 20. Never Type

DISP should provide a type representing computation that never returns.

Conceptually:

```text
Never
```

or:

```text
!
```

Example:

```disp
fn panic(message: str) -> Never {
    ...
}
```

---

# 21. Unit Type

Functions that return no meaningful value use a unit type.

Conceptually:

```text
Unit
```

Example:

```disp
fn log(message: str) -> Unit {
    print(message)
}
```

The language may allow omission of `-> Unit`.

---

# 22. Tuples

Tuple types:

```disp
(i32, String, bool)
```

Example:

```disp
let value: (i32, String) = (200, "OK")
```

---

# 23. Arrays

Fixed arrays carry their length in the type.

```disp
[i32; 4]
```

This differs from:

```disp
[i32; 8]
```

Array size may participate in compile-time checking.

---

# 24. Slices

Borrowed contiguous collections use slice types.

```disp
Slice<T>
MutSlice<T>
```

Example:

```disp
fn sum(values: Slice<i32>) -> i32 {
    ...
}
```

---

# 25. Lists

Dynamic collections use:

```disp
List<T>
```

Example:

```disp
let values: List<i32>
```

---

# 26. Maps

Associative collections use:

```disp
Map<K, V>
```

Example:

```disp
let users: Map<UserID, User>
```

Key constraints must be validated by the type system.

---

# 27. Function Types

Functions are first-class values.

Conceptual function type:

```text
fn(i32, i32) -> i32
```

Example:

```disp
let operation: fn(i32, i32) -> i32 = add
```

---

# 28. Closures

Closures have compiler-generated types.

Example:

```disp
let square = |x: i32| x * x
```

The compiler tracks captured values and ownership behavior.

---

# 29. Generic Types

DISP supports parametric polymorphism.

```disp
struct Box<T> {
    value: T
}
```

Example:

```disp
Box<i32>
Box<String>
Box<User>
```

---

# 30. Generic Functions

```disp
fn identity<T>(value: T) -> T {
    value
}
```

Usage:

```disp
let x = identity(10)
let y = identity("DISP")
```

Generic types should normally be inferred at call sites.

---

# 31. Generic Constraints

Generic parameters may require capabilities.

```disp
fn max<T: Ordered>(a: T, b: T) -> T {
    if a > b {
        a
    } else {
        b
    }
}
```

---

# 32. Multiple Constraints

```disp
fn process<T: Clone + Send + Display>(value: T) {
    ...
}
```

The type must satisfy every listed requirement.

---

# 33. Traits

Traits define reusable behavior.

```disp
trait Display {
    fn display(self: &Self) -> String
}
```

Implementation:

```disp
impl Display for User {
    fn display(self: &Self) -> String {
        self.name
    }
}
```

---

# 34. Trait Safety

Trait implementations must satisfy the exact declared contract.

The compiler must verify:

- required methods
- parameter types
- return types
- generic constraints
- associated types
- safety requirements

---

# 35. Associated Types

Traits may define associated types.

```disp
trait Iterator {
    type Item

    fn next(self: &mut Self) -> Option<Self.Item>
}
```

This avoids unnecessary generic parameter explosion.

---

# 36. Trait Composition

Traits may depend on other traits.

```disp
trait Ordered: Equal {
    fn compare(self: &Self, other: &Self) -> Ordering
}
```

---

# 37. Default Trait Methods

Traits may provide default behavior.

```disp
trait Speak {
    fn name(self: &Self) -> str

    fn speak(self: &Self) {
        print(self.name())
    }
}
```

Implementations may override default methods.

---

# 38. Type Equality

Two types are equal only according to language-defined equivalence rules.

DISP must not rely on structural coincidence where semantic distinction matters.

Newtypes remain distinct even when their internal representation is identical.

---

# 39. Type Identity

Named types have stable semantic identities within their module and package context.

This matters for:

- ABI
- serialization
- reflection
- linking
- package compatibility

---

# 40. Compile-Time Size

For sized types, the compiler must know:

```text
size
alignment
layout
```

when required for native code generation.

---

# 41. Unsized Types

DISP may support dynamically sized types.

Examples may include:

```text
str
Slice<T>
trait objects
```

Unsized values must be accessed through representations that carry required metadata.

---

# 42. Type Layout

Default layout should be compiler-controlled.

Explicit layout:

```disp
@repr(C)
struct Header {
    kind: u16
    size: u32
}
```

Layout guarantees must only exist when explicitly specified.

---

# 43. Alignment

Types have defined alignment requirements.

Advanced users may request explicit alignment:

```disp
@align(64)
struct CacheLine {
    ...
}
```

Invalid alignments must be rejected.

---

# 44. Ownership in Types

The type system participates in ownership checking.

These concepts are distinct:

```text
T
&T
&mut T
Shared<T>
Weak<T>
ptr<T>
mut ptr<T>
```

The compiler must never treat them as interchangeable without explicit conversion.

---

# 45. Immutable References

```disp
&T
```

permits read access.

Multiple immutable references may coexist where safe.

---

# 46. Mutable References

```disp
&mut T
```

provides exclusive mutation access.

The compiler must reject unsafe aliasing.

---

# 47. Raw Pointers

Raw pointers are separate from safe references.

```disp
ptr<T>
mut ptr<T>
```

Raw pointers may:

- be null
- dangle
- alias

Therefore dereferencing them requires unsafe context.

---

# 48. Shared Ownership Types

Explicit shared ownership:

```disp
Shared<T>
```

Non-owning shared observation:

```disp
Weak<T>
```

The cost of shared ownership must remain visible in the type.

---

# 49. Concurrency Properties

Types should carry or derive concurrency properties.

Core conceptual traits:

```text
Send
Share
```

`Send`:

```text
ownership may move between execution contexts
```

`Share`:

```text
shared references may be safely accessed concurrently
```

---

# 50. Automatic Concurrency Derivation

The compiler should automatically derive `Send` and `Share` when every relevant field satisfies the required guarantees.

Example:

```disp
struct Point {
    x: i32
    y: i32
}
```

This type should naturally be safe to send and share.

---

# 51. Non-Send Types

Some types should intentionally not cross thread boundaries.

Examples may include:

- thread-local handles
- certain GUI objects
- raw foreign resources
- execution-context-bound resources

The compiler must enforce these restrictions.

---

# 52. Interior Mutability

Types that permit mutation through shared references must make that capability explicit.

Examples:

```text
Atomic<T>
Mutex<T>
Cell<T>
```

Hidden shared mutation must not bypass DISP's aliasing rules.

---

# 53. Atomic Types

Atomic operations require dedicated types.

Example:

```disp
let count = Atomic<i64>(0)
```

Atomic ordering semantics must be explicitly specified.

---

# 54. Compile-Time Types

Compile-time values may participate in types.

Example:

```disp
Tensor<f32, [1024, 1024]>
```

The shape forms part of the type.

This enables compile-time verification of operations such as matrix multiplication.

---

# 55. Const Generics

DISP should support compile-time value parameters.

Example:

```disp
struct Vector<T, const N: uint> {
    values: [T; N]
}
```

Usage:

```disp
Vector<f32, 4>
Vector<f32, 8>
```

---

# 56. Shape Types

For numerical programming:

```disp
Tensor<f32, [3, 224, 224]>
```

The compiler may verify compatible shapes before execution.

Invalid operations should fail at compile time when dimensions are statically known.

---

# 57. Units of Measure

DISP may eventually support dimensional types.

Conceptual example:

```disp
let distance: meters<f64> = 100.0
let time: seconds<f64> = 10.0

let speed = distance / time
```

The compiler could derive:

```text
meters_per_second<f64>
```

This remains experimental.

---

# 58. Type Refinement

DISP may support values narrowed by proven conditions.

Example:

```disp
if x != None {
    // compiler knows x is present here
}
```

Pattern matching should provide strong type narrowing.

---

# 59. Flow-Sensitive Typing

The compiler may refine types based on control flow.

Example:

```disp
match value {
    Some(user) => {
        print(user.name)
    }

    None => {
        ...
    }
}
```

Inside `Some`, `user` has type:

```text
User
```

not:

```text
Option<User>
```

---

# 60. No Implicit Any

DISP must not have an implicit universal dynamic type equivalent to unrestricted `any`.

If dynamic values are needed, they must use an explicit type.

Conceptually:

```text
Dynamic
```

or:

```text
Any
```

The user must opt into reduced static guarantees.

---

# 61. Dynamic Values

If DISP includes dynamic typing, it must be explicit.

Example:

```disp
let value: Dynamic = external_data
```

Operations on dynamic values must use checked runtime semantics.

---

# 62. Type Erasure

Type erasure must be explicit when static type information is intentionally removed.

Example:

```disp
let service: dyn Service = backend
```

The exact syntax remains provisional.

---

# 63. Trait Objects

Runtime polymorphism may use trait objects.

Conceptually:

```disp
dyn Display
```

This should make dynamic dispatch visible.

---

# 64. Static Dispatch

Generic trait calls should use static dispatch by default where possible.

Example:

```disp
fn render<T: Display>(value: T) {
    value.display()
}
```

The compiler may specialize this at compile time.

---

# 65. Dynamic Dispatch

Dynamic dispatch must be explicit.

Example:

```disp
fn render(value: &dyn Display) {
    value.display()
}
```

The runtime cost should not be hidden.

---

# 66. Monomorphization

Generic code may be specialized for concrete types.

Example:

```disp
identity<i32>()
identity<f64>()
```

may generate specialized machine code.

The compiler should control code-size growth intelligently.

---

# 67. Generic Sharing

Where specialization provides no useful benefit, the compiler may share implementations if semantics and performance guarantees permit.

This is an implementation optimization.

---

# 68. Coercions

Automatic coercions must be:

- safe
- unambiguous
- predictable
- non-lossy

Potential examples:

```text
&mut T -> &T
i8 -> i64
array reference -> slice
```

Dangerous coercions are forbidden.

---

# 69. Casting

Explicit casting should use clear syntax.

Example:

```disp
let x = i64(value)
```

Unchecked reinterpretation requires unsafe operations.

---

# 70. Bit Reinterpretation

Reinterpreting raw bit patterns must never be implicit.

Conceptual example:

```disp
unsafe {
    let bits = reinterpret<u32>(value)
}
```

The compiler must verify size and alignment constraints where possible.

---

# 71. Compile-Time Validation

Types should encode invariants when practical.

Example:

```disp
type Port(u16)
```

A stronger validated type may use constructors:

```disp
struct NonZero<T> {
    ...
}
```

Invalid states should be difficult or impossible to represent.

---

# 72. Private Fields

Types may protect invariants through private fields.

```disp
pub struct UserID {
    value: u64
}
```

External code cannot construct invalid values if construction is restricted to validated functions.

---

# 73. Constructor Validation

Example:

```disp
impl Port {
    fn new(value: u16) -> Result<Self, PortError> {
        if value == 0 {
            return Err(PortError.Invalid)
        }

        Ok(Self { value })
    }
}
```

The type system and visibility model together enforce invariants.

---

# 74. Phantom Types

DISP may support zero-runtime-cost marker types.

Conceptual example:

```disp
struct Connection<State> {
    socket: Socket
}
```

States:

```disp
type Closed
type Open
```

This can enforce valid API transitions at compile time.

---

# 75. Typestate

Example:

```disp
let connection: Connection<Closed>

let connection = connection.open()
```

The returned value becomes:

```text
Connection<Open>
```

Only open connections may expose methods such as:

```disp
connection.send(data)
```

---

# 76. Capability Types

Security-sensitive APIs may use types to represent authority.

Example:

```disp
fn delete_file(
    capability: FileDeleteCapability,
    path: Path
) {
    ...
}
```

Possession of the capability becomes part of what allows the operation.

---

# 77. Effect Awareness

DISP may eventually encode important effects into function signatures.

Potential effects include:

```text
IO
Network
Database
Unsafe
Async
GPU
```

The exact effect system remains experimental.

---

# 78. Pure Functions

DISP may permit marking functions as pure.

Conceptually:

```disp
pure fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

A pure function may not perform hidden external side effects.

This could improve:

- optimization
- testing
- reasoning
- security analysis
- parallelization

---

# 79. Compile-Time Purity

Compile-time functions may be required to satisfy stricter effect rules.

Example:

```disp
comptime fn generate_table() {
    ...
}
```

Compiler execution must not silently gain unrestricted system access.

---

# 80. Database Types

DISP's Data domain should integrate with the normal type system.

Example:

```disp
data User {
    id: UserID primary
    name: String
    active: bool
}
```

Queries must preserve static type information.

---

# 81. Query Result Types

Example:

```disp
let users =
    User
    .select(id, name)
```

The compiler should know the exact result structure.

A query selecting:

```text
id
name
```

must not expose fields that were not selected.

---

# 82. Null Database Fields

Nullable database fields should map to:

```disp
Option<T>
```

rather than introducing a separate unsafe null model.

Example:

```disp
bio: Option<String>
```

---

# 83. Page Types

DISP's Page system must also participate in static typing.

Example:

```disp
component UserCard(user: User) {
    text(user.name)
}
```

Passing a different incompatible type must fail at compile time.

---

# 84. Event Types

Events should have defined types.

Example:

```disp
on change(value: String) {
    ...
}
```

The compiler verifies event payload compatibility.

---

# 85. Reactive State Types

State is statically typed.

```disp
state count: i32 = 0
```

Assignments must preserve the declared type.

---

# 86. GPU Types

Host and accelerator resources may require distinct types.

Example:

```disp
HostBuffer<f32>
GpuBuffer<f32>
```

This prevents accidental use of memory from the wrong execution domain.

---

# 87. Device Types

DISP may eventually represent device placement in types.

Conceptual example:

```disp
Tensor<f32, [1024], GPU>
Tensor<f32, [1024], CPU>
```

Transfers between devices become explicit or compiler-verifiable.

---

# 88. Type-Safe FFI

Foreign types must be explicitly declared.

Example:

```disp
@repr(C)
struct CHeader {
    kind: u16
    size: u32
}
```

FFI declarations must preserve layout and calling-convention requirements.

---

# 89. Unsafe Foreign Types

Foreign values that cannot satisfy DISP's normal guarantees must remain behind unsafe boundaries.

Safe wrappers should expose validated DISP types.

---

# 90. Type Metadata

DISP may expose controlled type metadata.

Examples:

```text
size_of<T>()
align_of<T>()
type_name<T>()
```

Reflection must not silently destroy optimization or type safety.

---

# 91. Reflection

If runtime reflection exists, it must be explicit and constrained.

Compile-time reflection is preferred where possible.

This keeps runtime overhead low.

---

# 92. Serialization

Serialization should be type-aware.

Example:

```disp
let bytes = encode.json(user)
```

Deserialization:

```disp
let user = decode.json<User>(input)?
```

Invalid external data must not create invalid typed values without validation.

---

# 93. Versioned Types

DISP packages may eventually expose schema/version compatibility metadata.

This could support safer:

- APIs
- databases
- network protocols
- serialization
- package upgrades

The exact model remains open.

---

# 94. Public API Type Stability

Public exported types form part of package compatibility.

Breaking public type changes should be detectable by tooling.

---

# 95. Compiler Diagnostics

Type errors should explain:

1. expected type
2. received type
3. where each type originated
4. why conversion is unsafe or invalid
5. the smallest practical fix

Example diagnostic concept:

```text
expected: UserID
found:    AccountID

UserID and AccountID are distinct types even though both contain u64.
```

---

# 96. No Hidden Type Weakening

The compiler must never silently weaken a strongly typed expression into a less safe representation merely to make code compile.

Explicit programmer intent is required.

---

# 97. Unsafe Type Operations

Operations that violate normal static guarantees require:

```disp
unsafe {
    ...
}
```

Examples include:

- raw pointer reinterpretation
- unchecked layout assumptions
- arbitrary bit casting
- unverifiable foreign memory access

---

# 98. Type-System Performance

Type safety should compile away where possible.

Examples:

- newtypes should normally have zero runtime overhead
- generic specialization should avoid unnecessary indirection
- bounds proofs should eliminate redundant checks
- typestate should normally exist only at compile time
- trait constraints should not require runtime metadata unless dynamic dispatch is used

---

# 99. Type-System Security

The type system should make dangerous states difficult to represent.

Priority guarantees include:

- null safety
- ownership correctness
- initialized values
- bounds safety
- explicit failure
- safe concurrency
- safe numeric conversion
- capability isolation
- typed external data
- explicit unsafe boundaries

---

# 100. Type-System Simplicity

Ordinary code should remain simple:

```disp
fn main() {
    let name = "DISP"
    let age = 10

    print("{name} {age}")
}
```

A programmer should not need advanced type-system knowledge for simple programs.

---

# 101. Progressive Power

Advanced type features appear only when required.

Progression:

```text
inference
    ↓
explicit types
    ↓
structs and enums
    ↓
generics
    ↓
traits
    ↓
ownership types
    ↓
typestate
    ↓
capabilities
    ↓
unsafe low-level representations
```

DISP should not force the final levels onto beginners.

---

# 102. Type-System Rule

Every type-system feature must improve at least one of:

```text
correctness
safety
performance
expressiveness
developer clarity
```

without introducing disproportionate complexity.

---

# 103. Core Type Architecture

The initial DISP type architecture is:

```text
Static typing
    +
Strong typing
    +
Type inference
    +
Algebraic data types
    +
Generics
    +
Traits
    +
Non-null references
    +
Option and Result
    +
Ownership-aware types
    +
Concurrency-aware types
    +
Compile-time value types
    +
Explicit dynamic dispatch
    +
Explicit unsafe escape hatches
```

---

# 104. DISP Type Principle

> If the compiler can prove a property, the programmer should not have to prove it manually.

But:

> If the compiler cannot prove safety, DISP must not silently pretend that it can.

---

# DISP

**Data. Intelligence. System. Page.**

**Strong types. Simple code. Compile-time confidence.**
