use disp::{
    backend::{self, BuildOptions},
    check_source,
    interpreter::{Interpreter, RuntimeLimits},
    lower_source,
};
use std::{fs, path::PathBuf, process::Command};

fn limits(steps: u64, output: u64, depth: usize) -> RuntimeLimits {
    RuntimeLimits {
        max_steps: steps,
        max_output_bytes: output,
        max_call_depth: depth,
        ..RuntimeLimits::default()
    }
}

fn native(name: &str, source: &str, environment: &[(&str, &str)]) -> std::process::Output {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("resource-limit-tests")
        .join(format!("{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let launch_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let mut blocked = None;
    for (variant, options) in [
        BuildOptions::default(),
        BuildOptions {
            optimized: true,
            ..BuildOptions::default()
        },
    ]
    .into_iter()
    .enumerate()
    {
        let artifact = backend::build(&hir, &mir, &path, options).unwrap();
        for attempt in 0..10 {
            let executable = if attempt == 0 {
                artifact.executable.clone()
            } else {
                let alternate = launch_root.join(format!(
                    "disp-resource-limit-launch-{}-{name}-{variant}-{attempt}.exe",
                    std::process::id()
                ));
                fs::copy(&artifact.executable, &alternate).unwrap();
                alternate
            };
            let mut command = Command::new(&executable);
            for (name, value) in environment {
                command.env(name, value);
            }
            match command.output() {
                Ok(output) => {
                    if attempt > 0 {
                        let _ = fs::remove_file(executable);
                    }
                    return output;
                }
                Err(error) if error.raw_os_error() == Some(4551) => {
                    blocked = Some(error);
                    if attempt > 0 {
                        let _ = fs::remove_file(executable);
                    }
                }
                Err(error) => panic!("workspace native binary failed to run: {error}"),
            }
        }
    }
    panic!(
        "Windows Application Control blocked every workspace path and build profile: {}",
        blocked.unwrap()
    )
}

#[test]
fn interpreter_execution_fuel_stops_unbounded_work() {
    let program = check_source("fn main() { loop {} }").unwrap();
    let error = Interpreter::with_limits(limits(16, 1024, 32))
        .run(&program)
        .expect_err("the execution budget must stop an empty infinite loop");
    assert!(error.message.contains("execution steps"), "{error:?}");
}

#[test]
fn interpreter_output_is_charged_before_it_is_committed() {
    let program = check_source("fn main() { print(\"12345\") }").unwrap();
    let error = Interpreter::with_limits(limits(1024, 5, 32))
        .run(&program)
        .expect_err("five characters plus a newline exceed five bytes");
    assert!(error.message.contains("printed output bytes"), "{error:?}");
}

#[test]
fn interpreter_explicit_memory_uses_the_shared_live_quota() {
    let program = check_source(
        r#"
fn allocate() -> Result<int, String> {
    memory = Memory.allocate(4096, 8)?
    return Ok(int(memory.len()))
}
fn main() {
    print(match allocate() { Ok(value) => value, Err(_) => -1 })
}
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_memory_bytes = 1024;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("explicit memory must be charged before host allocation");
    assert!(error.message.contains("managed memory bytes"), "{error:?}");
}

#[test]
fn explicit_memory_permits_are_released_for_sequential_reuse() {
    let source = r#"
fn allocate() -> Result<int, String> {
    memory = Memory.allocate(1024, 8)?
    return Ok(int(memory.len()))
}
fn main() {
    print(match allocate() { Ok(value) => value, Err(_) => -1 })
    print(match allocate() { Ok(value) => value, Err(_) => -1 })
}
"#;
    let program = check_source(source).unwrap();
    let mut configured = limits(10_000, 1024, 32);
    // The interpreter now meters the live scope/object graph in addition to the
    // explicit 1 KiB allocation. This ceiling fits one call frame but not two
    // leaked explicit allocations.
    configured.max_memory_bytes = 1800;
    assert_eq!(
        Interpreter::with_limits(configured).run(&program).unwrap(),
        ["1024", "1024"]
    );

    let output = native("memory-reuse", source, &[("DISP_MAX_MEMORY_BYTES", "1200")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "1024\n1024\n"
    );
}

#[test]
fn interpreter_call_depth_is_configurable_and_fail_closed() {
    let program =
        check_source("fn recurse() -> int { return recurse() } fn main() { print(recurse()) }")
            .unwrap();
    let error = Interpreter::with_limits(limits(10_000, 1024, 4))
        .run(&program)
        .expect_err("recursive calls must stop at the configured depth");
    assert!(error.message.contains("call depth"), "{error:?}");
}

#[test]
fn interpreter_live_tasks_cannot_multiply_without_bound() {
    let program = check_source(
        r#"
async fn pending() -> int {
    await Async.yield()
    return 1
}
async fn main() {
    first = Async.spawn(pending())
    second = Async.spawn(pending())
    print(await first + await second)
}
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_tasks = 1;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("the second simultaneously live task must be rejected");
    assert!(error.message.contains("live tasks"), "{error:?}");
}

#[test]
fn interpreter_live_threads_cannot_multiply_without_bound() {
    let program = check_source(
        r#"
fn wait() {
    Time.sleep(Duration.from_millis(100))
}
fn main() {
    first = spawn wait()
    second = spawn wait()
    first.join()
    second.join()
}
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_threads = 1;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("the second simultaneously live thread must be rejected");
    assert!(error.message.contains("live threads"), "{error:?}");
}

#[test]
fn interpreter_child_process_launch_attempts_are_bounded() {
    let program = check_source(
        r#"
fn attempt() -> bool {
    var arguments: List<String> = List.new()
    return match Process.run(Path("Z:/definitely/missing/disp-quota.exe"), arguments) {
        Ok(_) => true,
        Err(_) => false,
    }
}
fn main() {
    print(attempt())
    print(attempt())
}
"#,
    )
    .unwrap();
    let mut configured = limits(100_000, 1024, 32);
    configured.max_process_starts = 1;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("the second process launch attempt must be rejected");
    assert!(
        error.message.contains("child-process launch attempts"),
        "{error:?}"
    );
}

#[test]
fn native_execution_fuel_stops_long_running_code() {
    let output = native(
        "steps",
        "fn main() { var total = 0 for i in 0..1000 { total += i } print(total) }",
        &[("DISP_MAX_STEPS", "8")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("execution steps"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_output_is_charged_before_writing() {
    let output = native(
        "output",
        "fn main() { print(\"12345\") }",
        &[("DISP_MAX_OUTPUT_BYTES", "5")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("printed output bytes"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_managed_memory_is_bounded_process_wide() {
    let output = native(
        "memory",
        r#"
fn allocate() -> Result<int, String> {
    memory = Memory.allocate(4096, 8)?
    return Ok(int(memory.len()))
}
fn main() {
    match allocate() {
        Ok(value) => print(value),
        Err(error) => print(error),
    }
}
"#,
        &[("DISP_MAX_MEMORY_BYTES", "1024")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("managed memory bytes"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_call_depth_uses_the_same_fail_closed_contract() {
    let output = native(
        "depth",
        "fn recurse() -> int { return recurse() } fn main() { print(recurse()) }",
        &[("DISP_MAX_CALL_DEPTH", "4")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("call depth"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_live_tasks_share_one_process_quota() {
    let output = native(
        "tasks",
        r#"
async fn pending() -> int {
    await Async.yield()
    return 1
}
async fn main() {
    first = Async.spawn(pending())
    second = Async.spawn(pending())
    print(await first + await second)
}
"#,
        &[("DISP_MAX_TASKS", "1")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("live tasks"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_runtime_threads_share_one_process_quota() {
    let output = native(
        "threads",
        r#"
fn wait() {
    Time.sleep(Duration.from_millis(100))
}
fn main() {
    first = spawn wait()
    second = spawn wait()
    first.join()
    second.join()
}
"#,
        &[("DISP_MAX_THREADS", "1")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("live threads"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_child_process_launch_attempts_share_one_quota() {
    #[cfg(windows)]
    let missing = "Z:/definitely/missing/disp-quota.exe";
    #[cfg(not(windows))]
    let missing = "/definitely/missing/disp-quota";
    let output = native(
        "process-starts",
        &format!(
            r#"
fn attempt() -> bool {{
    var arguments: List<String> = List.new()
    return match Process.run(Path("{missing}"), arguments) {{
        Ok(_) => true,
        Err(_) => false,
    }}
}}
fn main() {{
    print(attempt())
    print(attempt())
}}
"#
        ),
        &[("DISP_MAX_PROCESS_STARTS", "1")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "false\n"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("child-process launch attempts"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn oversized_sync_and_async_file_writes_fail_before_mutation() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("resource-limit-write-preservation.txt");
    fs::write(&path, "safe").unwrap();
    let source = format!(
        r#"
async fn main() {{
    print(match File.write_text(Path("{}"), "blocked") {{ Ok(_) => true, Err(_) => false }})
    print(match await Async.write_text(Path("{}"), "blocked") {{ Ok(_) => true, Err(_) => false }})
}}
"#,
        path.to_string_lossy().replace('\\', "/"),
        path.to_string_lossy().replace('\\', "/")
    );
    let program = check_source(&source).unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_file_write_bytes = 3;
    assert_eq!(
        Interpreter::with_limits(configured).run(&program).unwrap(),
        ["false", "false"]
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "safe");

    let output = native("file-write", &source, &[("DISP_MAX_FILE_WRITE_BYTES", "3")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "false\nfalse\n"
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "safe");
}

#[test]
fn transactional_append_copy_and_failed_commit_preserve_destinations() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("resource-transaction-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("destination.txt");
    let source_path = root.join("source.txt");
    let directory_target = root.join("directory-target");
    fs::create_dir_all(&directory_target).unwrap();
    fs::write(&destination, "safe").unwrap();
    fs::write(&source_path, "toolong").unwrap();
    let source = format!(
        r#"
fn main() {{
    print(match File.append_text(Path("{}"), "xyz") {{ Ok(_) => true, Err(_) => false }})
    print(match File.copy(Path("{}"), Path("{}")) {{ Ok(_) => true, Err(_) => false }})
    print(match File.write_text(Path("{}"), "ok") {{ Ok(_) => true, Err(_) => false }})
}}
"#,
        destination.to_string_lossy().replace('\\', "/"),
        source_path.to_string_lossy().replace('\\', "/"),
        destination.to_string_lossy().replace('\\', "/"),
        directory_target.to_string_lossy().replace('\\', "/"),
    );
    let program = check_source(&source).unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_file_write_bytes = 6;
    assert_eq!(
        Interpreter::with_limits(configured).run(&program).unwrap(),
        ["false", "false", "false"]
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "safe");
    assert!(directory_target.is_dir());
    assert!(!fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".disp-tmp-")
    }));

    let output = native(
        "transaction-preservation",
        &source,
        &[("DISP_MAX_FILE_WRITE_BYTES", "6")],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "false\nfalse\nfalse\n"
    );
    assert_eq!(fs::read_to_string(&destination).unwrap(), "safe");
    assert!(directory_target.is_dir());
    assert!(!fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".disp-tmp-")
    }));
}

#[test]
fn oversized_collection_capacity_fails_before_host_allocation() {
    let source = "fn main() { text = String.with_capacity(100) print(text.len()) }";
    let program = check_source(source).unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_memory_bytes = 64;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("oversized String capacity must not reach the host allocator");
    assert!(error.message.contains("managed memory bytes"), "{error:?}");

    let output = native(
        "collection-capacity",
        source,
        &[("DISP_MAX_MEMORY_BYTES", "64")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("managed memory bytes"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn interpreter_collection_construction_and_growth_obey_memory_ceiling() {
    let cases = [
        "fn main() { values = List.of(1) print(values.len()) }",
        "fn main() { var values: List<int> = List.new() values.push(1) }",
        "fn main() { values = Map.of(1: 2) print(values.len()) }",
        "fn main() { var values: Map<int, int> = Map.new() values.set(1, 2) }",
        "fn main() { values = Set.of(1) print(values.len()) }",
        "fn main() { var values: Set<int> = Set.new() values.add(1) }",
        "fn main() { var text = String.new() text.push_str(\"x\") }",
    ];
    for source in cases {
        let program = check_source(source).unwrap();
        let mut configured = limits(10_000, 1024, 32);
        configured.max_memory_bytes = 0;
        let error = Interpreter::with_limits(configured)
            .run(&program)
            .expect_err("collection growth must honor the configured memory ceiling");
        assert!(
            error.message.contains("managed memory bytes"),
            "source: {source}\nerror: {error:?}"
        );
    }
}

#[test]
fn interpreter_ordinary_object_graph_is_bounded_in_aggregate() {
    let program = check_source(
        r#"
fn main() {
    first = String.with_capacity(1000)
    second = String.with_capacity(1000)
    print(first.len() + second.len())
}
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_memory_bytes = 2200;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("individually valid objects must share one live graph ceiling");
    assert!(error.message.contains("managed memory bytes"), "{error:?}");
    assert_eq!(error.span.start.line, 4);
}

#[test]
fn interpreter_object_graph_bytes_are_released_with_scope_exit() {
    let program = check_source(
        r#"
fn allocate() { value = String.with_capacity(1000) }
fn main() {
    allocate()
    allocate()
    print(true)
}
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_memory_bytes = 1800;
    assert_eq!(
        Interpreter::with_limits(configured).run(&program).unwrap(),
        ["true"]
    );
}

#[test]
fn interpreter_explicit_and_object_memory_share_one_ceiling() {
    let program = check_source(
        r#"
fn main() {
    value = String.with_capacity(1000)
    memory = Memory.allocate(1000, 8)
    print(match memory { Ok(_) => true Err(_) => false })
}
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_memory_bytes = 2200;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("explicit memory must include the live ordinary object graph");
    assert!(error.message.contains("managed memory bytes"), "{error:?}");
    assert_eq!(error.span.start.line, 4);
}

#[test]
fn interpreter_live_handles_are_bounded_and_closed_slots_are_reusable() {
    let exhausted = check_source(
        r#"
fn open() -> Result<int, String> {
    var first: Channel<int> = Channel.bounded(1)?
    var second: Channel<int> = Channel.bounded(1)?
    return Ok(1)
}
fn main() { print(match open() { Ok(_) => true Err(_) => false }) }
"#,
    )
    .unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_handles = 1;
    let error = Interpreter::with_limits(configured)
        .run(&exhausted)
        .expect_err("two simultaneously live channels must exceed one handle slot");
    assert!(error.message.contains("live resource handles"), "{error:?}");

    let reused = check_source(
        r#"
fn open() -> Result<bool, String> {
    var first: Channel<int> = Channel.bounded(1)?
    first.close()
    var second: Channel<int> = Channel.bounded(1)?
    return Ok(second.capacity() == 1)
}
fn main() { print(match open() { Ok(value) => value Err(_) => false }) }
"#,
    )
    .unwrap();
    assert_eq!(
        Interpreter::with_limits(configured).run(&reused).unwrap(),
        ["true"]
    );
}

#[test]
fn native_live_handles_are_bounded_and_closed_slots_are_reusable() {
    let exhausted = r#"
fn open() -> Result<int, String> {
    var first: Channel<int> = Channel.bounded(1)?
    var second: Channel<int> = Channel.bounded(1)?
    return Ok(1)
}
fn main() { print(match open() { Ok(_) => true Err(_) => false }) }
"#;
    let output = native("handles", exhausted, &[("DISP_MAX_HANDLES", "1")]);
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("live resource handles"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let reused = r#"
fn open() -> Result<bool, String> {
    var first: Channel<int> = Channel.bounded(1)?
    first.close()
    var second: Channel<int> = Channel.bounded(1)?
    return Ok(second.capacity() == 1)
}
fn main() { print(match open() { Ok(value) => value Err(_) => false }) }
"#;
    let output = native("handle-reuse", reused, &[("DISP_MAX_HANDLES", "1")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "true\n"
    );
}

#[test]
fn database_handles_share_the_process_wide_ceiling() {
    let source = r#"
fn open() -> Result<int, DataError> {
    var first = Database.memory()?
    var second = Database.memory()?
    return Ok(1)
}
fn main() { print(match open() { Ok(value) => value Err(_) => -1 }) }
"#;
    let program = check_source(source).unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_handles = 1;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("two live database connections must exceed one handle slot");
    assert!(error.message.contains("live resource handles"), "{error:?}");

    let output = native("database-handles", source, &[("DISP_MAX_HANDLES", "1")]);
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("live resource handles"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn socket_handles_share_the_process_wide_ceiling() {
    let source = r#"
fn open() -> Result<int, NetworkError> {
    var first = UdpSocket.bind(SocketAddress("127.0.0.1", 0))?
    var second = UdpSocket.bind(SocketAddress("127.0.0.1", 0))?
    return Ok(1)
}
fn main() { print(match open() { Ok(value) => value Err(_) => -1 }) }
"#;
    let program = check_source(source).unwrap();
    let mut configured = limits(10_000, 1024, 32);
    configured.max_handles = 1;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("two live sockets must exceed one handle slot");
    assert!(error.message.contains("live resource handles"), "{error:?}");

    let output = native("socket-handles", source, &[("DISP_MAX_HANDLES", "1")]);
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("live resource handles"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn live_child_processes_share_the_handle_ceiling() {
    let source = r#"
fn open() -> Result<int, IoError> {
    c0 = Process.command(Path("C:/Windows/System32/ping.exe"))
    c1 = c0.arg("-n")
    c2 = c1.arg("6")
    c3 = c2.arg("127.0.0.1")
    var first = c3.start()?
    d0 = Process.command(Path("C:/Windows/System32/ping.exe"))
    d1 = d0.arg("-n")
    d2 = d1.arg("6")
    d3 = d2.arg("127.0.0.1")
    var second = d3.start()?
    return Ok(1)
}
fn main() { print(match open() { Ok(value) => value Err(_) => -1 }) }
"#;
    let program = check_source(source).unwrap();
    let mut configured = limits(100_000, 1024, 32);
    configured.max_handles = 1;
    let error = Interpreter::with_limits(configured)
        .run(&program)
        .expect_err("two live child processes must exceed one handle slot");
    assert!(error.message.contains("live resource handles"), "{error:?}");

    let output = native("child-handles", source, &[("DISP_MAX_HANDLES", "1")]);
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("live resource handles"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_runtime_rejects_invalid_quota_configuration() {
    let output = native(
        "invalid",
        "fn main() { print(1) }",
        &[("DISP_MAX_STEPS", "not-a-number")],
    );
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("resource configuration error"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
