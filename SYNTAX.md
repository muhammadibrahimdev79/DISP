# DISP Syntax Specification

## 0. Status

This document defines the initial syntax of DISP.

The syntax is experimental until explicitly marked stable.

DISP syntax must remain:

- readable
- compact
- consistent
- statically analyzable
- easy to learn
- suitable for both high-level and low-level programming

### 0.1 Current lexical decisions for the 1.0 candidate

The current compiler uses UTF-8 source. Identifiers follow Unicode XID start/continue rules (with `_` also permitted) and must be written in Unicode NFC form. Keywords remain exact ASCII spellings. Diagnostic columns count Unicode scalar values and spans use an end-exclusive position.

Line breaks are whitespace rather than indentation-based syntax. Semicolons remain optional when the grammar already determines a statement boundary. A value-less early return is written `return;` when another statement follows, avoiding ambiguity with a returned expression.

These decisions govern the current implementation but do not mark the rest of this document stable.

---

# 1. Source Files

DISP source files use:

```text
.disp
```

Example:

```text
main.disp
server.disp
database.disp
ui.disp
```

---

# 2. Hello World

```disp
fn main() {
    print("Hello, DISP!")
}
```

---

# 3. Comments

Single-line comments:

```disp
// This is a comment
```

Multi-line comments:

```disp
/*
This is
a multi-line comment
*/
```

Documentation comments:

```disp
/// Adds two numbers.
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

---

# 4. Variables

Immutable variables use:

```disp
let name = "DISP"
let version = 1
```

Mutable variables use:

```disp
var count = 0

count += 1
```

Explicit types:

```disp
let age: i32 = 20

var score: f64 = 99.5
```

---

# 5. Constants

Compile-time constants use:

```disp
const PI: f64 = 3.141592653589793
const MAX_USERS: u64 = 1_000_000
```

Constants cannot change at runtime.

---

# 6. Primitive Types

```disp
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

# 7. Boolean Values

```disp
let enabled = true
let disabled = false
```

---

# 8. Integers

```disp
let a = 10
let b = -50
let million = 1_000_000
```

Explicit type:

```disp
let value: u64 = 100
```

---

# 9. Floating-Point Values

```disp
let temperature = 21.5
let pi: f64 = 3.14159
```

---

# 10. Strings

```disp
let language = "DISP"
```

Interpolation:

```disp
let name = "Ibrahim"

print("Hello {name}")
```

Expressions may appear inside interpolation:

```disp
print("Result: {a + b}")
```

---

# 11. Characters

```disp
let letter: char = 'D'
```

---

# 12. Operators

Arithmetic:

```disp
a + b
a - b
a * b
a / b
a % b
```

Comparison:

```disp
a == b
a != b
a < b
a <= b
a > b
a >= b
```

Logical:

```disp
a && b
a || b
!a
```

Assignment:

```disp
x = 10

x += 1
x -= 1
x *= 2
x /= 2
```

Bitwise:

```disp
a & b
a | b
a ^ b
~a

a << 2
a >> 2
```

---

# 13. Functions

Basic function:

```disp
fn greet() {
    print("Hello")
}
```

Parameters:

```disp
fn greet(name: str) {
    print("Hello {name}")
}
```

Return type:

```disp
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

Explicit return:

```disp
fn add(a: i32, b: i32) -> i32 {
    return a + b
}
```

---

# 14. Named Arguments

Functions may support named arguments:

```disp
fn connect(host: str, port: u16) {
    ...
}
```

Call:

```disp
connect(
    host: "localhost",
    port: 8080
)
```

---

# 15. Default Arguments

```disp
fn connect(
    host: str = "localhost",
    port: u16 = 8080
) {
    ...
}
```

Call:

```disp
connect()
```

or:

```disp
connect(port: 9000)
```

---

# 16. If

```disp
if age >= 18 {
    print("Adult")
}
```

Else:

```disp
if age >= 18 {
    print("Adult")
} else {
    print("Minor")
}
```

Else-if:

```disp
if score >= 90 {
    print("A")
} else if score >= 80 {
    print("B")
} else {
    print("C")
}
```

---

# 17. If Expressions

`if` may produce a value.

```disp
let status =
    if age >= 18 {
        "adult"
    } else {
        "minor"
    }
```

---

# 18. Match

```disp
match status {
    Active => print("Active")
    Disabled => print("Disabled")
    Deleted => print("Deleted")
}
```

With values:

```disp
match number {
    0 => print("Zero")
    1 => print("One")
    _ => print("Other")
}
```

---

# 19. Pattern Matching

```disp
match user {
    Some(user) => print(user.name)
    None => print("No user")
}
```

Destructuring:

```disp
match point {
    Point { x: 0, y } => print("Y: {y}")
    Point { x, y } => print("{x}, {y}")
}
```

---

# 20. While Loops

```disp
var i = 0

while i < 10 {
    print(i)
    i += 1
}
```

---

# 21. For Loops

Ranges:

```disp
for i in 0..10 {
    print(i)
}
```

Inclusive range:

```disp
for i in 0..=10 {
    print(i)
}
```

Collections:

```disp
for user in users {
    print(user.name)
}
```

---

# 22. Loop

Infinite loop:

```disp
loop {
    work()
}
```

Exit:

```disp
loop {
    if finished {
        break
    }
}
```

---

# 23. Continue

```disp
for value in values {
    if value < 0 {
        continue
    }

    process(value)
}
```

---

# 24. Structures

```disp
struct User {
    id: u64
    name: String
    active: bool
}
```

Creation:

```disp
let user = User {
    id: 1
    name: "DISP"
    active: true
}
```

Access:

```disp
print(user.name)
```

---

# 25. Methods

```disp
impl User {
    fn greet(self: &Self) {
        print("Hello {self.name}")
    }
}
```

Call:

```disp
user.greet()
```

---

# 26. Constructors

```disp
impl User {
    fn new(id: u64, name: String) -> Self {
        Self {
            id
            name
            active: true
        }
    }
}
```

Usage:

```disp
let user = User.new(1, "DISP")
```

---

# 27. Enums

```disp
enum Status {
    Active
    Disabled
    Deleted
}
```

Enums may hold values:

```disp
enum Result<T, E> {
    Ok(T)
    Err(E)
}
```

Enum variants have the nominal identity of their declaring enum. The canonical constructor and pattern spelling is qualified:

```disp
Status.Active
Message.Text("hello")
```

An unqualified variant is accepted only when it resolves unambiguously. Qualification is required when packages or enums expose the same variant name. `Some`, `None`, `Ok`, and `Err` are prelude constructors for `Option` and `Result`; a user enum reusing those variant names must use qualification.

---

# 28. Option

Optional values use:

```disp
Option<T>
```

Values:

```disp
Some(value)
None
```

Example:

```disp
fn find_user(id: u64) -> Option<User> {
    ...
}
```

---

# 29. Result

Recoverable operations use:

```disp
Result<T, E>
```

Example:

```disp
fn read_file(path: str) -> Result<String, IOError> {
    ...
}
```

---

# 30. Error Propagation

DISP may use `?` to propagate errors.

```disp
fn load() -> Result<Data, Error> {
    let text = File.read("data.txt")?
    let data = parse(text)?

    Ok(data)
}
```

For `Result<T, E>`, `?` unwraps `Ok(T)` and immediately returns the unchanged `Err(E)` from the enclosing function. The enclosing function must return `Result<_, E>` with the same nominal error type; implicit error conversion is not performed.

For `Option<T>`, `?` unwraps `Some(T)` and immediately returns `None`. The enclosing function must return `Option<_>`.

---

# 31. Arrays

Fixed-size arrays:

```disp
let numbers: [i32; 4] = [1, 2, 3, 4]
```

Access:

```disp
let first = numbers[0]
```

---

# 32. Lists

Dynamic collections:

```disp
let numbers = List<i32>[1, 2, 3]
```

Mutable list:

```disp
var numbers = List<i32>[]

numbers.push(10)
numbers.push(20)
```

---

# 33. Maps

```disp
let users = Map<String, u64> {
    "Alice": 1
    "Bob": 2
}
```

Access:

```disp
let id = users["Alice"]
```

---

# 34. Tuples

```disp
let point = (10, 20)
```

Typed:

```disp
let result: (i32, String) = (200, "OK")
```

Destructuring:

```disp
let (code, message) = result
```

---

# 35. Ranges

Exclusive end:

```disp
0..10
```

Inclusive end:

```disp
0..=10
```

---

# 36. References

Immutable reference:

```disp
let reference = &value
```

Mutable reference:

```disp
let reference = &mut value
```

---

# 37. Move

Explicit move:

```disp
let second = move first
```

After the move, `first` cannot be used until assigned a new valid value.

---

# 38. Generics

```disp
fn identity<T>(value: T) -> T {
    value
}
```

Generic structure:

```disp
struct Box<T> {
    value: T
}
```

---

# 39. Constraints

Generic requirements:

```disp
fn max<T: Ordered>(a: T, b: T) -> T {
    if a > b {
        a
    } else {
        b
    }
}
```

Multiple constraints:

```disp
fn process<T: Clone + Send>(value: T) {
    ...
}
```

---

# 40. Traits

Reusable behavior:

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

The bootstrap compiler currently performs static trait dispatch only. Implementations must provide exactly the trait's methods and associated type definitions, overlapping implementations are rejected, and a call is rejected when multiple implemented traits expose the same applicable method name. Generic function type arguments are inferred from value arguments; explicit function turbofish syntax is not currently specified.

Until the ownership milestone implements references, executable trait methods use a value receiver (`self: Self`). The documented `self: &Self` receiver becomes available with borrowing; accepting it earlier would give `&` placeholder semantics. Associated types use `type Name` in a trait and `type Name = Concrete` in an implementation. The current specification does not yet define associated-type projection syntax, so projections are not accepted.

---

# 41. Modules

```disp
module math
```

Import:

```disp
use math
```

Specific import:

```disp
use math.sqrt
```

Multiple imports:

```disp
use math.{sqrt, pow}
```

---

# 42. Public API

Declarations are private by default.

Public declaration:

```disp
pub fn calculate() {
    ...
}
```

Public type:

```disp
pub struct User {
    pub id: u64
    name: String
}
```

---

# 43. Namespaces

Namespaces use modules rather than separate syntax.

Example:

```disp
use net.http

let server = http.Server()
```

---

# 44. Async Functions

```disp
async fn fetch_data() -> Data {
    ...
}
```

Await:

```disp
let data = await fetch_data()
```

---

# 45. Tasks

```disp
let task = spawn process()
```

Structured tasks:

```disp
task.group {
    spawn fetch_users()
    spawn fetch_posts()
}
```

---

# 46. Parallel Loops

Conceptual syntax:

```disp
parallel for item in items {
    process(item)
}
```

Parallel execution must preserve DISP's safety guarantees.

---

# 47. Closures

```disp
let add = |a, b| {
    a + b
}
```

Short form:

```disp
let square = |x| x * x
```

Usage:

```disp
let result = square(10)
```

---

# 48. Pipelines

DISP may support pipeline syntax:

```disp
let result =
    data
    |> filter(valid)
    |> map(transform)
    |> collect()
```

The pipeline operator passes the previous result into the next operation.

---

# 49. Data Queries

DISP provides typed data-query operations.

```disp
let adults =
    users
    .where(age >= 18)
    .select(name, email)
```

Sorting:

```disp
let users =
    users
    .order_by(name)
```

Limit:

```disp
let users =
    users
    .limit(100)
```

---

# 50. Database Definition

Conceptual syntax:

```disp
data User {
    id: u64 primary
    name: String
    email: String unique
    active: bool
}
```

Queries remain statically typed.

---

# 51. Database Access

```disp
let user =
    User
    .where(id == 10)
    .first()
```

Insert:

```disp
User.insert {
    name: "Alice"
    email: "alice@example.com"
    active: true
}
```

Update:

```disp
User
    .where(id == 10)
    .update {
        active: false
    }
```

Delete:

```disp
User
    .where(id == 10)
    .delete()
```

---

# 52. Transactions

```disp
transaction {
    account_a.balance -= 100
    account_b.balance += 100
}
```

Failure should roll back the transaction.

---

# 53. Intelligence Types

DISP may provide first-class numerical types.

```disp
let values: Tensor<f32> = ...
```

Shape-aware types may be supported:

```disp
Tensor<f32, [1024, 1024]>
```

---

# 54. GPU Execution

Conceptual syntax:

```disp
gpu fn multiply(
    a: Tensor<f32>,
    b: Tensor<f32>
) -> Tensor<f32> {
    ...
}
```

Execution:

```disp
let result = gpu multiply(a, b)
```

---

# 55. SIMD

Conceptual syntax:

```disp
let values: simd<f32, 8>
```

The compiler should automatically vectorize normal code whenever safe and profitable.

---

# 56. Pages

DISP page definitions:

```disp
page Home {
    text("Hello, DISP!")
}
```

Components:

```disp
page Home {
    Column {
        text("Welcome")
        button("Continue")
    }
}
```

---

# 57. Components

Reusable UI:

```disp
component UserCard(user: User) {
    Column {
        text(user.name)
        text(user.email)
    }
}
```

Usage:

```disp
UserCard(user)
```

---

# 58. Styling

```disp
style UserCard {
    width: 100%
    padding: 16px
    radius: 12px
}
```

Styles remain part of DISP's typed page system rather than an unrelated language.

---

# 59. Events

```disp
button("Login") {
    on click {
        login()
    }
}
```

Input:

```disp
input {
    value: username

    on change(value) {
        username = value
    }
}
```

---

# 60. Reactive State

Conceptual syntax:

```disp
state count = 0
```

Usage:

```disp
button("Count: {count}") {
    on click {
        count += 1
    }
}
```

State changes trigger only the required UI updates.

---

# 61. Page Routing

```disp
route "/" -> Home
route "/login" -> Login
route "/user/{id}" -> UserPage
```

---

# 62. Server Routes

```disp
route GET "/api/users" {
    return users
}
```

Parameters:

```disp
route GET "/api/users/{id}" {
    let user = find_user(id)
    return user
}
```

---

# 63. JSON

DISP should provide typed serialization.

```disp
let json = encode.json(user)
```

Parsing:

```disp
let user = decode.json<User>(input)?
```

---

# 64. Compile-Time Execution

Compile-time execution may use:

```disp
comptime {
    ...
}
```

Example:

```disp
const TABLE = comptime generate_table()
```

Compile-time code must obey defined resource and security restrictions.

---

# 65. Attributes

Metadata:

```disp
@test
fn addition_test() {
    assert(add(2, 2) == 4)
}
```

Other attributes may include:

```disp
@inline
@deprecated
@export
@repr
```

Attributes must have defined semantics.

---

# 66. Tests

```disp
@test
fn test_addition() {
    assert(2 + 2 == 4)
}
```

Run:

```text
disp test
```

---

# 67. Assertions

```disp
assert(value > 0)
```

With message:

```disp
assert(value > 0, "value must be positive")
```

---

# 68. Unsafe

Unsafe operations require:

```disp
unsafe {
    ...
}
```

Example:

```disp
unsafe {
    let value = *pointer
}
```

Unsafe blocks must remain explicit and auditable.

---

# 69. Raw Pointers

```disp
let pointer: ptr<i32>
let pointer: mut ptr<i32>
```

Dereferencing requires unsafe code.

---

# 70. Foreign Functions

Conceptual C interoperability:

```disp
extern "C" {
    fn malloc(size: uint) -> ptr<void>
}
```

Calls requiring unverifiable safety must occur within unsafe boundaries.

---

# 71. Memory Layout

Explicit representation:

```disp
@repr(C)
struct Header {
    kind: u16
    size: u32
}
```

Packed representations must be explicit.

---

# 72. Defer

DISP may support scope-exit execution:

```disp
let file = File.open("data")

defer file.close()
```

For ordinary resource types, deterministic ownership cleanup should usually make manual `defer` unnecessary.

---

# 73. Type Aliases

```disp
type UserID = u64
```

Generic alias:

```disp
type UserResult<T> = Result<T, UserError>
```

---

# 74. Newtypes

Distinct strong types:

```disp
type UserID(u64)
type AccountID(u64)
```

These are not implicitly interchangeable.

---

# 75. Conversion

Explicit conversion:

```disp
let value = i64(number)
```

Potentially unsafe or lossy conversions must not happen silently.

Checked conversion:

```disp
let value = i32.try_from(number)?
```

---

# 76. Inference

DISP should infer types where unambiguous.

```disp
let number = 10
let name = "DISP"
```

The compiler must still assign static types.

Dynamic typing must not silently replace static guarantees.

## Current numeric semantics

`int` is a checked signed 64-bit integer and `uint` is a checked unsigned 64-bit integer. The exact-width integer types use the range named by their spelling; `f32` and `f64` retain their distinct IEEE-754 representations. Integer literals are arbitrary-width through `u128` and are contextually checked against an annotation, parameter, field, or return type. An unconstrained integer literal defaults to `int`; an unconstrained floating-point literal defaults to `f64`.

Implicit conversion is limited to lossless widening: signed-to-wider-signed, unsigned-to-wider-unsigned, unsigned-to-a-strictly-wider-signed type, and `f32` to `f64`. Signed and unsigned values are otherwise not mixed implicitly. `i8(value)` and the other numeric type constructors perform explicit checked conversion and fail at runtime if a dynamic value is out of range. `i8.try_from(value)` returns `Result<i8, ConversionError>` instead, so it composes with `?`.

Ordinary integer arithmetic is checked. Integer values provide `wrapping_add`, `wrapping_sub`, `wrapping_mul`, `saturating_add`, `saturating_sub`, and `saturating_mul`; each requires an operand of the same concrete integer type.

---

# 77. Semicolons

Semicolons are not required for ordinary statements.

Preferred:

```disp
let x = 10
let y = 20

print(x + y)
```

DISP should use newline and grammar structure rather than mandatory semicolons.

---

# 78. Blocks

Blocks use braces:

```disp
{
    let x = 10
    print(x)
}
```

Indentation improves readability but does not define scope.

---

# 79. Naming

Recommended conventions:

```text
variables       snake_case
functions       snake_case
modules         snake_case
types           PascalCase
traits          PascalCase
constants       UPPER_SNAKE_CASE
```

Example:

```disp
const MAX_USERS = 1000

struct UserAccount {
    ...
}

fn create_account() {
    ...
}
```

---

# 80. Reserved Keywords

Initial reserved words may include:

```text
let
var
const

fn
return

if
else
match

for
in
while
loop
break
continue

struct
enum
trait
impl
type

module
use
pub

async
await
spawn
parallel

move
mut

unsafe
extern

data
transaction

page
component
style
state
route

comptime

true
false

Self
self
```

This list remains provisional.

---

# 81. Syntax Philosophy

DISP should avoid unnecessary punctuation.

Preferred:

```disp
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

Not excessive symbolic syntax that makes common code difficult to read.

---

# 82. Explicitness Rule

DISP should require explicit syntax when an operation can significantly affect:

- safety
- ownership
- concurrency
- allocation
- external effects
- lossy conversion
- low-level hardware behavior

Normal harmless operations should remain concise.

---

# 83. Consistency Rule

The same concept should use the same syntax across DISP.

DISP must avoid introducing multiple unrelated ways to express the same fundamental operation unless there is a measurable benefit.

---

# 84. Simplicity Rule

A beginner should be able to write:

```disp
fn main() {
    let name = input("Name: ")

    print("Hello {name}")
}
```

without understanding:

- pointers
- ownership internals
- allocators
- lifetimes
- FFI
- compiler internals

Advanced control becomes visible only when required.

---

# 85. Power Rule

The same language must also permit:

```disp
unsafe {
    let register = mut ptr<u32>(0x4000_0000)

    *register = 1
}
```

when writing legitimate low-level systems software.

---

# 86. Unified Language Rule

Data, Intelligence, System, and Page constructs belong to the same language.

They must share:

- syntax principles
- type system
- module system
- error model
- package system
- compiler
- tooling
- security model

DISP must not become four unrelated languages hidden behind one name.

---

# 87. Example DISP Program

```disp
struct User {
    id: u64
    name: String
}

fn greet(user: &User) {
    print("Hello {user.name}")
}

fn main() {
    let user = User {
        id: 1
        name: "DISP"
    }

    greet(&user)
}
```

---

# 88. Example Full-Stack Direction

```disp
data User {
    id: u64 primary
    name: String
}

route GET "/api/users" {
    return User.select(id, name)
}

page Home {
    state users = await fetch("/api/users")

    Column {
        text("Users")

        for user in users {
            text(user.name)
        }
    }
}

style Home {
    width: 100%
    padding: 24px
}

fn main() {
    run(Home)
}
```

This demonstrates DISP's intended direction:

```text
Data
+
Backend
+
Logic
+
Page
+
Style
```

inside one coherent language.

---

# 89. Syntax Stability

No syntax is permanently locked until:

1. parsing is implemented
2. semantics are defined
3. compiler behavior is tested
4. real programs are written
5. usability is evaluated
6. performance impact is measured
7. security implications are reviewed

Syntax must serve semantics rather than aesthetics alone.

---

# 90. DISP Syntax Principle

> Simple code should look simple.
>
> Powerful code should remain understandable.
>
> Dangerous code should look dangerous.

---

# DISP

**Data. Intelligence. System. Page.**

**One syntax. One type system. One language.**
