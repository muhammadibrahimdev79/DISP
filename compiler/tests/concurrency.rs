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
