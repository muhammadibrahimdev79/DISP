use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn unique_root(name: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("disp-pass5-{name}-{nonce}"))
        .to_string_lossy()
        .replace('\\', "/")
}

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-system-{name}.disp"));
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

fn native_failure(name: &str, source: &str) -> Option<String> {
    let path = std::env::temp_dir().join(format!("disp-system-fail-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return None,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(!output.status.success());
    Some(String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn path_time_and_duration_have_concrete_layouts_and_abi() {
    let (hir,_)=lower_source("fn main(){ path=Path(\"x\"); now=Time.now(); delay=Duration.from_millis(1); print(path.len()+delay.millis()+now.elapsed().millis()) }").unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let path = layouts.layout(&disp::hir::Type::Path).unwrap();
    let instant = layouts.layout(&disp::hir::Type::Instant).unwrap();
    let duration = layouts.layout(&disp::hir::Type::Duration).unwrap();
    assert_eq!((path.size, path.align), (24, 8));
    assert_eq!((instant.size, instant.align), (8, 8));
    assert_eq!((duration.size, duration.align), (8, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::Path, &path, target),
        abi::PassMode::Indirect
    );
}

#[test]
fn filesystem_text_metadata_directory_listing_and_cleanup_are_differential() {
    let root = unique_root("io");
    let source = format!(
        r#"
fn work() -> Result<Unit, IoError> {{
    root=Path("{root}")
    Directory.create_all(root)?
    first=root.join("first.txt")
    second=root.join("second.txt")
    binary=root.join("bytes.bin")
    File.write_text(first,"ab")?
    File.append_text(first,"c")?
    File.copy(first,second)?
    print(File.read_text(second)?)
    print(File.size(second)?)
    print(File.modified_seconds(second)? > 0)
    bytes=List.of(u8(0),u8(255),u8(1))
    File.write_bytes(binary,bytes)?
    loaded=File.read_bytes(binary)?
    print(loaded.len())
    print(loaded[1])
    entries=Directory.read(root)?
    print(entries.len())
    File.remove(first)?
    File.remove(second)?
    File.remove(binary)?
    return Directory.remove(root)
}}
fn main() {{ print(work()) }}
"#
    );
    assert_eq!(
        run_source(&source).unwrap(),
        ["abc", "3", "true", "3", "255", "3", "Result.Ok(())"]
    );
    differential("filesystem", &source);
}

#[test]
fn path_operations_and_monotonic_time_are_differential() {
    let root = unique_root("path");
    let source = format!(
        r#"
fn main() {{
    root=Path("{root}")
    child=root.join("child.txt")
    print(root.is_absolute())
    print(child.as_string().contains("child.txt"))
    print(child.name())
    print(child.extension())
    print(child.parent())
    print(child.len() > root.len())
    started=Time.now()
    Time.sleep(Duration.from_millis(2))
    print(started.elapsed().millis() >= 1)
    print(Duration.from_seconds(2).millis())
    print(Time.unix_seconds() > 0)
}}
"#
    );
    let output = run_source(&source).unwrap();
    assert_eq!(
        output[0..4],
        ["true", "true", "Option.Some(child.txt)", "Option.Some(txt)"]
    );
    assert!(output[4].starts_with("Option.Some("));
    assert_eq!(output[5..], ["true", "true", "2000", "true"]);
    differential("path-time", &source);
}

#[test]
fn capability_timer_ticks_advance_in_fixed_ten_millisecond_units() {
    let source = r#"
fn main() uses Timer {
    first = Time.ticks()
    Time.sleep(Duration.from_millis(30))
    second = Time.ticks()
    print(second != first)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["true"]);
    differential("capability-timer", source);
}

#[test]
fn io_errors_propagate_without_panics_and_invalid_types_have_spans() {
    let missing = unique_root("missing");
    let source = format!(
        "fn read()->Result<String,IoError>{{ return File.read_text(Path(\"{missing}\")) }} fn main(){{ print(match read(){{ Ok(value)=>true, Err(error)=>false }}) }}"
    );
    assert_eq!(run_source(&source).unwrap(), ["false"]);
    differential("missing", &source);

    let wrong_path = check_source("fn main(){ print(File.exists(1)) }").unwrap_err();
    assert!(wrong_path.message.contains("filesystem path"));
    assert_eq!(wrong_path.span.start.line, 1);

    let wrong_text =
        check_source("fn main(){ path=Path(\"x\"); print(File.write_text(path,1)) }").unwrap_err();
    assert!(wrong_text.message.contains("file text"));

    let wrong_sleep = check_source("fn main(){ Time.sleep(1) }").unwrap_err();
    assert!(wrong_sleep.message.contains("sleep duration"));

    let nul = run_source("fn main(){ path=Path(\"a\0b\"); print(path) }").unwrap_err();
    assert!(nul.message.contains("NUL"));
    assert_eq!(nul.span.start.line, 1);
    if let Some(stderr) = native_failure("nul", "fn main(){ path=Path(\"a\0b\"); print(path) }") {
        assert!(stderr.contains("NUL"));
        assert!(stderr.contains("1:"));
    }

    let overflow = "fn main(){ print(Duration.from_seconds(u64(18446744073709551615))) }";
    assert!(
        run_source(overflow)
            .unwrap_err()
            .message
            .contains("overflow")
    );
    if let Some(stderr) = native_failure("duration-overflow", overflow) {
        assert!(stderr.contains("Duration overflow"));
    }

    let negative = "fn main(){ print(Duration.from_millis(-1)) }";
    assert!(
        run_source(negative)
            .unwrap_err()
            .message
            .contains("non-negative")
    );
    if let Some(stderr) = native_failure("duration-negative", negative) {
        assert!(stderr.contains("cannot be negative"));
    }

    let nominal =
        check_source("fn take(value:String){} fn main(){ take(Path(\"x\")) }").unwrap_err();
    assert!(nominal.message.contains("argument"));
}
