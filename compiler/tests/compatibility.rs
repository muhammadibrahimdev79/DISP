use disp::{check_path, package, project, run_path};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PROJECT: AtomicUsize = AtomicUsize::new(0);

fn project_with(manifest: &str, source: &str) -> PathBuf {
    let id = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("disp-compatibility-{}-{id}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("DISP.toml"), manifest).unwrap();
    fs::write(root.join("src/main.disp"), source).unwrap();
    root
}

#[test]
fn legacy_and_explicit_edition_one_have_identical_behavior() {
    let legacy = project_with(
        "[package]\nname = \"legacy\"\nversion = \"1.0.0\"\n",
        "fn main() { print(42) }\n",
    );
    let explicit = project_with(
        "[package]\nname = \"explicit\"\nversion = \"1.0.0\"\nedition = \"1\"\nfeatures = []\n",
        "fn main() { print(42) }\n",
    );
    check_path(&legacy).unwrap();
    check_path(&explicit).unwrap();
    assert_eq!(run_path(&legacy).unwrap(), run_path(&explicit).unwrap());
}

#[test]
fn editions_and_features_fail_closed() {
    for (manifest, expected, line) in [
        (
            "[package]\nname = \"future\"\nversion = \"1.0.0\"\nedition = \"2\"\n",
            "unsupported DISP edition `2`",
            4,
        ),
        (
            "[package]\nname = \"preview\"\nversion = \"1.0.0\"\nedition = \"1\"\nfeatures = [\"page-preview\"]\n",
            "unsupported DISP feature `page-preview`",
            5,
        ),
        (
            "[package]\nname = \"duplicate\"\nversion = \"1.0.0\"\nfeatures = [\"future\", \"future\"]\n",
            "unique quoted names",
            4,
        ),
        (
            "[package]\nname = \"invalid\"\nversion = \"1.0.0\"\nfeatures = [\"Uppercase\"]\n",
            "unique quoted names",
            4,
        ),
    ] {
        let root = project_with(manifest, "fn main() {}\n");
        let error = check_path(&root).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert_eq!(error.span.start.line, line);
        assert!(error.file.unwrap().ends_with("DISP.toml"));
    }
}

#[test]
fn migration_pins_compatibility_without_rewriting_source() {
    let legacy =
        "[package]\r\nname = \"legacy\"\r\nversion = \"1.0.0\"\r\nentry = \"src/main.disp\"\r\n";
    let source = "fn main() { print(7) }\n";
    let root = project_with(legacy, source);
    let source_path = root.join("src/main.disp");
    let before_source = fs::read(&source_path).unwrap();

    let check = project::migrate(&root, true).unwrap();
    assert!(check.changed);
    assert_eq!(fs::read_to_string(root.join("DISP.toml")).unwrap(), legacy);

    let migrated = project::migrate(&root, false).unwrap();
    assert!(migrated.changed);
    assert_eq!(migrated.edition, "1");
    assert_eq!(fs::read(&source_path).unwrap(), before_source);
    assert_eq!(
        fs::read_to_string(root.join("DISP.toml")).unwrap(),
        "[package]\nname = \"legacy\"\nversion = \"1.0.0\"\nedition = \"1\"\nfeatures = []\nentry = \"src/main.disp\"\n"
    );

    let again = project::migrate(&root, false).unwrap();
    assert!(!again.changed);
    assert_eq!(run_path(&root).unwrap(), ["7"]);
}

#[test]
fn dependency_editions_are_isolated_and_locked() {
    let id = NEXT_PROJECT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "disp-compatibility-dependency-{}-{id}",
        std::process::id()
    ));
    let app = root.join("app");
    let dependency = root.join("dependency");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(
        app.join("DISP.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\nedition = \"1\"\nfeatures = []\n\n[dependencies]\nlegacy = { path = \"../dependency\" }\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.disp"),
        "use legacy\nfn main() { print(legacy_value()) }\n",
    )
    .unwrap();
    fs::write(
        dependency.join("DISP.toml"),
        "[package]\nname = \"legacy\"\nversion = \"1.0.0\"\nentry = \"src/lib.disp\"\n",
    )
    .unwrap();
    fs::write(
        dependency.join("src/lib.disp"),
        "pub fn legacy_value() -> int = 42\n",
    )
    .unwrap();

    package::write_lock(&app).unwrap();
    assert_eq!(run_path(&app).unwrap(), ["42"]);

    assert!(project::migrate(&dependency, false).unwrap().changed);
    let stale = check_path(&app).unwrap_err();
    assert!(
        stale
            .message
            .contains("lockfile does not match the package manifests")
    );
    package::write_lock(&app).unwrap();
    assert_eq!(run_path(&app).unwrap(), ["42"]);
}
