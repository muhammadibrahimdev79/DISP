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
