use std::{fs, process::Command};

fn disp(arguments: &[&str]) -> Option<std::process::Output> {
    for attempt in 0..4 {
        match Command::new(env!("CARGO_BIN_EXE_disp"))
            .args(arguments)
            .output()
        {
            Ok(output) => return Some(output),
            Err(error) if error.raw_os_error() == Some(4551) && attempt < 3 => continue,
            Err(error) if error.raw_os_error() == Some(4551) => return None,
            Err(error) => panic!("disp should execute: {error}"),
        }
    }
    unreachable!()
}

fn source_file(name: &str, source: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("disp-tests-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("temporary directory should be created");
    let path = directory.join(name);
    fs::write(&path, source).expect("temporary source should be written");
    path
}

#[test]
fn version_and_help_identify_the_developer_preview() {
    let Some(version) = disp(&["--version"]) else {
        return;
    };
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        "DISP 0.1.0 Developer Preview"
    );
    let Some(help) = disp(&["--help"]) else {
        return;
    };
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("disp <file.disp|project-directory>"));
    assert!(help.contains("program arguments follow `--`"));
    assert!(help.contains("disp fmt [--check]"));
    assert!(help.contains("disp migrate [--check]"));
    assert!(help.contains("--diagnostic-format=<human|json>"));
    assert!(help.contains("--sanitize"));
    assert!(help.contains("--freestanding"));
    assert!(help.contains("--freestanding32"));
    assert!(help.contains("--freestanding64"));
    assert!(help.contains("--freestanding-aarch64"));
    assert!(help.contains("header"));
}

#[test]
fn header_command_writes_the_versioned_c_contract_deterministically() {
    let path = source_file(
        "header-cli.disp",
        "extern C(\"fixture\") { fn fixture_add(left: CInt, right: CInt) -> CInt } fn main() {}",
    );
    let Some(first) = disp(&["header", path.to_str().unwrap()]) else {
        return;
    };
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let header = path.with_extension("h");
    assert_eq!(
        String::from_utf8(first.stdout).unwrap().trim(),
        header.display().to_string()
    );
    let first_bytes = fs::read(&header).unwrap();
    let Some(second) = disp(&["header", path.to_str().unwrap()]) else {
        return;
    };
    assert!(second.status.success());
    assert_eq!(fs::read(&header).unwrap(), first_bytes);
    let text = String::from_utf8(first_bytes).unwrap();
    assert!(text.contains("#define DISP_C_ABI_VERSION 1u"));
    assert!(text.contains("int32_t fixture_add(int32_t arg1, int32_t arg2);"));
}

#[test]
fn library_command_writes_a_shared_artifact_and_consumer_header() {
    let path = source_file(
        "library-cli.disp",
        "export C fn library_value() -> CInt uses Pure { return 42 } fn main() {}",
    );
    let Some(output) = disp(&["build", "--library", path.to_str().unwrap()]) else {
        return;
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(std::path::Path::new(&lines[0]).is_file());
    assert_eq!(std::path::Path::new(&lines[1]), path.with_extension("h"));
    let header = fs::read_to_string(&lines[1]).unwrap();
    assert!(header.contains("DISP_C_API int32_t library_value(int32_t *out_result);"));
}

#[test]
fn sanitized_builds_are_instrumented_or_fail_closed() {
    let path = source_file(
        "sanitized-cli.disp",
        "fn main() { let value = 7 print(value) }",
    );
    let Some(output) = disp(&["build", "--sanitize", path.to_str().unwrap()]) else {
        return;
    };
    if output.status.success() {
        let executable = path.parent().unwrap().join("build/sanitized-cli.exe");
        assert!(executable.is_file());
        if cfg!(windows) {
            assert!(
                executable
                    .parent()
                    .unwrap()
                    .read_dir()
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("clang_rt.asan_dynamic-")
                            && name.ends_with(".dll"))),
                "successful sanitized Windows builds must include the ASan runtime"
            );
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("native linking failed")
                || stderr.contains("sanitizer runtime")
                || stderr.contains("libasan")
                || stderr.contains("libubsan"),
            "sanitizer failure must identify the unavailable instrumented toolchain: {stderr}"
        );
        assert!(!stderr.contains("usage:"));
    }
}

#[test]
fn migrate_is_explicit_idempotent_and_checkable() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("disp-migrate-{}-{unique}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    let manifest = root.join("DISP.toml");
    let legacy = "[package]\nname = \"legacy\"\nversion = \"0.1.0\"\nentry = \"src/main.disp\"\n";
    fs::write(&manifest, legacy).unwrap();
    fs::write(root.join("src/main.disp"), "fn main() {}\n").unwrap();

    let Some(before) = disp(&["migrate", "--check", root.to_str().unwrap()]) else {
        return;
    };
    assert!(!before.status.success());
    assert!(String::from_utf8_lossy(&before.stderr).contains("run `disp migrate"));
    assert_eq!(fs::read_to_string(&manifest).unwrap(), legacy);

    let Some(migrated) = disp(&["migrate", root.to_str().unwrap()]) else {
        return;
    };
    assert!(migrated.status.success());
    let expected = "[package]\nname = \"legacy\"\nversion = \"0.1.0\"\nedition = \"1\"\nfeatures = []\nentry = \"src/main.disp\"\n";
    assert_eq!(fs::read_to_string(&manifest).unwrap(), expected);

    let Some(again) = disp(&["migrate", root.to_str().unwrap()]) else {
        return;
    };
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("already declares"));
    assert_eq!(fs::read_to_string(&manifest).unwrap(), expected);

    let Some(checked) = disp(&["migrate", "--check", root.to_str().unwrap()]) else {
        return;
    };
    assert!(checked.status.success());
    assert_eq!(fs::read_to_string(&manifest).unwrap(), expected);
}

#[test]
fn json_diagnostics_are_structured_stable_and_global() {
    let source = source_file("json-invalid.disp", "fn main() { print(missing) }");
    let path = source.to_str().unwrap();
    let Some(leading) = disp(&["--diagnostic-format=json", "check", path]) else {
        return;
    };
    let Some(infix) = disp(&["check", "--diagnostic-format=json", path]) else {
        return;
    };
    assert!(!leading.status.success() && !infix.status.success());
    assert!(leading.stdout.is_empty() && infix.stdout.is_empty());
    assert_eq!(leading.stderr, infix.stderr);
    let json = String::from_utf8(leading.stderr).unwrap();
    assert_eq!(json.lines().count(), 1);
    assert!(json.starts_with("{\"schema\":\"disp.diagnostic.v1\""));
    assert!(json.contains("\"code\":\"DISP-RESOLVE-0001\""));
    assert!(json.contains("\"stage\":\"resolver\""));
    assert!(json.contains("\"message\":\"unknown name `missing`\""));
    assert!(json.contains("\"span\":{\"start\":"));
    assert!(json.trim_end().ends_with('}'));
}

#[test]
fn json_driver_errors_have_null_source_locations() {
    let Some(output) = disp(&["--diagnostic-format=json", "unknown", "extra"]) else {
        return;
    };
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let json = String::from_utf8(output.stderr).unwrap();
    assert!(json.contains("\"code\":\"DISP-DRIVER-0001\""));
    assert!(json.contains("\"stage\":\"driver\""));
    assert!(json.contains("\"file\":null,\"span\":null"));
}

#[test]
fn diagnostic_option_after_separator_remains_a_program_argument() {
    let source = source_file(
        "diagnostic-argument.disp",
        "fn main(args: List<String>) { print(args.len()) print(match args.get(0) { Some(value) => (*value).contains(\"--diagnostic-format=json\"), None => false }) }",
    );
    let Some(output) = disp(&[
        "interpret",
        source.to_str().unwrap(),
        "--",
        "--diagnostic-format=json",
    ]) else {
        return;
    };
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "1\ntrue\n"
    );
}

#[test]
fn formatter_is_idempotent_and_check_mode_is_non_mutating() {
    let source = source_file(
        "format.disp",
        "fn main() {  \r\n\tif true {\r\n\t\tprint(\"DISP\")   \r\n\t}\r\n}\r\n",
    );
    let path = source.to_str().unwrap();
    let original = fs::read(&source).unwrap();
    let Some(check_before) = disp(&["fmt", "--check", path]) else {
        return;
    };
    assert!(!check_before.status.success());
    assert_eq!(fs::read(&source).unwrap(), original);

    let Some(format) = disp(&["fmt", path]) else {
        return;
    };
    assert!(format.status.success());
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "fn main() {\n    if true {\n        print(\"DISP\")\n    }\n}\n"
    );

    let Some(check_after) = disp(&["fmt", "--check", path]) else {
        return;
    };
    assert!(check_after.status.success());
}

#[test]
fn formatter_walks_project_sources_including_nested_modules() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let project = std::env::temp_dir().join(format!(
        "disp-format-project-{}-{unique}",
        std::process::id()
    ));
    let nested = project.join("src/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        project.join("src/main.disp"),
        "fn main() {  \n\tprint(1) \n}\n",
    )
    .unwrap();
    fs::write(
        nested.join("values.disp"),
        "module values  \npub fn answer() -> int = 42  \n",
    )
    .unwrap();

    let Some(format) = disp(&["fmt", project.to_str().unwrap()]) else {
        return;
    };
    assert!(format.status.success());
    assert_eq!(
        fs::read_to_string(project.join("src/main.disp")).unwrap(),
        "fn main() {\n    print(1)\n}\n"
    );
    assert_eq!(
        fs::read_to_string(nested.join("values.disp")).unwrap(),
        "module values\npub fn answer() -> int = 42\n"
    );

    let Some(check) = disp(&["fmt", "--check", project.to_str().unwrap()]) else {
        return;
    };
    assert!(check.status.success());
}

#[test]
fn run_and_check_commands_use_the_full_pipeline() {
    let source = source_file(
        "valid.disp",
        "fn double(x: int) -> int { return x * 2 } fn main() { print(double(21)) }",
    );
    let Some(run) = disp(&["run", source.to_str().unwrap()]) else {
        return;
    };
    if !run.status.success() && String::from_utf8_lossy(&run.stderr).contains("os error 4551") {
        return;
    }
    assert!(run.status.success());
    assert_eq!(String::from_utf8(run.stdout).unwrap().trim(), "42");

    let Some(check) = disp(&["check", source.to_str().unwrap()]) else {
        return;
    };
    assert!(check.status.success());
    assert!(check.stdout.is_empty());

    let Some(build) = disp(&["build", "--emit-obj", source.to_str().unwrap()]) else {
        return;
    };
    assert!(build.status.success());
    let executable = source.parent().unwrap().join("build").join("valid.exe");
    let object = source
        .parent()
        .unwrap()
        .join("build")
        .join("valid")
        .join("valid.o");
    assert!(executable.exists());
    assert!(object.exists());
}

#[test]
fn native_run_cache_reuses_unchanged_builds_and_tracks_imports() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("disp-native-cache-{}-{unique}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let entry = directory.join("main.disp");
    let dependency = directory.join("answer.disp");
    fs::write(
        &entry,
        "module main\nuse answer\nfn main() { print(value()) }\n",
    )
    .unwrap();
    fs::write(&dependency, "module answer\npub fn value() -> int = 42\n").unwrap();

    let path = entry.to_str().unwrap();
    let Some(first) = disp(&[path]) else {
        return;
    };
    if !first.status.success() && String::from_utf8_lossy(&first.stderr).contains("os error 4551") {
        return;
    }
    assert!(first.status.success());
    assert_eq!(String::from_utf8(first.stdout).unwrap().trim(), "42");
    let executable = directory.join("build/main.exe");
    let fingerprint = directory.join("build/main/fingerprint.sha256");
    assert!(fingerprint.is_file());
    let first_modified = fs::metadata(&executable).unwrap().modified().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let Some(cached) = disp(&[path]) else {
        return;
    };
    if !cached.status.success() && String::from_utf8_lossy(&cached.stderr).contains("os error 4551")
    {
        return;
    }
    assert!(cached.status.success());
    assert_eq!(String::from_utf8(cached.stdout).unwrap().trim(), "42");
    assert_eq!(
        fs::metadata(&executable).unwrap().modified().unwrap(),
        first_modified,
        "an unchanged program should reuse its native executable"
    );

    fs::write(&dependency, "module answer\npub fn value() -> int = 43\n").unwrap();
    let Some(rebuilt) = disp(&[path]) else {
        return;
    };
    if !rebuilt.status.success()
        && String::from_utf8_lossy(&rebuilt.stderr).contains("os error 4551")
    {
        return;
    }
    assert!(
        rebuilt.status.success(),
        "rebuilt native program failed: {}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    assert_eq!(String::from_utf8(rebuilt.stdout).unwrap().trim(), "43");
    assert_ne!(
        fs::metadata(&executable).unwrap().modified().unwrap(),
        first_modified,
        "changing an imported module must invalidate the native cache"
    );

    fs::write(
        &dependency,
        "module answer\npub fn value() -> int = missing\n",
    )
    .unwrap();
    let Some(invalid) = disp(&[path]) else {
        return;
    };
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty(), "stale native code must not run");
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("unknown name `missing`")
    );
}

#[test]
fn run_and_interpret_forward_arguments_after_separator() {
    let source = source_file(
        "arguments.disp",
        "fn main(args: List<String>) { print(args.len()) print(match args.get(0) { Some(value) => (*value).contains(\"hello world\"), None => false }) }",
    );
    let path = source.to_str().unwrap();
    let Some(interpreted) = disp(&["interpret", path, "--", "hello world", "second"]) else {
        return;
    };
    assert!(interpreted.status.success());
    assert_eq!(
        String::from_utf8(interpreted.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "2\ntrue\n"
    );

    let Some(native) = disp(&["run", path, "--", "hello world", "second"]) else {
        return;
    };
    if !native.status.success() && String::from_utf8_lossy(&native.stderr).contains("os error 4551")
    {
        return;
    }
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        String::from_utf8(native.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "2\ntrue\n"
    );
}

#[test]
fn compile_fail_reports_stage_and_location() {
    let source = source_file("invalid.disp", "fn main() { print(missing) }");
    let Some(output) = disp(&["check", source.to_str().unwrap()]) else {
        return;
    };
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("resolver error"));
    assert!(stderr.contains("unknown name `missing`"));
    assert!(stderr.contains("invalid.disp:1:"));
}

#[test]
fn rejects_non_disp_sources() {
    let source = source_file("invalid.txt", "fn main() {}");
    let Some(output) = disp(&[source.to_str().unwrap()]) else {
        return;
    };
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("must end with `.disp`")
    );
}

#[test]
fn developer_hir_and_mir_dumps_are_available() {
    let source = source_file("dump.disp", "fn main() { let value = 1 print(value) }");
    let Some(hir) = disp(&["check", "--dump-hir", source.to_str().unwrap()]) else {
        return;
    };
    let Some(mir) = disp(&["check", "--dump-mir", source.to_str().unwrap()]) else {
        return;
    };
    assert!(hir.status.success() && mir.status.success());
    assert!(String::from_utf8(hir.stdout).unwrap().contains("fn0 main"));
    let mir = String::from_utf8(mir.stdout).unwrap();
    assert!(mir.contains("mir fn0 main") && mir.contains("bb0:"));
}

#[test]
fn effect_dump_is_deterministic_and_distinguishes_contracts_from_inference() {
    let source = source_file(
        "effects.disp",
        "fn load(path: Path) -> Result<String, IoError> uses FileSystem = File.read_text(path)\nfn inferred(path: Path) -> Result<String, IoError> = load(path)\nfn main() uses Pure {}\n",
    );
    let Some(output) = disp(&["check", "--dump-effects", source.to_str().unwrap()]) else {
        return;
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
        "load uses FileSystem\ninferred uses FileSystem [inferred]\nmain uses Pure\n"
    );
}

#[test]
fn constant_dump_is_deterministic_and_reports_evaluated_values() {
    let source = source_file(
        "constants.disp",
        "fn main() { const base = 7 const answer = base * 6 const label = \"DISP\" print(answer) }",
    );
    let Some(output) = disp(&["check", "--dump-constants", source.to_str().unwrap()]) else {
        return;
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
        "main::base = 7\nmain::answer = 42\nmain::label = \"DISP\"\n"
    );
}

#[test]
fn expansion_dump_records_bounded_structured_generation() {
    let source = source_file(
        "expansions.disp",
        "fn main() { let values = Meta.map(3, |index: int| index + 1) print(values) }",
    );
    let Some(output) = disp(&["check", "--dump-expansions", source.to_str().unwrap()]) else {
        return;
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout)
        .unwrap()
        .replace("\r\n", "\n");
    assert!(output.starts_with("Meta.map generated "), "{output}");
    assert!(output.ends_with(" nodes at 1:26\n"), "{output}");
}

#[test]
fn new_creates_a_runnable_directory_project() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("disp-new-{}-{unique}", std::process::id()));
    let Some(created) = disp(&["new", root.to_str().unwrap()]) else {
        return;
    };
    assert!(created.status.success());
    assert!(root.join("DISP.toml").is_file());
    assert!(root.join("src/main.disp").is_file());
    let manifest = fs::read_to_string(root.join("DISP.toml")).unwrap();
    assert!(manifest.contains("edition = \"1\""));
    assert!(manifest.contains("features = []"));

    let Some(checked) = disp(&["check", root.to_str().unwrap()]) else {
        return;
    };
    assert!(checked.status.success());
    let Some(interpreted) = disp(&["interpret", root.to_str().unwrap()]) else {
        return;
    };
    assert!(interpreted.status.success());
    assert_eq!(
        String::from_utf8(interpreted.stdout).unwrap().trim(),
        "Hello from DISP"
    );
    let Some(native) = disp(&["run", root.to_str().unwrap()]) else {
        return;
    };
    if !native.status.success() && String::from_utf8_lossy(&native.stderr).contains("os error 4551")
    {
        return;
    }
    assert!(native.status.success());
    assert_eq!(
        String::from_utf8(native.stdout).unwrap().trim(),
        "Hello from DISP"
    );

    let Some(second) = disp(&["new", root.to_str().unwrap()]) else {
        return;
    };
    assert!(!second.status.success());
    assert!(
        String::from_utf8(second.stderr)
            .unwrap()
            .contains("refusing to overwrite")
    );
}

#[test]
fn lock_command_is_explicit_deterministic_and_enables_dependency_builds() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("disp-lock-cli-{}-{unique}", std::process::id()));
    let app = root.join("app");
    let library = root.join("library");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(library.join("src")).unwrap();
    fs::write(
        app.join("DISP.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nanswer = { path = \"../library\" }\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.disp"),
        "use answer\nfn main() { print(value()) }",
    )
    .unwrap();
    fs::write(
        library.join("DISP.toml"),
        "[package]\nname = \"answer\"\nversion = \"1.0.0\"\nentry = \"src/lib.disp\"\n",
    )
    .unwrap();
    fs::write(library.join("src/lib.disp"), "pub fn value() -> int = 42").unwrap();

    let Some(before) = disp(&["check", app.to_str().unwrap()]) else {
        return;
    };
    assert!(!before.status.success());
    assert!(
        String::from_utf8(before.stderr)
            .unwrap()
            .contains("DISP.lock")
    );

    let Some(locked) = disp(&["lock", app.to_str().unwrap()]) else {
        return;
    };
    assert!(locked.status.success());
    let first = fs::read(app.join("DISP.lock")).unwrap();
    let Some(locked_again) = disp(&["lock", app.to_str().unwrap()]) else {
        return;
    };
    assert!(locked_again.status.success());
    assert_eq!(fs::read(app.join("DISP.lock")).unwrap(), first);

    let Some(tree) = disp(&["tree", app.to_str().unwrap()]) else {
        return;
    };
    assert!(tree.status.success());
    let tree = String::from_utf8(tree.stdout).unwrap();
    assert!(tree.contains("app@0.1.0"));
    assert!(tree.contains("answer -> answer@1.0.0"));

    let Some(run) = disp(&["run", app.to_str().unwrap()]) else {
        return;
    };
    if !run.status.success() && String::from_utf8_lossy(&run.stderr).contains("os error 4551") {
        return;
    }
    assert!(run.status.success());
    assert_eq!(String::from_utf8(run.stdout).unwrap().trim(), "42");
}
