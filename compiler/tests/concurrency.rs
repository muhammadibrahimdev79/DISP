use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-concurrency-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        expected
    );
}

#[test]
fn thread_has_concrete_layout_and_indirect_abi() {
    let (hir, _) = lower_source(
        "fn work(value: int) -> int { return value } fn main() { task = spawn work(1) print(task.join()) }",
    )
    .unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let thread = disp::hir::Type::Thread(Box::new(disp::hir::Type::Int {
        signed: true,
        width: None,
    }));
    let layout = layouts.layout(&thread).unwrap();
    assert_eq!((layout.size, layout.align), (16, 8));
    assert_eq!(
        abi::classify(&thread, &layout, target),
        abi::PassMode::Indirect
    );
}

#[test]
fn spawn_join_owned_values_generics_and_structs_are_differential() {
    let source = r#"
struct Answer { value: int }
fn square(value: int) -> int { return value * value }
fn echo<T>(value: T) -> T { return value }
fn wrap(value: int) -> Answer { return Answer { value: value } }
fn main() {
    first = spawn square(9)
    second = spawn echo("owned")
    third = spawn wrap(7)
    print(first.join())
    print(second.join())
    answer = third.join()
    print(answer.value)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["81", "owned", "7"]);
    differential("owned-generics", source);
}

#[test]
fn unjoined_thread_is_joined_during_deterministic_cleanup() {
    let source = r#"
fn worker() -> int { print("finished") return 1 }
fn main() { task = spawn worker() }
"#;
    assert_eq!(run_source(source).unwrap(), ["finished"]);
    differential("cleanup", source);
}

#[test]
fn spawn_moves_non_copy_arguments_and_join_consumes_handle() {
    let moved = check_source(
        "fn consume_text(value: String) -> uint { return value.len() } fn main() { text = String() task = spawn consume_text(text) print(text) print(task.join()) }",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{}", moved.message);

    let joined = check_source(
        "fn work() -> int { return 1 } fn main() { task = spawn work() print(task.join()) print(task.join()) }",
    )
    .unwrap_err();
    assert!(joined.message.contains("moved"), "{}", joined.message);
}

#[test]
fn thread_boundary_rejects_borrows_raw_pointers_and_non_calls_with_spans() {
    let borrowed = check_source(
        "fn read(value: &int) -> int { return *value } fn main() { value = 1 task = spawn read(&value) print(task.join()) }",
    )
    .unwrap_err();
    assert!(borrowed.message.contains("cannot accept references"));
    assert_eq!(borrowed.span.start.line, 1);
    assert!(borrowed.span.start.column > 60);

    let raw = check_source(
        "fn read(value: ptr<int>) -> int { unsafe { return *value } } fn main() { task = spawn read(0) print(task.join()) }",
    )
    .unwrap_err();
    assert!(
        raw.message.contains("raw pointers") || raw.message.contains("thread"),
        "{}",
        raw.message
    );

    let non_call = check_source("fn main() { task = spawn 1 }").unwrap_err();
    assert!(non_call.message.contains("direct function call"));
    assert_eq!(non_call.span.start.column, 26);
}

#[test]
fn spawn_rejects_intrinsics_and_borrowed_results() {
    let intrinsic = check_source("fn main() { task = spawn print(1) }").unwrap_err();
    assert!(intrinsic.message.contains("DISP function"));

    let result = check_source(
        "fn borrow(value: &int) -> &int { return value } fn main() { value = 1 task = spawn borrow(&value) print(*task.join()) }",
    )
    .unwrap_err();
    assert!(
        result.message.contains("references") || result.message.contains("thread boundary"),
        "{}",
        result.message
    );
}

#[test]
fn mutex_shares_owned_state_and_serializes_mutation_differentially() {
    let source = r#"
fn increment(counter: Mutex<int>, times: int) {
    var index = 0
    while index < times {
        guard = counter.lock()
        *guard += 1
        index += 1
    }
}
fn main() {
    counter = Mutex.new(0)
    first = spawn increment(counter.share(), 300)
    second = spawn increment(counter.share(), 300)
    first.join()
    second.join()
    guard = counter.lock()
    print(*guard)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["600"]);
    differential("mutex-counter", source);
}

#[test]
fn mutex_guard_cannot_cross_thread_boundary_and_mutex_is_non_copy() {
    let guard = check_source(
        "fn take(value: MutexGuard<int>) -> int { return *value } fn main() { mutex = Mutex.new(1) guard = mutex.lock() task = spawn take(guard) print(task.join()) }",
    )
    .unwrap_err();
    assert!(
        guard.message.contains("borrowed views")
            || guard.message.contains("cannot be transferred")
            || guard.message.contains("references"),
        "{}",
        guard.message
    );

    let moved =
        check_source("fn main() { mutex = Mutex.new(1) other = mutex print(mutex) print(other) }")
            .unwrap_err();
    assert!(moved.message.contains("moved"), "{}", moved.message);
}

#[test]
fn mutex_reentrancy_is_recursive_and_differential() {
    let source = r#"
fn main() {
    mutex = Mutex.new(0)
    outer = mutex.lock()
    *outer += 1
    inner = mutex.lock()
    *inner += 1
    print(*outer)
    print(*inner)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["2", "2"]);
    differential("recursive-mutex", source);
}

#[test]
fn atomic_int_is_shared_checked_and_differential() {
    let source = r#"
fn increment(counter: AtomicInt, times: int) {
    var index = 0
    while index < times {
        counter.add(1)
        index += 1
    }
}
fn main() {
    counter = AtomicInt.new(0)
    first = spawn increment(counter.share(), 500)
    second = spawn increment(counter.share(), 500)
    first.join()
    second.join()
    print(counter.load())
    print(counter.fetch_add(2))
    print(counter.load())
    counter.store(7)
    print(counter.load())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["1000", "1000", "1002", "7"]);
    differential("atomic-int", source);
}

#[test]
fn atomic_int_rejects_wrong_values_and_is_non_copy() {
    let wrong = check_source("fn main() { value = AtomicInt.new(true) }").unwrap_err();
    assert!(
        wrong.message.contains("AtomicInt value"),
        "{}",
        wrong.message
    );

    let moved = check_source(
        "fn main() { value = AtomicInt.new(1) other = value print(value.load()) print(other.load()) }",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{}", moved.message);

    let overflow =
        run_source("fn main() { value = AtomicInt.new(9223372036854775807) print(value.add(1)) }")
            .unwrap_err();
    assert!(
        overflow.message.contains("overflow"),
        "{}",
        overflow.message
    );
}

#[test]
fn atomic_memory_orders_are_explicit_validated_and_differential() {
    let source = r#"
fn increment_relaxed(counter: AtomicInt, times: int) {
    var index = 0
    while index < times {
        counter.add_relaxed(1)
        index += 1
    }
}
fn publish(payload: AtomicInt, ready: AtomicInt) {
    payload.store_relaxed(42)
    ready.store_release(1)
}
fn main() {
    counter = AtomicInt.new(0)
    first = spawn increment_relaxed(counter.share(), 500)
    second = spawn increment_relaxed(counter.share(), 500)
    first.join()
    second.join()
    print(counter.load_relaxed())

    payload = AtomicInt.new(0)
    ready = AtomicInt.new(0)
    writer = spawn publish(payload.share(), ready.share())
    writer.join()
    print(ready.load_acquire())
    print(payload.load_relaxed())
    print(payload.fetch_add_acq_rel(3))
    payload.store_seq_cst(9)
    print(payload.load_seq_cst())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["1000", "1", "42", "42", "9"]);
    differential("atomic-orders", source);

    for invalid in [
        "fn main() { value = AtomicInt.new(0) print(value.load_release()) }",
        "fn main() { value = AtomicInt.new(0) value.store_acquire(1) }",
        "fn main() { value = AtomicInt.new(0) value.load_acq_rel() }",
    ] {
        let error = check_source(invalid).unwrap_err();
        assert!(
            error.message.contains("method") || error.message.contains("AtomicInt"),
            "{}",
            error.message
        );
    }
}

#[test]
fn bounded_channels_move_messages_apply_backpressure_and_close() {
    let source = r#"
fn produce(queue: Channel<int>) {
    queue.send(1)
    queue.send(2)
    queue.close()
}
fn exercise() -> Result<int, String> {
    var queue: Channel<int> = Channel.bounded(1)?
    worker = spawn produce(queue.share())
    print(match queue.receive() { Some(value) => value None => -1 })
    print(match queue.receive() { Some(value) => value None => -1 })
    print(match queue.receive() { Some(value) => value None => -1 })
    worker.join()
    print(queue.is_closed())
    print(queue.len())
    print(queue.capacity())
    return Ok(0)
}
fn main() { print(match exercise() { Ok(value) => value Err(_) => -1 }) }
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["1", "2", "-1", "true", "0", "1", "0"]
    );
    differential("bounded-channel", source);

    let invalid = r#"
fn exercise() -> Result<uint, String> {
    var queue: Channel<int> = Channel.bounded(0)?
    return Ok(queue.capacity())
}
fn main() { print(match exercise() { Ok(_) => "bad" Err(error) => error }) }
"#;
    assert_eq!(
        run_source(invalid).unwrap(),
        ["Channel capacity must be greater than zero"]
    );
    differential("invalid-channel-capacity", invalid);

    let moved = check_source(
        "fn exercise()->Result<int,String>{ var queue: Channel<String> = Channel.bounded(1)? text = String() queue.send(text) print(text) return Ok(0) } fn main() {}",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{}", moved.message);
}

#[test]
fn bounded_channels_stress_multiple_producers_and_drain_after_close() {
    let source = r#"
fn produce(queue: Channel<int>, count: int) {
    var index = 0
    while index < count {
        queue.send(1)
        index += 1
    }
}
fn exercise() -> Result<int, String> {
    var queue: Channel<int> = Channel.bounded(3)?
    first = spawn produce(queue.share(), 250)
    second = spawn produce(queue.share(), 250)
    third = spawn produce(queue.share(), 250)
    fourth = spawn produce(queue.share(), 250)
    var total = 0
    var received = 0
    while received < 1000 {
        total += match queue.receive() { Some(value) => value None => 0 }
        received += 1
    }
    first.join()
    second.join()
    third.join()
    fourth.join()
    queue.send(9)
    queue.close()
    print(queue.send(10))
    print(match queue.receive() { Some(value) => value None => -1 })
    print(match queue.receive() { Some(value) => value None => -1 })
    print(total)
    return Ok(0)
}
fn main() { print(match exercise() { Ok(value) => value Err(_) => -1 }) }
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["false", "9", "-1", "1000", "0"]
    );
    differential("channel-mpmc-stress", source);

    let borrowed = check_source(
        "fn use_queue(queue: Channel<&int>){} fn exercise()->Result<int,String>{ value=1 var queue: Channel<&int> = Channel.bounded(1)? queue.send(&value) worker=spawn use_queue(queue.share()) worker.join() return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(
        borrowed.message.contains("references") || borrowed.message.contains("thread"),
        "{}",
        borrowed.message
    );
}

#[test]
fn channel_has_pointer_layout_and_native_owned_queue_cleanup() {
    let (program, _) = lower_source("fn main() {}").unwrap();
    let target = Target::host().unwrap();
    let channel = disp::hir::Type::Channel(Box::new(disp::hir::Type::String));
    let mut layouts = LayoutEngine::new(target, &program);
    let layout = layouts.layout(&channel).unwrap();
    let word = u64::from(target.pointer_width) / 8;
    assert_eq!((layout.size, layout.align), (word, word));
    assert_eq!(
        abi::classify(&channel, &layout, target),
        abi::PassMode::Direct
    );

    let source = r#"
fn exercise() -> Result<int, String> {
    var queue: Channel<String> = Channel.bounded(2)?
    queue.send("owned")
    return Ok(0)
}
fn main() { print(match exercise() { Ok(value) => value Err(_) => -1 }) }
"#;
    let path = std::env::temp_dir().join("disp-channel-owned-cleanup.disp");
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifact.backend_ir.unwrap()).unwrap();
    assert!(generated.contains("disp_channel_send"));
    assert!(generated.contains("disp_channel_receive"));
    assert!(generated.contains("disp_channel_release"));
    assert!(generated.contains("state->head+_drop_i"));
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("native channel cleanup execution failed: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "0\n"
    );
}
