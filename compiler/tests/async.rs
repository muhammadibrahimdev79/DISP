use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-async-{name}.disp"));
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
fn async_calls_are_lazy_owned_futures_and_await_is_differential() {
    let source = r#"
async fn double(value: int) -> int {
    print("double")
    return value * 2
}

async fn answer() -> int {
    first = await double(20)
    return first + 2
}

async fn main() {
    future = answer()
    print("created")
    print(await future)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["created", "double", "42"]);
    differential("nested", source);
}

#[test]
fn future_has_concrete_owned_native_layout() {
    let (hir, mir) =
        lower_source("async fn work() -> int { return 1 } async fn main() { print(await work()) }")
            .unwrap();
    let ty = disp::hir::Type::Future(Box::new(disp::hir::Type::Int {
        signed: true,
        width: None,
    }));
    let target = Target::host().unwrap();
    let layout = LayoutEngine::new(target, &hir).layout(&ty).unwrap();
    assert_eq!((layout.size, layout.align), (24, 8));
    assert_eq!(abi::classify(&ty, &layout, target), abi::PassMode::Indirect);
    let narrow_target = Target {
        pointer_width: 32,
        pointer_alignment: 4,
        ..target
    };
    let narrow = LayoutEngine::new(narrow_target, &hir).layout(&ty).unwrap();
    assert_eq!((narrow.size, narrow.align), (12, 4));
    assert!(hir.functions.iter().any(|function| function.asynchronous));
    assert!(mir.functions.iter().any(|function| function.asynchronous));
    assert!(
        mir.functions
            .iter()
            .flat_map(|function| &function.blocks)
            .any(|block| matches!(block.terminator, disp::mir::Terminator::Await { .. }))
    );
}

#[test]
fn await_diagnostics_are_source_spanned() {
    let outside =
        check_source("async fn value() -> int { return 1 } fn main() { print(await value()) }")
            .unwrap_err();
    assert!(
        outside
            .message
            .contains("only allowed inside an `async fn`")
    );
    assert_eq!(outside.span.start.line, 1);
    assert!(outside.span.start.column > 45);

    let non_future = check_source("async fn main() { print(await 1) }").unwrap_err();
    assert!(non_future.message.contains("requires Future"));

    let borrowed = check_source("async fn read(value: &int) -> int { return *value } fn main() {}")
        .unwrap_err();
    assert!(borrowed.message.contains("cannot capture a borrowed"));
}

#[test]
fn future_is_linear_and_spawn_does_not_duplicate_async_work() {
    let reused = check_source(
        "async fn value() -> int { return 1 } async fn main() { future = value() print(await future) print(await future) }",
    )
    .unwrap_err();
    assert!(reused.message.contains("moved"), "{}", reused.message);

    let spawned = check_source(
        "async fn value() -> int { return 1 } fn main() { task = spawn value() print(task.join()) }",
    )
    .unwrap_err();
    assert!(spawned.message.contains("synchronous function"));
}

#[test]
fn generic_async_and_async_function_values_are_differential() {
    let source = r#"
async fn identity<T>(value: T) -> T { return value }
async fn increment(value: int) -> int { return value + 1 }

async fn main() {
    let operation: fn(int) -> Future<int> = increment
    print(await identity("generic"))
    print(await operation(41))
}
"#;
    differential("generic-callable", source);
}

#[test]
fn dropping_an_unpolled_future_cancels_owned_work() {
    let source = r#"
async fn deferred(text: String) -> uint {
    print("unexpected")
    return text.len()
}

fn main() {
    text = "owned by future"
    future = deferred(text)
    print("cancelled")
}
"#;
    assert_eq!(run_source(source).unwrap(), ["cancelled"]);
    differential("cancel", source);
}

#[test]
fn suspension_resumes_without_repeating_prior_side_effects() {
    let source = r#"
async fn suspended() -> int {
    print("before")
    await Async.yield()
    print("after")
    return 42
}

async fn main() {
    print(await suspended())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["before", "after", "42"]);
    differential("suspend-once", source);
}

#[test]
fn loops_recursion_and_owned_state_survive_suspension() {
    let source = r#"
async fn factorial(value: int) -> int {
    if value <= 1 { return 1 }
    await Async.yield()
    return value * await factorial(value - 1)
}

async fn build() -> String {
    text = String()
    var index = 0
    while index < 3 {
        text.push_str("x")
        await Async.yield()
        index += 1
    }
    return text
}

async fn main() {
    print(await factorial(6))
    print(await build())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["720", "xxx"]);
    differential("state-control-flow", source);
}

#[test]
fn owned_async_trait_methods_are_differential() {
    let source = r#"
trait Compute {
    async fn compute(self) -> int
}

struct Number { value: int }

impl Compute for Number {
    async fn compute(self) -> int {
        await Async.yield()
        return self.value
    }
}

async fn evaluate<T: Compute>(value: T) -> int {
    return await value.compute()
}

async fn main() {
    print(await evaluate(Number { value: 42 }))
}
"#;
    differential("trait-method", source);
}
