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
    let command = layouts.layout(&disp::hir::Type::ProcessCommand).unwrap();
    let layout = layouts.layout(&disp::hir::Type::ProcessOutput).unwrap();
    assert_eq!((command.size, command.align), (160, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::ProcessCommand, &command, target),
        abi::PassMode::Indirect
    );
    assert_eq!((layout.size, layout.align), (40, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::ProcessOutput, &layout, target),
        abi::PassMode::Indirect
    );
}

#[test]
fn configured_commands_are_linear_bounded_and_differential() {
    #[cfg(windows)]
    let source = r#"
fn configured() -> Result<bool, IoError> {
    c0 = Process.command(Path("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"))
    c1 = c0.arg("-NoProfile")
    c2 = c1.arg("-NonInteractive")
    c3 = c2.arg("-Command")
    c4 = c3.arg("$data=[Console]::In.ReadToEnd(); [Console]::Out.Write($env:DISP_MODE + ':' + $data + ':' + (Get-Location).Path)")
    c5 = c4.clear_environment()
    c6 = c5.environment("DISP_MODE", "old")
    c7 = c6.environment("DISP_MODE", "safe")
    c8 = c7.directory(Path("C:/Windows"))
    c9 = c8.input_text("hello\n")
    c10 = c9.timeout(Duration.from_seconds(3))
    output = c10.run()?
    text_ok = match output.stdout_text() { Ok(text) => text.contains("safe:hello") && text.contains("Windows"), Err(error) => false }
    return Ok(output.success() && text_ok)
}
fn main() { print(match configured() { Ok(value) => value, Err(error) => false }) }
"#;
    #[cfg(not(windows))]
    let source = r#"
fn configured() -> Result<bool, IoError> {
    c0 = Process.command(Path("/bin/sh"))
    c1 = c0.arg("-c")
    c2 = c1.arg("read data; printf '%s:%s:%s' \"$DISP_MODE\" \"$data\" \"$PWD\"")
    c3 = c2.clear_environment()
    c4 = c3.environment("DISP_MODE", "old")
    c5 = c4.environment("DISP_MODE", "safe")
    c6 = c5.directory(Path("/"))
    c7 = c6.input_text("hello\n")
    c8 = c7.timeout(Duration.from_seconds(3))
    output = c8.run()?
    text_ok = match output.stdout_text() { Ok(text) => text.contains("safe:hello:/"), Err(error) => false }
    return Ok(output.success() && text_ok)
}
fn main() { print(match configured() { Ok(value) => value, Err(error) => false }) }
"#;
    differential_with_args("configured", source, &[]);

    #[cfg(windows)]
    let timeout = r#"
fn timed_out() -> bool {
    c0 = Process.command(Path("C:/Windows/System32/ping.exe"))
    c1 = c0.arg("-n")
    c2 = c1.arg("6")
    c3 = c2.arg("127.0.0.1")
    c4 = c3.timeout(Duration.from_millis(1))
    result = c4.run()
    return match result { Ok(output) => false, Err(error) => true }
}
fn main() { print(timed_out()) }
"#;
    #[cfg(not(windows))]
    let timeout = r#"
fn timed_out() -> bool {
    c0 = Process.command(Path("/bin/sleep"))
    c1 = c0.arg("5")
    c2 = c1.timeout(Duration.from_millis(1))
    result = c2.run()
    return match result { Ok(output) => false, Err(error) => true }
}
fn main() { print(timed_out()) }
"#;
    differential_with_args("timeout", timeout, &[]);
}

#[test]
fn command_argument_lists_and_binary_input_are_differential() {
    #[cfg(windows)]
    let source = r#"
fn inspect() -> Result<bool, IoError> {
    args = List.of("-NoProfile", "-NonInteractive", "-Command", "[Console]::Out.Write([Console]::In.ReadToEnd())")
    bytes = List.of(u8(65), u8(66), u8(67))
    c0 = Process.command(Path("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"))
    c1 = c0.arguments(args)
    c2 = c1.input(bytes)
    output = c2.run()?
    text_ok = match output.stdout_text() { Ok(text) => text == "ABC", Err(error) => false }
    return Ok(output.success() && text_ok)
}
fn main() { print(match inspect() { Ok(value) => value, Err(error) => false }) }
"#;
    #[cfg(not(windows))]
    let source = r#"
fn inspect() -> Result<bool, IoError> {
    var args: List<String> = List.new()
    bytes = List.of(u8(65), u8(66), u8(67))
    c0 = Process.command(Path("/bin/cat"))
    c1 = c0.arguments(args)
    c2 = c1.input(bytes)
    output = c2.run()?
    text_ok = match output.stdout_text() { Ok(text) => text == "ABC", Err(error) => false }
    return Ok(output.success() && text_ok)
}
fn main() { print(match inspect() { Ok(value) => value, Err(error) => false }) }
"#;
    differential_with_args("binary-input", source, &[]);
}

#[test]
fn process_command_configuration_has_exact_type_diagnostics() {
    let cases = [
        ("fn main(){ Process.command(1) }", "path must be Path"),
        (
            "fn main(){ command=Process.command(Path(\"x\")); command.arg(1) }",
            "process argument",
        ),
        (
            "fn main(){ command=Process.command(Path(\"x\")); command.environment(1,\"x\") }",
            "environment",
        ),
        (
            "fn main(){ command=Process.command(Path(\"x\")); command.input(List.of(1)) }",
            "List<u8>",
        ),
        (
            "fn main(){ command=Process.command(Path(\"x\")); command.timeout(1) }",
            "process timeout",
        ),
    ];
    for (source, message) in cases {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, 1);
        assert!(error.span.start.column > 1);
    }

    let moved = check_source(
        "fn main(){ command=Process.command(Path(\"x\")); configured=command.arg(\"a\"); command.run() }",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"));

    let moved_argument = check_source(
        r#"fn main(){
            var argument=String.new()
            argument.push_str("owned")
            command=Process.command(Path("x"))
            configured=command.arg(argument)
            print(argument)
        }"#,
    )
    .unwrap_err();
    assert!(moved_argument.message.contains("moved"));

    let moved_in_fluent_chain = check_source(
        r#"fn main(){
            var argument=String.new()
            argument.push_str("owned")
            result=Process.command(Path("x")).arg(argument).run()
            print(argument)
        }"#,
    )
    .unwrap_err();
    assert!(moved_in_fluent_chain.message.contains("moved"));

    let moved_path = check_source(
        r#"fn main(){
            path=Path("x")
            command=Process.command(path)
            print(path)
        }"#,
    )
    .unwrap_err();
    assert!(moved_path.message.contains("moved"));
}

#[test]
fn process_environment_validation_is_typed_and_differential() {
    #[cfg(windows)]
    let source = r#"
fn invalid(name: String) -> bool {
    c0 = Process.command(Path("C:/Windows/System32/where.exe"))
    c1 = c0.arg("cmd.exe")
    c2 = c1.environment(name, "value")
    return match c2.run() { Ok(output) => false, Err(error) => true }
}
fn empty_value() -> Result<bool, IoError> {
    c0 = Process.command(Path("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"))
    c1 = c0.arg("-NoProfile")
    c2 = c1.arg("-NonInteractive")
    c3 = c2.arg("-Command")
    c4 = c3.arg("[Console]::Out.Write($env:DISP_EMPTY.Length)")
    c5 = c4.environment("DISP_EMPTY", "")
    output = c5.run()?
    return Ok(match output.stdout_text() { Ok(text) => text == "0", Err(error) => false })
}
fn main() {
    print(invalid(""))
    print(invalid("BAD=NAME"))
    print(match empty_value() { Ok(value) => value, Err(error) => false })
}
"#;
    #[cfg(not(windows))]
    let source = r#"
fn invalid(name: String) -> bool {
    c0 = Process.command(Path("/bin/true"))
    c1 = c0.environment(name, "value")
    return match c1.run() { Ok(output) => false, Err(error) => true }
}
fn empty_value() -> Result<bool, IoError> {
    c0 = Process.command(Path("/bin/sh"))
    c1 = c0.arg("-c")
    c2 = c1.arg("printf '%s' \"${DISP_EMPTY+x}:$DISP_EMPTY\"")
    c3 = c2.environment("DISP_EMPTY", "")
    output = c3.run()?
    return Ok(match output.stdout_text() { Ok(text) => text == "x:", Err(error) => false })
}
fn main() {
    print(invalid(""))
    print(invalid("BAD=NAME"))
    print(match empty_value() { Ok(value) => value, Err(error) => false })
}
"#;
    differential_with_args("environment-validation", source, &[]);
}
