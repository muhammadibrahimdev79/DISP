use disp::{backend, check_path, check_source, lower_path, run_path};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PROJECT: AtomicUsize = AtomicUsize::new(0);

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let id = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "disp-module-tests-{}-{id}-{name}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    for (relative, source) in files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, source).unwrap();
    }
    root.join("main.disp")
}

fn native_output(entry: &Path) -> Option<String> {
    let (hir, mir) = lower_path(entry).unwrap();
    let artifacts = backend::build(&hir, &mir, entry, backend::BuildOptions::default()).unwrap();
    for attempt in 0..4 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(output.status.success());
                return Some(String::from_utf8(output.stdout).unwrap());
            }
            Err(error) if error.raw_os_error() == Some(4551) && attempt < 3 => continue,
            Err(error) if error.raw_os_error() == Some(4551) => return None,
            Err(error) => panic!("native module example should execute: {error}"),
        }
    }
    unreachable!()
}

fn native_failure(entry: &Path) -> Option<String> {
    let (hir, mir) = lower_path(entry).unwrap();
    let artifacts = backend::build(&hir, &mir, entry, backend::BuildOptions::default()).unwrap();
    for attempt in 0..4 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(!output.status.success());
                return Some(String::from_utf8(output.stderr).unwrap());
            }
            Err(error) if error.raw_os_error() == Some(4551) && attempt < 3 => continue,
            Err(error) if error.raw_os_error() == Some(4551) => return None,
            Err(error) => panic!("native failing module example should execute: {error}"),
        }
    }
    unreachable!()
}

#[test]
fn modules_compile_through_interpreter_and_native_backend() {
    let entry = project(
        "end-to-end",
        &[
            (
                "main.disp",
                "module main\nuse geometry\nfn main() { let p = Point { x: 3, y: 4 } print(length_squared(p)) print(kind(-7)) }",
            ),
            (
                "geometry.disp",
                "module geometry\npub struct Point { x: int, y: int }\nfn square(x: int) -> int = x * x\npub fn length_squared(p: Point) -> int = square(p.x) + square(p.y)\npub fn kind(x: int) -> int { if x < 0 { return 7 } return 0 }",
            ),
        ],
    );
    check_path(&entry).unwrap();
    let interpreted = run_path(&entry).unwrap().join("\n") + "\n";
    assert_eq!(interpreted, "25\n7\n");
    if let Some(native) = native_output(&entry) {
        assert_eq!(native.replace("\r\n", "\n"), interpreted);
    }
}

#[test]
fn selective_imports_and_public_reexports_are_deterministic() {
    let entry = project(
        "reexports",
        &[
            (
                "main.disp",
                "module main\nuse facade\nfn main() { print(answer()) }",
            ),
            ("facade.disp", "module facade\npub use internal.{answer}\n"),
            (
                "internal.disp",
                "module internal\npub fn answer() -> int = 42\npub fn unused() -> int = 0",
            ),
        ],
    );
    assert_eq!(run_path(&entry).unwrap(), ["42"]);
}

#[test]
fn private_items_do_not_leak_across_modules() {
    let entry = project(
        "private",
        &[
            (
                "main.disp",
                "module main\nuse vault\nfn main() { print(secret()) }",
            ),
            (
                "vault.disp",
                "module vault\nfn secret() -> int = 9\npub fn open() -> int = 1",
            ),
        ],
    );
    let error = check_path(&entry).unwrap_err();
    assert_eq!(error.message, "unknown name `secret`");
    assert!(error.file.unwrap().ends_with("main.disp"));
    assert_eq!(error.span.start.line, 3);
}

#[test]
fn module_cycles_report_the_import_chain_and_exact_source() {
    let entry = project(
        "cycle",
        &[
            ("main.disp", "module main\nuse alpha\nfn main() {}"),
            ("alpha.disp", "module alpha\nuse beta\npub fn a() {}"),
            ("beta.disp", "module beta\nuse alpha\npub fn b() {}"),
        ],
    );
    let error = check_path(&entry).unwrap_err();
    assert!(error.message.contains("alpha -> beta -> alpha"));
    assert!(error.file.unwrap().ends_with("beta.disp"));
    assert_eq!(error.span.start.line, 2);
}

#[test]
fn declarations_must_match_their_module_paths() {
    let entry = project(
        "declaration",
        &[
            ("main.disp", "use math\nfn main() {}"),
            ("math.disp", "module wrong\npub fn value() -> int = 1"),
        ],
    );
    let error = check_path(&entry).unwrap_err();
    assert!(error.message.contains("does not match source path `math`"));
    assert!(error.file.unwrap().ends_with("math.disp"));
    assert_eq!(error.span.start.line, 1);
}

#[test]
fn conflicting_wildcard_imports_fail_instead_of_guessing() {
    let entry = project(
        "collision",
        &[
            (
                "main.disp",
                "use left\nuse right\nfn main() { print(value()) }",
            ),
            ("left.disp", "pub fn value() -> int = 1"),
            ("right.disp", "pub fn value() -> int = 2"),
        ],
    );
    let error = check_path(&entry).unwrap_err();
    assert!(error.message.contains("conflicting items named `value`"));
    assert!(error.file.unwrap().ends_with("main.disp"));
    assert_eq!(error.span.start.line, 2);
}

#[test]
fn imported_external_functions_keep_their_real_c_link_names() {
    let entry = project(
        "external",
        &[
            (
                "main.disp",
                "use native\nfn main() { unsafe { print(abs(-12)) } }",
            ),
            (
                "native.disp",
                "pub extern C { fn abs(value: CInt) -> CInt }",
            ),
        ],
    );
    assert_eq!(run_path(&entry).unwrap(), ["12"]);
    if let Some(native) = native_output(&entry) {
        assert_eq!(native.trim(), "12");
    }
}

#[test]
fn missing_and_non_public_selected_items_have_source_diagnostics() {
    let entry = project(
        "selected-private",
        &[
            ("main.disp", "use values.{hidden}\nfn main() {}"),
            (
                "values.disp",
                "fn hidden() -> int = 1\npub fn visible() -> int = 2",
            ),
        ],
    );
    let error = check_path(&entry).unwrap_err();
    assert!(error.message.contains("has no public item `hidden`"));
    assert!(error.file.unwrap().ends_with("main.disp"));
    assert_eq!(error.span.start.line, 1);
}

#[test]
fn package_directories_use_their_strict_manifest_entry() {
    let marker = project(
        "package-directory",
        &[
            (
                "DISP.toml",
                "[package]\nname = \"calculator\"\nversion = \"1.2.3\"\nedition = \"1\"\nentry = \"src/start.disp\"\n",
            ),
            (
                "src/start.disp",
                "module start\nuse math\nfn main() { print(double(21)) }",
            ),
            (
                "src/math.disp",
                "module math\npub fn double(value: int) -> int = value * 2",
            ),
        ],
    );
    let root = marker.parent().unwrap();
    assert_eq!(run_path(root).unwrap(), ["42"]);
    if let Some(native) = native_output(root) {
        assert_eq!(native.trim(), "42");
    }
}

#[test]
fn malformed_manifests_fail_closed_with_exact_locations() {
    let marker = project(
        "bad-manifest",
        &[
            (
                "DISP.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1\"\nmystery = \"ignored?\"\n",
            ),
            ("src/main.disp", "fn main() {}"),
        ],
    );
    let error = check_path(marker.parent().unwrap()).unwrap_err();
    assert_eq!(error.message, "unknown package field `mystery`");
    assert!(error.file.unwrap().ends_with("DISP.toml"));
    assert_eq!(error.span.start.line, 4);
}

#[test]
fn malformed_manifest_forms_never_receive_guessed_meanings() {
    let cases = [
        (
            "name = \"demo\"\nversion = \"0.1.0\"\n",
            "must appear under `[package]`",
        ),
        (
            "[package]\nname = \"Demo\"\nversion = \"0.1.0\"\n",
            "package names must be",
        ),
        (
            "[package]\nname = \"demo\"\nversion = \"01.2.3\"\n",
            "MAJOR.MINOR.PATCH",
        ),
        (
            "[package]\nname = \"demo\"\nname = \"again\"\nversion = \"0.1.0\"\n",
            "duplicate package field",
        ),
        (
            "[dependencies]\nname = \"demo\"\n",
            "local dependency must use",
        ),
        (
            "[package]\nname = \"demo\"\n[package]\nversion = \"0.1.0\"\n",
            "duplicate `[package]` section",
        ),
        (
            "[package]\nname = demo\nversion = \"0.1.0\"\n",
            "quoted strings",
        ),
    ];
    for (index, (manifest, expected)) in cases.into_iter().enumerate() {
        let marker = project(
            &format!("malformed-{index}"),
            &[("DISP.toml", manifest), ("src/main.disp", "fn main() {}")],
        );
        let error = check_path(marker.parent().unwrap()).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(error.file.unwrap().ends_with("DISP.toml"));
    }
}

#[test]
fn nested_module_paths_are_resolved_from_the_source_root() {
    let entry = project(
        "nested",
        &[
            (
                "main.disp",
                "use tools.math.{triple}\nfn main() { print(triple(14)) }",
            ),
            (
                "tools/math.disp",
                "module tools.math\npub fn triple(value: int) -> int = value * 3",
            ),
        ],
    );
    assert_eq!(run_path(&entry).unwrap(), ["42"]);
}

#[test]
fn package_entries_cannot_escape_the_project() {
    let marker = project(
        "entry-escape",
        &[(
            "DISP.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nentry = \"../outside.disp\"\n",
        )],
    );
    let error = check_path(marker.parent().unwrap()).unwrap_err();
    assert!(error.message.contains("relative `.disp` path"));
    assert!(error.file.unwrap().ends_with("DISP.toml"));
    assert_eq!(error.span.start.line, 4);
}

#[test]
fn selective_aliases_keep_same_named_nominal_types_distinct() {
    let entry = project(
        "aliases",
        &[
            (
                "main.disp",
                "use left.{Token as LeftToken}\nuse right.{Token as RightToken}\nfn main() { let left = LeftToken { value: 20 } let right = RightToken { value: 22 } print(left.value + right.value) }",
            ),
            ("left.disp", "pub struct Token { value: int }"),
            ("right.disp", "pub struct Token { value: int }"),
        ],
    );
    assert_eq!(run_path(&entry).unwrap(), ["42"]);

    let mismatch = project(
        "nominal-mismatch",
        &[
            (
                "main.disp",
                "use left.{Token as LeftToken}\nuse right.{accept}\nfn main() { accept(LeftToken { value: 1 }) }",
            ),
            ("left.disp", "pub struct Token { value: int }"),
            (
                "right.disp",
                "pub struct Token { value: int }\npub fn accept(value: Token) {}",
            ),
        ],
    );
    let error = check_path(&mismatch).unwrap_err();
    assert!(error.message.contains("function argument expected"));
    assert!(error.file.unwrap().ends_with("main.disp"));
}

#[test]
fn public_apis_cannot_accidentally_expose_private_types() {
    let entry = project(
        "private-api",
        &[
            ("main.disp", "use service\nfn main() {}"),
            (
                "service.disp",
                "struct Secret { value: int }\npub fn reveal() -> Secret { return Secret { value: 1 } }",
            ),
        ],
    );
    let error = check_path(&entry).unwrap_err();
    assert_eq!(
        error.message,
        "public API exposes private type or trait `Secret`"
    );
    assert!(error.file.unwrap().ends_with("service.disp"));
    assert_eq!(error.span.start.line, 2);
}

#[test]
fn runtime_failures_name_the_imported_source_in_both_engines() {
    let entry = project(
        "runtime-source",
        &[
            ("main.disp", "use fault\nfn main() { fail(9) }"),
            (
                "fault.disp",
                "pub fn fail(index: int) {\n    let values = [10, 20]\n    print(values[index])\n}",
            ),
        ],
    );
    let interpreted = run_path(&entry).unwrap_err();
    assert!(interpreted.file.unwrap().ends_with("fault.disp"));
    assert_eq!(interpreted.span.start.line, 3);
    if let Some(native) = native_failure(&entry) {
        assert!(native.contains("fault.disp:3:"), "{native}");
        assert!(native.contains("index out of bounds"), "{native}");
    }
}

#[test]
fn module_identities_and_source_maps_survive_hir_and_mir_lowering() {
    let entry = project(
        "module-ir",
        &[
            (
                "main.disp",
                "use math.{increment}\nfn main() { print(increment(41)) }",
            ),
            (
                "math.disp",
                "pub fn increment(value: int) -> int = value + 1",
            ),
        ],
    );
    let (hir, mir) = lower_path(&entry).unwrap();
    assert!(
        hir.functions
            .iter()
            .any(|function| function.name == "$disp$math$increment")
    );
    assert!(
        mir.functions
            .iter()
            .any(|function| function.name == "$disp$math$increment")
    );
    assert_eq!(hir.source_files.len(), 2);
    assert_eq!(mir.source_files, hir.source_files);

    let (second_hir, second_mir) = lower_path(&entry).unwrap();
    assert_eq!(disp::hir::dump(&hir), disp::hir::dump(&second_hir));
    assert_eq!(disp::mir::dump(&mir), disp::mir::dump(&second_mir));
}

#[test]
fn string_only_compiler_apis_reject_imports_without_guessing_a_filesystem_root() {
    let error = check_source("use math\nfn main() {}").unwrap_err();
    assert_eq!(
        error.message,
        "module imports require a source path or project directory"
    );
    assert!(error.help.unwrap().contains("check_path"));
}
