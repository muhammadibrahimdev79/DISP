use disp::{backend, check_path, package, run_path};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_WORKSPACE: AtomicUsize = AtomicUsize::new(0);

fn workspace(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let id = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "disp-package-tests-{}-{id}-{name}",
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
    root
}

fn basic_workspace(name: &str) -> PathBuf {
    workspace(
        name,
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"1\"\nentry = \"src/main.disp\"\n\n[dependencies]\narithmetic = { path = \"../math\" }\n",
            ),
            (
                "app/src/main.disp",
                "module main\nuse arithmetic\nfn main() { print(double(21)) }",
            ),
            (
                "math/DISP.toml",
                "[package]\nname = \"math\"\nversion = \"1.0.0\"\nedition = \"1\"\nentry = \"src/lib.disp\"\n",
            ),
            (
                "math/src/lib.disp",
                "module lib\npub fn double(value: int) -> int = value * 2",
            ),
        ],
    )
}

fn native_output(project: &Path) -> Option<String> {
    let (hir, mir) = disp::lower_path(project).unwrap();
    let artifacts = backend::build(&hir, &mir, project, backend::BuildOptions::default()).unwrap();
    for attempt in 0..4 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(output.status.success());
                return Some(String::from_utf8(output.stdout).unwrap());
            }
            Err(error) if error.raw_os_error() == Some(4551) && attempt < 3 => continue,
            Err(error) if error.raw_os_error() == Some(4551) => return None,
            Err(error) => panic!("native dependency program should execute: {error}"),
        }
    }
    unreachable!()
}

#[test]
fn local_dependencies_require_an_explicit_lock_then_compile_differentially() {
    let root = basic_workspace("end-to-end");
    let app = root.join("app");
    let error = check_path(&app).unwrap_err();
    assert!(error.message.contains("require `DISP.lock`"));
    assert!(error.file.unwrap().ends_with("DISP.lock"));

    let lock = package::write_lock(&app).unwrap();
    assert_eq!(lock, fs::canonicalize(&app).unwrap().join("DISP.lock"));
    check_path(&app).unwrap();
    let interpreted = run_path(&app).unwrap().join("\n") + "\n";
    assert_eq!(interpreted, "42\n");
    if let Some(native) = native_output(&app) {
        assert_eq!(native.replace("\r\n", "\n"), interpreted);
    }
}

#[test]
fn lockfiles_are_byte_deterministic_and_human_inspectable() {
    let root = basic_workspace("deterministic");
    let app = root.join("app");
    let lock = package::write_lock(&app).unwrap();
    let first = fs::read(&lock).unwrap();
    package::write_lock(&app).unwrap();
    let second = fs::read(&lock).unwrap();
    assert_eq!(first, second);
    let text = String::from_utf8(first).unwrap();
    assert!(text.contains("lock-version = \"1\""));
    assert!(text.contains("id = \"math@1.0.0\""));
    assert!(text.contains("arithmetic=math@1.0.0"));
    assert!(text.contains("sha256 = \""));
    assert!(!text.contains('\\'));
}

#[test]
fn changed_dependency_contents_fail_until_explicitly_relocked() {
    let root = basic_workspace("integrity");
    let app = root.join("app");
    package::write_lock(&app).unwrap();
    fs::write(
        root.join("math/src/lib.disp"),
        "module lib\npub fn double(value: int) -> int = value * 3",
    )
    .unwrap();
    let error = check_path(&app).unwrap_err();
    assert!(error.message.contains("does not match"));
    assert!(error.help.unwrap().contains("review"));

    package::write_lock(&app).unwrap();
    assert_eq!(run_path(&app).unwrap(), ["63"]);
}

#[test]
fn transitive_dependencies_and_public_reexports_use_declared_aliases() {
    let root = workspace(
        "transitive",
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nmath = { path = \"../math\" }\n",
            ),
            (
                "app/src/main.disp",
                "use math\nfn main() { print(answer()) }",
            ),
            (
                "math/DISP.toml",
                "[package]\nname = \"math\"\nversion = \"1.0.0\"\nentry = \"src/lib.disp\"\n[dependencies]\nfoundation = { path = \"../foundation\" }\n",
            ),
            (
                "math/src/lib.disp",
                "module lib\npub use foundation.{answer}",
            ),
            (
                "foundation/DISP.toml",
                "[package]\nname = \"foundation\"\nversion = \"2.0.0\"\nentry = \"src/lib.disp\"\n",
            ),
            (
                "foundation/src/lib.disp",
                "module lib\npub fn answer() -> int = 42",
            ),
        ],
    );
    let app = root.join("app");
    package::write_lock(&app).unwrap();
    assert_eq!(run_path(&app).unwrap(), ["42"]);
    let lock = fs::read_to_string(app.join("DISP.lock")).unwrap();
    assert!(lock.contains("foundation=foundation@2.0.0"));
}

#[test]
fn dependency_tree_expands_shared_packages_only_once() {
    let root = workspace(
        "shared-tree",
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nleft = { path = \"../left\" }\nright = { path = \"../right\" }\n",
            ),
            ("app/src/main.disp", "fn main() {}"),
            (
                "left/DISP.toml",
                "[package]\nname = \"left\"\nversion = \"1.0.0\"\n[dependencies]\ncommon = { path = \"../common\" }\n",
            ),
            ("left/src/main.disp", "fn left() {}"),
            (
                "right/DISP.toml",
                "[package]\nname = \"right\"\nversion = \"1.0.0\"\n[dependencies]\ncommon = { path = \"../common\" }\n",
            ),
            ("right/src/main.disp", "fn right() {}"),
            (
                "common/DISP.toml",
                "[package]\nname = \"common\"\nversion = \"1.0.0\"\n[dependencies]\nleaf = { path = \"../leaf\" }\n",
            ),
            ("common/src/main.disp", "fn common() {}"),
            (
                "leaf/DISP.toml",
                "[package]\nname = \"leaf\"\nversion = \"1.0.0\"\n",
            ),
            ("leaf/src/main.disp", "fn leaf() {}"),
        ],
    );
    let graph = package::resolve(&root.join("app")).unwrap();
    let tree = graph.tree();
    assert_eq!(tree.len(), 6);
    assert_eq!(
        tree.iter().filter(|line| line.id == "leaf@1.0.0").count(),
        1
    );
}

#[test]
fn package_cycles_are_rejected_with_the_dependency_source_location() {
    let root = workspace(
        "cycle",
        &[
            (
                "a/DISP.toml",
                "[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dependencies]\nb = { path = \"../b\" }\n",
            ),
            ("a/src/main.disp", "fn main() {}"),
            (
                "b/DISP.toml",
                "[package]\nname = \"b\"\nversion = \"1.0.0\"\n[dependencies]\na = { path = \"../a\" }\n",
            ),
            ("b/src/main.disp", "fn ignored() {}"),
        ],
    );
    let error = package::write_lock(&root.join("a")).unwrap_err();
    assert!(error.message.contains("package dependency cycle"));
    let file = error.file.unwrap().replace('\\', "/");
    assert!(file.ends_with("b/DISP.toml"));
}

#[test]
fn duplicate_package_identities_from_different_sources_are_rejected() {
    let root = workspace(
        "identity",
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nleft = { path = \"../left\" }\nright = { path = \"../right\" }\n",
            ),
            ("app/src/main.disp", "fn main() {}"),
            (
                "left/DISP.toml",
                "[package]\nname = \"shared\"\nversion = \"1.0.0\"\n",
            ),
            ("left/src/main.disp", "pub fn left() {}"),
            (
                "right/DISP.toml",
                "[package]\nname = \"shared\"\nversion = \"1.0.0\"\n",
            ),
            ("right/src/main.disp", "pub fn right() {}"),
        ],
    );
    let error = package::write_lock(&root.join("app")).unwrap_err();
    assert!(error.message.contains("resolves to both"));
}

#[test]
fn unsupported_dependency_sources_and_malformed_specs_fail_closed() {
    for (index, dependency) in [
        "math = \"1.0.0\"",
        "math = { git = \"https://example.invalid/math\" }",
        "math = { path = \"C:/absolute\" }",
        "math = { path = \"../math\", version = \"1\" }",
    ]
    .into_iter()
    .enumerate()
    {
        let manifest = format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\n{dependency}\n"
        );
        let root = workspace(
            &format!("invalid-{index}"),
            &[
                ("app/DISP.toml", &manifest),
                ("app/src/main.disp", "fn main() {}"),
            ],
        );
        let error = package::write_lock(&root.join("app")).unwrap_err();
        assert!(
            error.message.contains("local dependency"),
            "{}",
            error.message
        );
        assert_eq!(error.span.start.line, 5);
    }
}

#[test]
fn dependency_aliases_must_be_importable_disp_identifiers() {
    for alias in ["math-kit", "Math", "2math", "math.tool"] {
        let manifest = format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\n{alias} = {{ path = \"../math\" }}\n"
        );
        let root = workspace(
            "bad-alias",
            &[
                ("app/DISP.toml", &manifest),
                ("app/src/main.disp", "fn main() {}"),
            ],
        );
        let error = package::write_lock(&root.join("app")).unwrap_err();
        assert!(error.message.contains("valid lowercase DISP identifiers"));
    }
}

#[test]
fn repeated_dependency_sections_are_rejected_instead_of_merged() {
    let root = workspace(
        "repeated-dependencies",
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nleft = { path = \"../left\" }\n[dependencies]\nright = { path = \"../right\" }\n",
            ),
            ("app/src/main.disp", "fn main() {}"),
        ],
    );
    let error = package::write_lock(&root.join("app")).unwrap_err();
    assert!(error.message.contains("duplicate `[dependencies]` section"));
    assert_eq!(error.span.start.line, 6);
}

#[test]
fn malformed_or_manually_edited_lockfiles_are_never_partially_accepted() {
    let root = basic_workspace("bad-lock");
    let app = root.join("app");
    package::write_lock(&app).unwrap();
    fs::write(app.join("DISP.lock"), "lock-version = \"999\"\n").unwrap();
    let error = check_path(&app).unwrap_err();
    assert!(error.message.contains("does not match"));
    assert!(error.file.unwrap().ends_with("DISP.lock"));
}

#[test]
fn dependency_modules_keep_nominal_types_and_runtime_locations_package_scoped() {
    let root = workspace(
        "nominal",
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nleft = { path = \"../left\" }\nright = { path = \"../right\" }\n",
            ),
            (
                "app/src/main.disp",
                "use left.{Token as LeftToken}\nuse right.{Token as RightToken}\nfn main() { let a = LeftToken { value: 20 } let b = RightToken { value: 22 } print(a.value + b.value) }",
            ),
            (
                "left/DISP.toml",
                "[package]\nname = \"left\"\nversion = \"1.0.0\"\nentry = \"src/lib.disp\"\n",
            ),
            ("left/src/lib.disp", "pub struct Token { value: int }"),
            (
                "right/DISP.toml",
                "[package]\nname = \"right\"\nversion = \"1.0.0\"\nentry = \"src/lib.disp\"\n",
            ),
            ("right/src/lib.disp", "pub struct Token { value: int }"),
        ],
    );
    let app = root.join("app");
    package::write_lock(&app).unwrap();
    assert_eq!(run_path(&app).unwrap(), ["42"]);
    let (hir, mir) = disp::lower_path(&app).unwrap();
    assert!(hir.structs[0].name != hir.structs[1].name);
    assert!(
        mir.source_files
            .iter()
            .any(|source| source.path.to_string_lossy().contains("dependencies"))
    );
}

#[test]
fn dependency_aliases_cannot_shadow_local_modules() {
    let root = workspace(
        "alias-shadow",
        &[
            (
                "app/DISP.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nmath = { path = \"../dependency\" }\n",
            ),
            (
                "app/src/main.disp",
                "use math\nfn main() { print(value()) }",
            ),
            ("app/src/math.disp", "pub fn value() -> int = 1"),
            (
                "dependency/DISP.toml",
                "[package]\nname = \"dependency\"\nversion = \"1.0.0\"\nentry = \"src/lib.disp\"\n",
            ),
            ("dependency/src/lib.disp", "pub fn value() -> int = 2"),
        ],
    );
    let app = root.join("app");
    package::write_lock(&app).unwrap();
    let error = check_path(&app).unwrap_err();
    assert!(error.message.contains("ambiguous"));
}

#[test]
fn dependency_hashes_are_stable_across_crlf_and_lf_checkouts() {
    let root = basic_workspace("line-endings");
    let app = root.join("app");
    package::write_lock(&app).unwrap();
    let lock = fs::read(app.join("DISP.lock")).unwrap();
    let dependency = root.join("math/src/lib.disp");
    let source = fs::read_to_string(&dependency).unwrap();
    fs::write(&dependency, source.replace('\n', "\r\n")).unwrap();
    assert_eq!(
        package::render(&package::resolve(&app).unwrap())
            .unwrap()
            .as_bytes(),
        lock
    );
    check_path(&app).unwrap();
}
