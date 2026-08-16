# DISP thread and synchronization model

Status: implemented Pass 015 working contract.

## Structured OS threads

`spawn function(arguments)` transfers owned, Send-compatible arguments into a new OS thread and
returns `Thread<T>`. `join()` consumes the handle, waits for completion, and returns `T`. Completion
of a thread happens-before a successful join returns. A live handle is joined during deterministic
cleanup, so a DISP thread cannot be silently detached by dropping its handle.

References, borrowed views, raw and checked pointers, callable values, and mutex guards cannot
cross a thread boundary. `Mutex<T>` and `AtomicInt` are non-Copy owned synchronization handles;
sharing them requires an explicit `.share()` operation.

## Mutex ordering

Unlocking a `Mutex<T>` guard performs release synchronization. A later successful `lock()` on the
same mutex performs acquire synchronization and observes writes protected by the earlier guard.
The guard is linear, cannot cross a thread boundary, and unlocks during every structured cleanup
path. Native execution uses the platform mutex primitive; the interpreter implements the same
acquire/release contract.

`Mutex<T>` is recursive for the owning thread. Re-locking the same mutex increments a checked
recursion depth and returns another linear guard; the mutex becomes available to another thread
only after every guard owned by the current thread is released. This contract is identical across
the interpreter, Windows critical sections, and recursive POSIX mutexes.

## AtomicInt ordering

The compatibility methods `load`, `store`, `add`, and `fetch_add` are sequentially consistent.
Ordered spellings make weaker contracts visible in source:

| Operation | Valid methods |
|---|---|
| Load | `load_relaxed`, `load_acquire`, `load_seq_cst` |
| Store | `store_relaxed`, `store_release`, `store_seq_cst` |
| Checked add, returning the new value | `add_relaxed`, `add_acquire`, `add_release`, `add_acq_rel`, `add_seq_cst` |
| Checked fetch-add, returning the old value | `fetch_add_relaxed`, `fetch_add_acquire`, `fetch_add_release`, `fetch_add_acq_rel`, `fetch_add_seq_cst` |

Relaxed operations guarantee atomicity and modification order but create no inter-thread
happens-before edge. A release operation synchronizes with an acquire operation that observes it
or its release sequence. Acquire-release read-modify-write combines both directions. Sequentially
consistent operations additionally participate in one total order.

Invalid combinations such as `load_release`, `load_acq_rel`, or `store_acquire` have no method and
fail during type checking. Checked add uses a compare-exchange loop; overflow reports a controlled
runtime diagnostic and does not modify the atomic value. The interpreter uses Rust atomic
orderings and native execution emits the corresponding C11 `memory_order_*` operation.

## Bounded typed channels

`Channel<T>` is an owned, non-Copy, explicitly shared multi-producer/multi-consumer queue. Capacity
allocation is recoverable and requires a contextual element type because an empty queue contains no
message from which to infer `T`:

```disp
fn worker(jobs: Channel<String>) {
    match jobs.receive() {
        Some(job) => print(job)
        None => print("closed")
    }
}

fn run() -> Result<int, String> {
    var jobs: Channel<String> = Channel.bounded(64)?
    thread = spawn worker(jobs.share())
    jobs.send("compile")
    jobs.close()
    thread.join()
    return Ok(0)
}
```

The capacity must be greater than zero and its allocation must fit the target address space.
`send(value)` consumes `value`, blocks while the queue is full, and returns `true` after enqueueing.
Closure wakes blocked senders; a send that observes closure returns `false` and destroys its
consumed value. `receive()` blocks while an open queue is empty, returns `Some(value)` by moving the
oldest queued message, drains messages that were buffered before closure, and returns `None` only
when the channel is both closed and empty.

`close()` is idempotent and wakes all blocked senders and receivers. `len()`, `capacity()`, and
`is_closed()` inspect synchronized state. A successful send release-synchronizes with the receive
that removes that message. The final channel handle deterministically destroys every message still
queued before releasing its buffer and platform synchronization state. Channel payloads containing
references, borrowed views, guards, tasks, or pointers cannot cross a thread boundary.

## Current boundary

DISP does not infer relaxed orderings, reorder an explicitly selected operation, or make ordinary
unsynchronized shared memory available. A channel must be closed explicitly when other structured
threads could otherwise remain blocked; dropping one shared handle does not revoke the handles
owned by other threads.
