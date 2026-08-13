use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source, run_source_with_args,
};
use std::{fs, process::Command};

fn differential_with_args(name: &str, source: &str, arguments: &[String]) {
    let expected = run_source_with_args(source, arguments).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-process-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).args(arguments).output() {
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
fn main_arguments_and_environment_are_differential() {
    let source = r#"
fn main(args: List<String>) {
    print(args.len())
    print(match args.get(0) { Some(value) => (*value).contains("first"), None => false })
    environment_args = Environment.arguments()
    print(match environment_args.get(1) { Some(value) => (*value).contains("second"), None => false })
    print(match Environment.get("PATH") { Some(value) => !value.is_empty(), None => false })
}
"#;
    differential_with_args(
        "arguments",
        source,
        &["first argument".into(), "second".into()],
    );
}

#[test]
fn direct_process_execution_captures_status_bytes_and_text() {
    #[cfg(windows)]
    let (program, arguments) = ("C:/Windows/System32/where.exe", r#"List.of("cmd.exe")"#);
    #[cfg(not(windows))]
    let (program, arguments) = ("/bin/echo", r#"List.of("DISP process")"#);
    let source = format!(
        r#"
fn inspect() -> Result<bool, IoError> {{
    output = Process.run(Path("{program}"), {arguments})?
    text_ok = match output.stdout_text() {{ Ok(text) => true, Err(error) => false }}
    bytes = output.stdout()
    return Ok(output.success() && output.status() == 0 && bytes.len() > 0 && text_ok)
}}
fn main() {{ print(match inspect() {{ Ok(value) => value, Err(error) => false }}) }}
"#
    );
    differential_with_args("capture", &source, &[]);
}

#[test]
fn process_failures_and_invalid_calls_are_typed() {
    let missing = r#"
fn inspect() -> Result<bool, IoError> {
    var args: List<String> = List.new()
    output = Process.run(Path("Z:/definitely/missing/disp-program.exe"), args)?
    return Ok(output.success())
}
fn main() { print(match inspect() { Ok(value) => value, Err(error) => false }) }
"#;
    assert_eq!(run_source(missing).unwrap(), vec!["false"]);
    differential_with_args("missing", missing, &[]);

    let path = check_source("fn main() { Process.run(\"program\", List.new()) }").unwrap_err();
    assert!(path.message.contains("process path") || path.message.contains("Path"));
    assert_eq!((path.span.start.line, path.span.start.column), (1, 25));
    let arguments =
        check_source("fn main() { Process.run(Path(\"program\"), List.of(1)) }").unwrap_err();
    assert!(arguments.message.contains("List<String>"));
    assert_eq!(
        (arguments.span.start.line, arguments.span.start.column),
        (1, 42)
    );
    let environment = check_source("fn main() { Environment.get(1) }").unwrap_err();
    assert!(environment.message.contains("environment variable name"));
    assert_eq!(
        (environment.span.start.line, environment.span.start.column),
        (1, 29)
    );
    let signature = check_source("fn main(args: List<int>) {}").unwrap_err();
    assert!(signature.message.contains("List<String>"));
    assert_eq!(
        (signature.span.start.line, signature.span.start.column),
        (1, 4)
    );
}

#[test]
fn process_output_has_concrete_native_layout_and_abi() {
    let (hir, _) = lower_source("fn main() {}").unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let layout = layouts.layout(&disp::hir::Type::ProcessOutput).unwrap();
    assert_eq!((layout.size, layout.align), (40, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::ProcessOutput, &layout, target),
        abi::PassMode::Indirect
    );
}
