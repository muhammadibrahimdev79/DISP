use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-async-io-{label}-{}-{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

fn source_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

fn native(name: &str, source: &str, emit_c: bool) -> Option<(String, Option<String>)> {
    let path = unique_path(&format!("{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = artifacts
        .backend_ir
        .map(|path| fs::read_to_string(path).unwrap());
    for _ in 0..4 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return Some((
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .replace("\r\n", "\n"),
                    generated,
                ));
            }
            Err(error) if error.raw_os_error() == Some(4551) => {}
            Err(error) => panic!("native execution failed: {error}"),
        }
    }
    None
}

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let Some((actual, _)) = native(name, source, false) else {
        return;
    };
    assert_eq!(actual, expected);
}

#[test]
fn async_sleep_and_cooperative_timer_tasks_are_differential() {
    let source = r#"
async fn delayed(value: int, delay: Duration) -> int {
    await Async.sleep(delay)
    return value
}

async fn main() {
    slow = Async.spawn(delayed(1, Duration.from_millis(8)))
    fast = Async.spawn(delayed(2, Duration.from_millis(1)))
    print(await fast)
    print(await slow)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["2", "1"]);
    differential("timers", source);
}

#[test]
fn async_text_and_byte_files_are_owned_and_differential() {
    let text = unique_path("text.txt");
    let binary = unique_path("bytes.bin");
    let source = format!(
        r#"
async fn work() -> Result<Unit, IoError> {{
    write = await Async.write_text(Path("{}"), "native async")
    write?
    read = await Async.read_text(Path("{}"))
    print(read?)

    values = List.of(u8(0), u8(255), u8(9))
    write_bytes = await Async.write_bytes(Path("{}"), values)
    write_bytes?
    read_bytes = await Async.read_bytes(Path("{}"))
    loaded = read_bytes?
    print(loaded.len())
    print(loaded[1])

    File.remove(Path("{}"))?
    return File.remove(Path("{}"))
}}

async fn main() {{ print(await work()) }}
"#,
        source_path(&text),
        source_path(&text),
        source_path(&binary),
        source_path(&binary),
        source_path(&text),
        source_path(&binary),
    );
    assert_eq!(
        run_source(&source).unwrap(),
        ["native async", "3", "255", "Result.Ok(())"]
    );
    differential("files", &source);
    assert!(!text.exists());
    assert!(!binary.exists());
}

#[test]
fn unpolled_async_io_is_lazy_and_cancels_without_side_effects() {
    let path = unique_path("cancelled.txt");
    let source = format!(
        r#"
async fn main() {{
    future = Async.write_text(Path("{}"), "must not be written")
    print(File.exists(Path("{}")))
}}
"#,
        source_path(&path),
        source_path(&path)
    );
    assert_eq!(run_source(&source).unwrap(), ["false"]);
    differential("lazy-cancel", &source);
    assert!(!path.exists());
}

#[test]
fn cancellation_drains_started_io_without_undoing_completed_side_effects() {
    let path = unique_path("started-cancel.txt");
    let source = format!(
        r#"
async fn background(path: Path) {{
    result = await Async.write_text(path, "started")
}}

async fn main() {{
    if true {{
        task = Async.spawn(background(Path("{}")))
        await Async.yield()
    }}
    print("cancelled")
}}
"#,
        source_path(&path),
    );
    assert_eq!(run_source(&source).unwrap(), ["cancelled"]);
    assert_eq!(fs::read_to_string(&path).unwrap(), "started");
    fs::remove_file(&path).unwrap();

    let Some((actual, _)) = native("started-cancel", &source, false) else {
        return;
    };
    assert_eq!(actual, "cancelled\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), "started");
    fs::remove_file(&path).unwrap();
}

#[test]
fn async_io_errors_remain_typed_results() {
    let missing = unique_path("missing.txt");
    let source = format!(
        r#"
async fn main() {{
    result = await Async.read_text(Path("{}"))
    print(match result {{ Ok(value) => true, Err(error) => false }})
}}
"#,
        source_path(&missing)
    );
    assert_eq!(run_source(&source).unwrap(), ["false"]);
    differential("error", &source);
}

#[test]
fn async_text_rejects_invalid_utf8_while_byte_reads_preserve_it() {
    let path = unique_path("invalid-utf8.bin");
    let source = format!(
        r#"
async fn work() -> Result<Unit, IoError> {{
    write = await Async.write_bytes(Path("{}"), List.of(u8(255)))
    write?
    text = await Async.read_text(Path("{}"))
    print(match text {{ Ok(value) => true, Err(error) => false }})
    bytes = await Async.read_bytes(Path("{}"))
    loaded = bytes?
    print(loaded[0])
    return File.remove(Path("{}"))
}}
async fn main() {{ print(await work()) }}
"#,
        source_path(&path),
        source_path(&path),
        source_path(&path),
        source_path(&path),
    );
    assert_eq!(
        run_source(&source).unwrap(),
        ["false", "255", "Result.Ok(())"]
    );
    differential("invalid-utf8", &source);
    assert!(!path.exists());
}

#[test]
fn async_io_owns_paths_and_write_buffers() {
    let moved_path = check_source(
        r#"async fn main() {
path = Path("x")
future = Async.read_text(path)
print(path.len())
}"#,
    )
    .unwrap_err();
    assert!(
        moved_path.message.contains("moved"),
        "{}",
        moved_path.message
    );
    assert_eq!(moved_path.span.start.line, 4);

    let moved_text = check_source(
        r#"async fn main() {
text = String()
future = Async.write_text(Path("x"), text)
print(text.len())
}"#,
    )
    .unwrap_err();
    assert!(
        moved_text.message.contains("moved"),
        "{}",
        moved_text.message
    );

    let moved_bytes = check_source(
        r#"async fn main() {
values = List.of(u8(1))
future = Async.write_bytes(Path("x"), values)
print(values.len())
}"#,
    )
    .unwrap_err();
    assert!(
        moved_bytes.message.contains("moved"),
        "{}",
        moved_bytes.message
    );
}

#[test]
fn async_io_type_and_arity_errors_have_source_spans() {
    let duration = check_source("async fn main() { future = Async.sleep(1) }").unwrap_err();
    assert!(duration.message.contains("async sleep duration"));

    let path = check_source("async fn main() { future = Async.read_text(1) }").unwrap_err();
    assert!(path.message.contains("filesystem path must be Path"));

    let borrowed = check_source(
        "async fn main() { text = String() let view: &str = &text future = Async.write_text(Path(\"x\"), view) }",
    )
    .unwrap_err();
    assert!(
        borrowed.message.contains("owned String"),
        "{}",
        borrowed.message
    );

    let bytes =
        check_source("async fn main() { future = Async.write_bytes(Path(\"x\"), List.of(1)) }")
            .unwrap_err();
    assert!(bytes.message.contains("owned List<u8>"));

    let arity = check_source("async fn main() { future = Async.read_text() }").unwrap_err();
    assert!(arity.message.contains("no async operation"));
    assert_eq!(arity.span.start.line, 1);
}

#[test]
fn async_io_reaches_hir_mir_and_native_reactor_lowering() {
    let source = r#"
async fn main() {
    timer = Async.sleep(Duration.from_millis(1))
    read = Async.read_text(Path("missing"))
    await timer
    print(await read)
}
"#;
    let (hir, mir) = lower_source(source).unwrap();
    assert!(disp::hir::dump(&hir).contains("Async.read_text"));
    assert!(mir.functions.iter().flat_map(|f| &f.blocks).any(|block| {
        matches!(
            &block.terminator,
            disp::mir::Terminator::Call {
                target: disp::hir::CallTarget::Intrinsic(name),
                ..
            } if name == "Async.read_text"
        )
    }));
    assert!(
        mir.functions
            .iter()
            .flat_map(|f| &f.blocks)
            .any(|block| { matches!(block.terminator, disp::mir::Terminator::Await { .. }) })
    );
    let Some((_, generated)) = native("reactor-lowering", source, true) else {
        return;
    };
    let generated = generated.unwrap();
    assert!(generated.contains("disp_reactor_wait"));
    assert!(generated.contains("disp_future_sleep"));
    assert!(generated.contains("disp_async_file_worker"));
    assert!(generated.contains("disp_thread_detach"));
    assert!(generated.contains("Async_read_text_poll"));
}
