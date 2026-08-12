# DISP Standard Library

> **Design draft:** GPT-generated and not authoritative. See [the documentation index](../README.md) for current, test-backed behavior.

## 0. Status

This document defines the initial standard-library architecture for DISP.

The standard library is experimental until explicitly stabilized.

Its goals are:

- simplicity
- security
- performance
- portability
- consistency
- minimal dependencies
- pay-for-what-you-use deployment

---

# 1. Core Principle

> Common things should be easy. Expensive things should be visible. Dangerous things should be explicit.

The DISP standard library must feel like one coherent system rather than a collection of unrelated APIs.

---

# 2. Library Structure

Initial module hierarchy:

```text
core
├── option
├── result
├── memory
├── iter
└── traits

std
├── collections
├── text
├── math
├── io
├── fs
├── path
├── net
├── http
├── time
├── async
├── sync
├── process
├── env
├── random
├── crypto
├── encode
├── data
├── intelligence
├── gpu
├── page
├── system
├── testing
└── diagnostics
```

---

# 3. `core`

`core` contains functionality required by almost every DISP program.

It must work without:

```text
operating system
heap allocator
filesystem
network
threads
runtime executor
```

where the target permits.

---

# 4. Core Types

Essential types include:

```text
Option<T>
Result<T, E>
Never
Unit
Range<T>
Slice<T>
MutSlice<T>
```

---

# 5. Core Traits

Initial fundamental traits may include:

```text
Equal
Ordered
Hash
Clone
Copy
Drop
Display
Debug
Iterator
IntoIterator
Default
Send
Share
```

---

# 6. Option

```disp
let user: Option<User>
```

Values:

```disp
Some(user)
None
```

Useful operations:

```disp
value.is_some()
value.is_none()
value.map(...)
value.unwrap_or(...)
value.expect(...)
```

`Option<T>` replaces ordinary nullable values.

---

# 7. Result

```disp
Result<T, E>
```

Values:

```disp
Ok(value)
Err(error)
```

Operations:

```disp
result.is_ok()
result.is_err()
result.map(...)
result.map_error(...)
result.unwrap_or(...)
```

Error propagation:

```disp
let value = operation()?
```

---

# 8. Collections

The standard collection library should include:

```text
Array
List<T>
Deque<T>
Map<K, V>
Set<T>
Heap<T>
LinkedList<T>
```

Only broadly useful structures should be standardized.

---

# 9. List

```disp
var numbers = List<i32>[]

numbers.push(10)
numbers.push(20)
```

Operations may include:

```text
push
pop
insert
remove
get
first
last
clear
reserve
capacity
len
is_empty
```

---

# 10. Map

```disp
var users = Map<UserID, User>()
```

Operations:

```text
insert
remove
get
contains
keys
values
entries
len
clear
```

Hashing must resist algorithmic complexity attacks where untrusted keys are common.

---

# 11. Set

```disp
var ids = Set<UserID>()

ids.insert(id)
```

Operations:

```text
insert
remove
contains
union
intersection
difference
```

---

# 12. Iterators

Collections should expose a unified iterator model.

Example:

```disp
for item in items {
    process(item)
}
```

Functional operations:

```disp
let result =
    items
    .filter(valid)
    .map(transform)
    .collect()
```

---

# 13. Lazy Iteration

Iterator transformations should normally remain lazy.

Example:

```disp
let values =
    numbers
    .filter(|x| x > 0)
    .map(|x| x * 2)
```

No intermediate collection should be required unless requested.

---

# 14. Text

Text modules should distinguish:

```text
String
str
bytes
Unicode scalar values
graphemes
```

The behavior of each must be explicit.

---

# 15. String

Owned UTF-8 text:

```disp
let name = String("DISP")
```

Common operations:

```text
len
is_empty
push
contains
starts_with
ends_with
replace
split
trim
```

---

# 16. String Views

Borrowed text should use:

```text
str
```

Example:

```disp
fn greet(name: str) {
    print("Hello {name}")
}
```

Borrowing text should not require allocation.

---

# 17. Unicode

DISP must clearly distinguish:

```text
bytes
Unicode code points
user-visible characters
```

APIs must avoid pretending these are always equivalent.

---

# 18. Formatting

Formatting should use interpolation:

```disp
print("User: {user.name}")
```

Formatting APIs should be type-safe.

---

# 19. Parsing

Typed parsing:

```disp
let age = parse<i32>("25")?
```

Invalid input must return an explicit error.

---

# 20. Math

The math module should provide:

```text
abs
min
max
sqrt
pow
log
exp
sin
cos
tan
floor
ceil
round
```

Behavior must be defined for supported numeric types.

---

# 21. Numeric Constants

Examples:

```disp
math.PI
math.E
math.TAU
```

Constants should have defined precision.

---

# 22. Checked Arithmetic

```disp
a.checked_add(b)
a.checked_sub(b)
a.checked_mul(b)
a.checked_div(b)
```

---

# 23. Saturating Arithmetic

```disp
a.saturating_add(b)
```

---

# 24. Wrapping Arithmetic

```disp
a.wrapping_add(b)
```

Overflow behavior must never be ambiguous.

---

# 25. SIMD

Portable SIMD should exist through:

```text
simd<T, N>
```

Example:

```disp
let values: simd<f32, 8>
```

The compiler may map operations to target-specific vector instructions.

---

# 26. Filesystem

Filesystem functionality belongs to:

```text
std.fs
```

Example:

```disp
let file = File.open("data.txt")?
```

---

# 27. File Reading

```disp
let text = fs.read_text("config.disp")?
let bytes = fs.read("file.bin")?
```

---

# 28. File Writing

```disp
fs.write_text("output.txt", text)?
fs.write("output.bin", bytes)?
```

---

# 29. Safe Filesystem APIs

Filesystem APIs must account for:

```text
permissions
symlinks
race conditions
path traversal
atomic replacement
partial writes
```

Secure variants should be easy to use.

---

# 30. Paths

Paths must not be represented as ordinary strings internally.

Use:

```text
Path
PathBuf
```

Example:

```disp
let path = Path("src/main.disp")
```

Platform path semantics should remain encapsulated.

---

# 31. Directory Operations

Operations may include:

```text
create
remove
list
walk
rename
copy
metadata
```

Recursive operations must make potentially expensive behavior obvious.

---

# 32. I/O

Core I/O abstractions:

```text
Reader
Writer
Seek
BufferedReader
BufferedWriter
```

These should compose across:

```text
files
network streams
memory buffers
compression
encryption
```

---

# 33. Networking

Networking module:

```text
std.net
```

Core types:

```text
IpAddress
SocketAddress
TcpSocket
TcpListener
UdpSocket
```

The implemented address layer uses a compact Copy `IpAddress` value for IPv4 and IPv6. Parsing is
strict, formatting is canonical, and no text allocation is retained inside the address:

```disp
address = IpAddress.parse("2001:db8::1")?
print(address.as_string())
print(address.is_ipv6())
endpoint = SocketAddress(address, 443)
```

`is_ipv4()`, `is_ipv6()`, `is_loopback()`, and `is_unspecified()` are allocation-free. A
`SocketAddress` may be constructed from an `IpAddress`, `String`, or borrowed `str`.

DNS resolution returns a sorted, deduplicated, owned `List<IpAddress>` so results cannot borrow
resolver or operating-system storage:

```disp
addresses = Dns.resolve("example.com")?
addresses = await Async.resolve("example.com")?
addresses = await Async.resolve_timeout("example.com", Duration.from_seconds(5))?
```

Async DNS futures are lazy: resolution and any timeout begin on first poll. Dropping or timing out
a future discards late results safely while the runtime drains its native worker before shutdown.

---

# 34. TCP

The currently implemented client foundation uses an explicit validated address and a lazy
connect future. A stream is owned, non-Copy, and closed deterministically when dropped:

```disp
connected = await Async.connect(SocketAddress("example.com", 443))
var stream = connected?
stream.write(request_bytes)?
response = stream.read(4096)?
stream.close()
```

`read` and `write` return `Result<_, NetworkError>`. Reads are bounded to 16 MiB per call,
and text protocols must explicitly validate or decode the returned `List<u8>`.

Streams also provide lazy readiness-polled operations. Their deadlines begin on first poll,
not when the future is constructed:

```disp
connected = await Async.connect_timeout(address, Duration.from_seconds(5))
var stream = connected?
written = await stream.write_async_timeout(bytes, Duration.from_seconds(2))
response = await stream.read_async_timeout(4096, Duration.from_seconds(2))
```

`read_async` and `write_async` are the variants without deadlines. A zero-length successful
read is EOF. `shutdown_read()` and `shutdown_write()` explicitly half-close one direction;
operations on that direction then return `Err(NetworkError)`. One read and one write may progress
concurrently, while multiple operations in the same direction are serialized. Async writes own a
copy of their input bytes, and stream futures retain reference-counted native state, so closing or
dropping the owner cannot produce dangling access.

Owned TCP servers bind through the same validated address type. Accept futures retain the
underlying listener state without exposing lifetimes or permitting use-after-close:

```disp
bound = TcpListener.bind(SocketAddress("127.0.0.1", 8080))
var listener = bound?
connection = await listener.accept_timeout(Duration.from_seconds(30))
var stream = connection?
```

`local_port()` returns `Result<uint, NetworkError>`, which supports safe operating-system
port assignment with port zero. `accept()` and `accept_timeout()` are lazy nonblocking futures;
closing or dropping the listener safely terminates pending accepts. High-level protocol clients
remain design work.

---

# 35. UDP

The implemented UDP foundation preserves message boundaries and sender identity:

```disp
bound = UdpSocket.bind(SocketAddress("0.0.0.0", 9000))
var socket = bound?
packet = await socket.receive_from_async_timeout(65535, Duration.from_seconds(30))
datagram = packet?
socket.send_to(datagram.bytes(), datagram.source())?
```

`bind` and `local_port` use the same validated owned address model as TCP. `send_to` and
`receive_from` are synchronous; `send_to_async`, `receive_from_async`, and their `_timeout`
variants are lazy readiness-polled futures. Outgoing futures copy both the byte sequence and
destination address, so caller mutation or drop cannot change pending output. One send and one
receive may progress independently, while operations in the same direction are serialized.

`UdpDatagram` owns its bytes and source address. `bytes()` and `source()` return independent owned
copies; `len()` and `is_empty()` do not allocate. UDP payloads are limited to 65,507 bytes and
receive limits to 65,535 bytes. If a packet is larger than the requested receive limit, that packet
is consumed and the operation returns `Err(NetworkError)` instead of silently exposing truncated
data. Zero-length datagrams remain successful, distinct messages. Closing or dropping a socket
invalidates pending operations through retained reference-counted state without dangling access.

---

# 36. HTTP

HTTP should be an official high-level module.

Example:

```disp
let response = await http.get("https://example.com")?
```

---

# 37. HTTP Client

Conceptually:

```disp
let client = HttpClient()

let response =
    await client.get(url)?
```

Configurable behavior:

```text
timeouts
redirects
headers
TLS
connection pooling
body limits
```

Secure defaults are mandatory.

---

# 38. HTTP Server

Example:

```disp
let server = HttpServer()

server.route(GET, "/hello", hello)

await server.run(":8080")
```

The Page/backend syntax may provide higher-level integration.

---

# 39. HTTP Limits

Servers should make limits straightforward:

```text
request size
header size
body size
connection count
timeouts
concurrency
```

Unbounded input must not be the easy default.

---

# 40. Time

Time APIs must distinguish:

```text
Instant
Duration
Date
Time
DateTime
Timezone
```

---

# 41. Duration

Example:

```disp
let timeout = 5.seconds
let delay = 250.milliseconds
```

---

# 42. Monotonic Time

Performance measurement:

```disp
let start = Instant.now()

work()

let elapsed = start.elapsed()
```

must use monotonic time.

---

# 43. Date and Time

Example:

```disp
let now = DateTime.now()
```

Timezone-sensitive behavior must be explicit.

---

# 44. Async

The async library should provide:

```text
Task<T>
spawn
sleep
timeout
select
join
task groups
channels
streams
```

---

# 45. Spawn

```disp
let task = spawn calculate()
```

Structured spawning should be preferred.

---

# 46. Join

```disp
let result = await task
```

Failures and cancellation must be typed.

---

# 47. Timeout

```disp
let result =
    await timeout(
        5.seconds,
        fetch()
    )?
```

---

# 48. Synchronization

The synchronization module should include:

```text
Mutex<T>
RwLock<T>
Atomic<T>
Semaphore
Barrier
Once
Channel<T>
```

---

# 49. Mutex

```disp
let state = Mutex(AppState())
```

Lock guards should release automatically through ownership.

---

# 50. Channels

```disp
let channel = Channel<Message>(capacity: 100)
```

Operations:

```disp
channel.send(message)
channel.receive()
```

Bounded channels should be easy and efficient.

---

# 51. Process

Process APIs:

```disp
let process =
    Process.spawn("git", ["status"])?
```

Access to subprocess execution may require capabilities in sandboxed profiles.

---

# 52. Environment

```disp
let home = env.get("HOME")
```

Missing variables return:

```text
Option<String>
```

rather than invalid empty values.

---

# 53. Randomness

Two categories must remain distinct:

```text
random
crypto.random
```

---

# 54. Fast Randomness

For simulations:

```disp
let rng = Random(seed: 123)
```

This may be deterministic.

---

# 55. Secure Randomness

For security:

```disp
let bytes = crypto.random_bytes(32)?
```

This must use a cryptographically secure source.

---

# 56. Cryptography Philosophy

DISP should provide safe cryptographic APIs while avoiding casual construction of insecure protocols.

High-level operations should be preferred over raw primitives.

---

# 57. Hashing

Potential cryptographic hashes:

```text
SHA-256
SHA-384
SHA-512
SHA-3
BLAKE2
BLAKE3
```

Actual inclusion depends on standards, ecosystem needs, and security review.

---

# 58. Password Hashing

Password or credential hashing should expose dedicated APIs.

Example:

```disp
let hash = crypto.password.hash(secret)?
```

Verification:

```disp
crypto.password.verify(secret, hash)?
```

Algorithms and parameters must be upgradeable.

---

# 59. MAC

Authenticated hashing:

```text
HMAC
```

should use dedicated types rather than manually concatenating secrets and messages.

---

# 60. Authenticated Encryption

High-level authenticated encryption APIs should be preferred.

Example concept:

```disp
let sealed =
    crypto.aead.encrypt(
        key,
        nonce,
        plaintext,
        associated_data
    )?
```

Unauthenticated encryption should not be the easy default.

---

# 61. Secret Types

Sensitive information may use:

```disp
Secret<String>
SecureBuffer
```

These types should:

```text
redact debug output
restrict accidental copying
support zeroization
```

where meaningful.

---

# 62. Encoding

Encoding module:

```text
std.encode
```

Potential formats:

```text
JSON
Base64
Hex
UTF
binary serialization
```

---

# 63. JSON

```disp
let json = encode.json(user)
```

Decode:

```disp
let user = decode.json<User>(input)?
```

Parsing must validate external input.

---

# 64. Binary Encoding

Binary APIs must define:

```text
endianness
integer width
alignment
length encoding
versioning
```

No machine-dependent implicit binary representation should be used for portable formats.

---

# 65. Compression

Potential module:

```text
std.compress
```

Supported algorithms should be selected based on adoption, interoperability, security, and performance.

Compression APIs must protect against decompression bombs through configurable limits.

---

# 66. Data

DISP Data functionality should integrate database access with the type system.

Example:

```disp
let users =
    User
    .where(active == true)
    .select(id, name)
```

---

# 67. Database Connections

Conceptual:

```disp
let db = Database.connect(config)?
```

Database drivers should use one shared interface where feasible.

---

# 68. Database Drivers

Potential support:

```text
PostgreSQL
SQLite
MySQL-compatible systems
distributed databases
embedded databases
```

Drivers may be packages rather than all living in the minimal standard library.

---

# 69. Prepared Queries

Parameters must be bound safely.

Unsafe string concatenation should never be required for ordinary database operations.

---

# 70. Transactions

```disp
transaction db {
    ...
}
```

or:

```disp
db.transaction {
    ...
}
```

Exact syntax must remain consistent with the language specification.

---

# 71. Connection Pools

Server applications should have:

```text
bounded pools
timeouts
health checks
cancellation
```

---

# 72. Data Frames

For analytics:

```text
DataFrame
Series<T>
Column<T>
```

may become standard Intelligence/Data types.

---

# 73. Columnar Processing

Analytics operations should support:

```text
vectorization
SIMD
zero-copy views
parallel execution
columnar layouts
```

---

# 74. Intelligence

The Intelligence library should provide foundational numerical and machine-learning abstractions.

Core concepts:

```text
Tensor<T>
Shape
Device
Model
Gradient
Optimizer
Dataset
```

---

# 75. Tensor

Example:

```disp
let tensor = Tensor<f32>.zeros([1024, 1024])
```

---

# 76. Shape Checking

Where dimensions are static:

```disp
Tensor<f32, [32, 128]>
```

the compiler should detect invalid operations where possible.

---

# 77. Tensor Operations

Standard operations may include:

```text
add
subtract
multiply
matmul
transpose
reshape
reduce
softmax
convolution
normalization
```

---

# 78. Automatic Differentiation

DISP Intelligence may support:

```disp
let gradient = grad(loss)
```

Automatic differentiation should integrate with normal language semantics.

---

# 79. Models

Conceptual:

```disp
model Network {
    ...
}
```

or standard typed structures.

DISP must avoid forcing an entirely separate language for AI models.

---

# 80. Devices

Device abstraction:

```text
CPU
GPU
Accelerator
```

Example:

```disp
let device = Device.best()
```

---

# 81. Device Placement

```disp
let tensor = tensor.to(GPU)
```

Expensive memory transfers should be visible.

---

# 82. GPU

GPU library should expose both:

```text
high-level tensor operations
low-level kernels
```

---

# 83. GPU Kernels

```disp
gpu fn add(a: Slice<f32>, b: Slice<f32>) {
    ...
}
```

The compiler must validate device restrictions.

---

# 84. GPU Portability

DISP should avoid locking ordinary code to one GPU vendor.

Backends may target:

```text
SPIR-V
PTX
WebGPU
platform GPU APIs
```

where appropriate.

---

# 85. System

System APIs provide controlled access to low-level functionality.

Potential modules:

```text
system.memory
system.thread
system.process
system.hardware
system.ffi
system.os
```

Many operations may require `unsafe`.

---

# 86. Raw Memory

Low-level allocation:

```disp
unsafe {
    ...
}
```

Safe wrappers should be preferred everywhere else.

---

# 87. Memory Mapping

System applications may require:

```text
memory-mapped files
hardware registers
shared memory
```

These must expose strict lifetime and synchronization rules.

---

# 88. FFI

Foreign-function support belongs to:

```text
system.ffi
```

Safe wrapper generation may eventually be automated.

---

# 89. Page

The Page library should unify:

```text
components
layout
style
events
state
routing
forms
accessibility
rendering
```

---

# 90. Components

```disp
component Greeting(name: str) {
    text("Hello {name}")
}
```

Components are statically typed.

---

# 91. Layout

Core layout primitives may include:

```text
Row
Column
Stack
Grid
Scroll
Container
```

Example:

```disp
Column {
    text("Hello")
    button("Continue")
}
```

---

# 92. Styling

```disp
style Card {
    padding: 16px
    radius: 12px
}
```

Style values should be typed.

Invalid values should fail before runtime.

---

# 93. Responsive Layout

DISP Page should support responsive interfaces without requiring raw CSS.

Conceptual example:

```disp
when width < 600px {
    ...
}
```

Exact syntax remains provisional.

---

# 94. State

```disp
state count = 0
```

Reactive changes should update only dependent interface elements.

---

# 95. Events

```disp
button("Save") {
    on click {
        save()
    }
}
```

Event payloads must be typed.

---

# 96. Forms

Forms should integrate:

```text
validation
typed input
error states
submission
accessibility
```

without forcing manual DOM manipulation.

---

# 97. Accessibility

Accessibility must be a first-class Page requirement.

Components should support:

```text
semantic roles
labels
keyboard control
focus management
screen readers
contrast metadata
```

The compiler or linter should detect common accessibility errors.

---

# 98. Routing

```disp
route "/" -> Home
route "/users/{id}" -> UserPage
```

Route parameters should be typed where possible.

---

# 99. Page Rendering Targets

Potential targets:

```text
browser
desktop
mobile
server rendering
static rendering
```

The same component system should be reused where practical.

---

# 100. Testing

Testing is part of the official library.

```disp
@test
fn addition() {
    assert(add(2, 2) == 4)
}
```

---

# 101. Assertions

```disp
assert(value == expected)
```

Additional helpers:

```text
assert_equal
assert_not_equal
assert_error
assert_matches
```

---

# 102. Test Isolation

Tests should support:

```text
temporary directories
mock clocks
deterministic randomness
isolated environment variables
network restrictions
```

---

# 103. Property Testing

DISP should eventually support:

```text
property-based testing
```

Example concept:

```disp
@property
fn reverse_twice(values: List<i32>) {
    assert(values.reverse().reverse() == values)
}
```

---

# 104. Benchmarking

Benchmark APIs:

```disp
@bench
fn parser_speed(bench: &mut Bench) {
    ...
}
```

Benchmarks should report statistically meaningful results.

---

# 105. Diagnostics

Standard diagnostic facilities may include:

```text
log
trace
metrics
backtrace
```

These must remain optional.

---

# 106. Logging

```disp
log.info("server started")
log.warn("high memory usage")
log.error("request failed")
```

Structured fields:

```disp
log.info(
    "request completed",
    status: 200,
    duration: elapsed
)
```

---

# 107. Secret Redaction

Sensitive types should not expose their values through ordinary logging.

Example:

```disp
let token: Secret<String>
log.debug(token)
```

should display a redacted representation.

---

# 108. Standard Error Types

Libraries should use structured typed errors.

Avoid APIs whose only failure representation is:

```text
integer error code
string message
```

Error types should support useful context.

---

# 109. Resource Limits

APIs processing external data should allow limits.

Examples:

```text
maximum file size
maximum request size
maximum JSON depth
maximum decompressed size
maximum database rows
maximum redirects
```

---

# 110. Cancellation

Potentially long-running APIs should support cancellation where appropriate.

Examples:

```text
network requests
database queries
file operations
GPU workloads
async tasks
```

---

# 111. Timeouts

Network and external-resource APIs should make timeout configuration straightforward.

Infinite waits should not silently become universal defaults.

---

# 112. Backpressure

Streaming APIs must support bounded flow.

Example:

```disp
async for chunk in response.body {
    process(chunk)
}
```

Slow consumers should not automatically cause unlimited memory growth.

---

# 113. Zero-Copy APIs

Where practical, the standard library should expose borrowed views.

Example:

```disp
let slice = buffer.slice(100..200)
```

Copying should occur only when ownership or transformation actually requires it.

---

# 114. Allocation Awareness

APIs that allocate substantially should make this understandable through:

```text
documentation
naming
types
compiler diagnostics
```

where appropriate.

---

# 115. Platform Abstraction

Portable modules should abstract OS differences.

Example:

```disp
fs.read(...)
```

rather than forcing application code to use:

```text
Win32
POSIX
Darwin APIs
```

---

# 116. Platform-Specific APIs

Advanced applications may still access platform-specific modules:

```text
system.windows
system.linux
system.macos
```

These APIs are explicitly non-portable.

---

# 117. Standard Library Profiles

Not every target includes every module.

Example:

```text
core
native
server
embedded
web
gpu
managed
```

Unsupported modules should fail clearly during compilation.

---

# 118. Embedded Standard Library

Embedded environments may use:

```text
core
fixed collections
math
atomic operations
hardware abstractions
```

without requiring:

```text
filesystem
networking
processes
heap allocation
```

---

# 119. Fixed-Capacity Collections

Embedded and real-time code should have:

```text
FixedList<T, N>
FixedString<N>
FixedMap<K, V, N>
```

These avoid dynamic allocation.

---

# 120. Real-Time APIs

Real-time-safe APIs should clearly communicate whether they can:

```text
allocate
block
lock
perform I/O
trigger GC
```

Hidden latency must be avoided.

---

# 121. Security Policy

The standard library must prefer secure defaults.

Examples:

```text
TLS certificate verification enabled
cryptographic randomness for security APIs
prepared database queries
bounded parsers
safe path handling
authenticated encryption
```

---

# 122. Unsafe APIs

Dangerous operations should live behind:

```disp
unsafe {
    ...
}
```

or clearly unsafe types.

Safe modules must not casually expose raw pointers.

---

# 123. Deprecation

Unsafe, broken, or obsolete APIs may be deprecated.

Example:

```disp
@deprecated("use secure_api instead")
```

The toolchain should provide migration guidance.

---

# 124. Versioning

Standard-library changes follow DISP language versioning.

Compatibility must distinguish:

```text
source compatibility
binary compatibility
behavioral compatibility
security compatibility
```

---

# 125. Experimental APIs

Unstable APIs must be visibly marked.

Stable applications should not accidentally depend on experimental behavior.

---

# 126. Documentation

Every public standard-library API should document:

```text
purpose
parameters
return values
errors
allocation behavior
thread safety
complexity
security considerations
examples
```

where applicable.

---

# 127. Complexity Documentation

Collections should document algorithmic complexity.

Example:

```text
List.push      amortized O(1)
Map.get        expected O(1)
sorting        O(n log n)
```

---

# 128. Performance Testing

Standard-library components must have benchmarks.

Priority areas:

```text
collections
strings
parsers
networking
async
serialization
database access
tensor operations
Page rendering
```

---

# 129. Security Testing

Priority areas for fuzzing:

```text
text parsing
JSON
URLs
HTTP
TLS boundaries
path handling
database decoding
serialization
compression
Page input
```

---

# 130. No Hidden Global State

Standard-library APIs should avoid hidden mutable process-global state.

Global resources should be explicit or safely encapsulated.

---

# 131. No Hidden Network Activity

Importing or using a standard module must not silently contact external systems.

Network activity happens only when explicitly requested.

---

# 132. No Hidden Telemetry

The standard library must not automatically transmit:

```text
source code
usage information
crash data
identifiers
metrics
```

---

# 133. Dependency Policy

The core standard library should minimize third-party dependencies.

Critical dependencies require:

```text
security review
maintenance review
license review
performance evaluation
```

---

# 134. Native Dependency Isolation

Platform libraries may be used internally where appropriate.

Their unsafe interfaces should be wrapped behind safe DISP abstractions.

---

# 135. Consistency Rule

Equivalent operations should follow equivalent naming.

Examples:

```text
len
is_empty
get
insert
remove
clear
```

should behave consistently across compatible containers.

---

# 136. Error Consistency

Comparable APIs should use comparable error patterns.

The standard library should avoid arbitrary mixtures of:

```text
exceptions
null
error integers
magic values
Result
```

Recoverable failure should normally use:

```text
Result<T, E>
```

---

# 137. Naming Rule

Names should favor clarity over excessive abbreviation.

Preferred:

```text
connection
duration
buffer
iterator
```

Abbreviations should be reserved for universally understood domain terminology.

---

# 138. Simplicity Rule

A beginner should be able to write:

```disp
fn main() {
    let name = input("Name: ")
    print("Hello {name}")
}
```

without learning the architecture of the standard library.

---

# 139. Power Rule

An expert must still be able to control:

```text
allocation
buffers
syscalls
synchronization
device placement
serialization
network behavior
memory layout
```

when necessary.

---

# 140. Pay-for-What-You-Use Rule

Using:

```text
std.math
```

must not automatically include:

```text
HTTP
GPU
database
Page
async
crypto
```

Unused modules should disappear from the final executable.

---

# 141. Zero-Cost Rule

An abstraction may only be called zero-cost when generated behavior demonstrates that claim.

Performance must be measured.

---

# 142. Standard Library Architecture

The initial hierarchy is:

```text
core
│
├── fundamental types
├── traits
├── iterators
└── memory foundations

std
│
├── collections
├── text
├── math
├── I/O
├── filesystem
├── networking
├── HTTP
├── time
├── concurrency
├── process
├── cryptography
├── serialization
├── Data
├── Intelligence
├── GPU
├── Page
├── System
└── testing
```

---

# 143. Implementation Strategy

The first implementation should focus only on:

```text
1. core types
2. strings
3. List
4. Map
5. iterators
6. basic math
7. basic I/O
8. filesystem
9. time
10. testing
```

Then expand into:

```text
networking
async
crypto
data
intelligence
GPU
Page
```

The library must grow with the compiler rather than trying to implement everything immediately.

---

# 144. Standard Library Principle

> One coherent API surface.

> Secure defaults.

> Predictable costs.

> No feature included merely because another language has it.

---

# DISP

**Data. Intelligence. System. Page.**

**One language. One standard library. One coherent platform.**
