# Structured async, cancellation, deadlines, and cleanup

Status: implemented and conformance-backed Pass 016 behavior.

## Ownership and lifecycle

Calling an `async fn` constructs a lazy, linear `Future<T>`. Construction owns all moved
arguments but performs no function-body or I/O side effect. `await` consumes the future exactly
once. Dropping an unpolled future cancels it by destroying its owned state.

`Async.spawn(future)` moves a `Future<T>` into the current async scope and returns a linear
`Task<T>`. Tasks cannot be returned, stored in aggregates, duplicated, or transferred outside
that scope. Every task therefore reaches exactly one terminal path before its scope is destroyed:

- `await task` consumes the handle and moves out the completed result;
- `task.cancel()` consumes the handle, cancels pending work, and completes owned-state cleanup
  before returning;
- implicit scope cleanup has the same cancellation obligation as `cancel()`;
- `task.is_finished()` borrows the handle and reports whether a result is ready without consuming
  that result.

Cancellation is cooperative between scheduler polls. A cancellation request prevents later polls;
the future drop function releases operation state and any owned inputs exactly once. Cancellation
does not roll back external side effects that completed before the cancellation boundary.

For a native operation that the operating system has already started and cannot safely cancel,
cleanup-before-return means the task and future release their ownership and discard the result; it
does not mean blocking the cooperative executor until the system call finishes. The runtime keeps
only the worker-owned state needed to finish that operation and drains all such workers before
process exit.

## Deadlines

Timeout-bearing operations accept `Duration`. Their deadline starts on the first poll, preserving
future laziness: time spent holding an unpolled future does not consume its timeout. A zero duration
fails deterministically before application I/O. Deadline expiry is a typed domain error rather than
a panic.

Current timeout-bearing domains include DNS, TCP connect/accept/read/write, UDP receive/send, TLS
handshake/read/write, HTTP requests, and child processes. Closing an associated socket or listener
wakes or terminates blocked native operations; operation drop releases reactor and platform state.

## Backpressure and cleanup

`Channel<T>` is the bounded async/thread handoff primitive currently available. Its finite capacity
applies producer backpressure, closure wakes blocked operations, buffered messages drain in FIFO
order, and final channel cleanup destroys every queued message exactly once.

Async file inputs, HTTP bodies, socket write buffers, process input, and task results are owned by
their future or task until completion/cancellation. Generated native futures always carry a drop
function; `Future<T>` and `Task<T>` are non-Copy so MIR drop flags select one cleanup path.

## Pass 016 hardening matrix

The active pass is extending evidence across these state transitions:

| State transition | Required evidence |
|---|---|
| constructed -> dropped | no side effect; all owned inputs released |
| pending -> explicitly cancelled | no later poll; cleanup completes before `cancel()` returns |
| pending -> scope exit | same observable cleanup contract as explicit cancellation |
| pending -> deadline expired | typed error, bounded wakeup, no repeated side effect |
| pending -> resource closed | blocked operation wakes and releases its registration |
| ready -> inspected -> awaited | `is_finished()` preserves the result for one consuming await |
| ready -> cancelled/dropped | unclaimed result destroyed exactly once |
| producer saturation | bounded memory and explicit backpressure |
| injected I/O/platform failure | typed failure and complete partial-state cleanup |

Interpreter and native execution cover this matrix through task-tree cancellation, lazy and
started file operations, started TCP/UDP cancellation, resource closure, zero and expiring
deadlines, malformed HTTP/TLS/network inputs, and public conformance cases. Global allocator
exhaustion and process-wide resource quotas belong to Pass 017. Multi-threaded work stealing and
distributed executors remain outside the current implemented core.
