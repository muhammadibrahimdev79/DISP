use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-tasks-{name}.disp"));
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
fn cooperative_tasks_are_spawned_scheduled_and_awaited_differentially() {
    let source = r#"
async fn work(value: int) -> int {
    print(value)
    await Async.yield()
    return value * 10
}

async fn main() {
    left = Async.spawn(work(2))
    right = Async.spawn(work(4))
    await Async.yield()
    print("parent")
    print(await left + await right)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["2", "4", "parent", "60"]);
    differential("cooperative", source);
}

#[test]
fn task_has_pointer_layout_direct_abi_and_mir_await() {
    let (hir, mir) = lower_source(
        "async fn work() -> int { return 1 } async fn main() { let task: Task<int> = Async.spawn(work()) print(await task) }",
    )
    .unwrap();
    let ty = disp::hir::Type::Task(Box::new(disp::hir::Type::Int {
        signed: true,
        width: None,
    }));
    let target = Target::host().unwrap();
    let layout = LayoutEngine::new(target, &hir).layout(&ty).unwrap();
    assert_eq!((layout.size, layout.align), (8, 8));
    assert_eq!(abi::classify(&ty, &layout, target), abi::PassMode::Direct);
    assert!(mir.functions.iter().flat_map(|f| &f.blocks).any(|block| {
        matches!(
            &block.terminator,
            disp::mir::Terminator::Await { future, .. }
                if matches!(future, disp::mir::Operand::Move(_))
        )
    }));
}

#[test]
fn generic_nested_tasks_and_owned_results_are_differential() {
    let source = r#"
async fn identity<T>(value: T) -> T {
    await Async.yield()
    return value
}

async fn nested(value: String) -> String {
    task = Async.spawn(identity(value))
    return await task
}

async fn main() {
    task = Async.spawn(nested("owned result"))
    print(await task)
}
"#;
    differential("generic-owned", source);
}

#[test]
fn dropping_a_task_cancels_unstarted_owned_work() {
    let source = r#"
async fn deferred(text: String) -> String {
    print("unexpected")
    return text
}

async fn main() {
    task = Async.spawn(deferred("owned by cancelled task"))
    print("cancelled")
}
"#;
    assert_eq!(run_source(source).unwrap(), ["cancelled"]);
    differential("cancel", source);
}

#[test]
fn spawn_and_task_diagnostics_are_source_spanned() {
    let outside = check_source(
        "async fn work() -> int { return 1 }\nfn main() { task = Async.spawn(work()) }",
    )
    .unwrap_err();
    assert!(
        outside
            .message
            .contains("only allowed inside an `async fn`")
    );
    assert_eq!(outside.span.start.line, 2);

    let wrong = check_source("async fn main() { task = Async.spawn(42) }").unwrap_err();
    assert!(wrong.message.contains("requires Future"));
    assert_eq!(wrong.span.start.line, 1);

    let reused = check_source(
        "async fn work() -> int { return 1 } async fn main() { task = Async.spawn(work()) print(await task) print(await task) }",
    )
    .unwrap_err();
    assert!(reused.message.contains("moved"), "{}", reused.message);
}

#[test]
fn task_handles_cannot_escape_their_structured_async_scope() {
    let result = check_source(
        "async fn work() -> int { return 1 } async fn escape() -> Task<int> { return Async.spawn(work()) } fn main() {}",
    )
    .unwrap_err();
    assert!(
        result.message.contains("cannot escape"),
        "{}",
        result.message
    );

    let field = check_source("struct Holder { task: Task<int> } fn main() {}").unwrap_err();
    assert!(field.message.contains("cannot be stored in a struct"));

    let payload =
        check_source("enum Holder { Running(Task<int>), Empty } fn main() {}").unwrap_err();
    assert!(payload.message.contains("cannot be stored in an enum"));
}

#[test]
fn native_c_contains_real_scheduler_and_result_cleanup() {
    let source = r#"
async fn work() -> String { await Async.yield() return "done" }
async fn main() { task = Async.spawn(work()) print(await task) }
"#;
    let path = std::env::temp_dir().join("disp-task-runtime.disp");
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
    assert!(generated.contains("disp_executor_tick"));
    assert!(generated.contains("disp_task_spawn"));
    assert!(generated.contains("disp_task_poll"));
    assert!(generated.contains("disp_task_result_drop_s"));
    differential("owned-cleanup", source);
}
