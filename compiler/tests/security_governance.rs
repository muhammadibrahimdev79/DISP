use std::{collections::BTreeSet, fs, path::Path};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler must be inside repository")
}

fn rust_files(directory: &Path, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
}

#[test]
fn threat_model_and_response_policy_are_concrete_and_release_blocking() {
    let root = repository_root();
    let policy = fs::read_to_string(root.join("SECURITY.md")).unwrap();
    for required in [
        "security/advisories/new",
        "Initial acknowledgement",
        "Critical",
        "Release blockers",
        "sandbox escape",
        "signature-verification bypass",
        "Suppressing a failing security gate",
    ] {
        assert!(
            policy.contains(required),
            "security policy lacks {required}"
        );
    }

    let model = fs::read_to_string(root.join("docs/security/THREAT_MODEL_0.1.md")).unwrap();
    for required in [
        "Protected assets",
        "Adversaries and assumptions",
        "Trust boundaries and controls",
        "Unsafe-code inventory",
        "Abuse cases and required outcomes",
        "Verification and review cadence",
        "independent review",
    ] {
        assert!(model.contains(required), "threat model lacks {required}");
    }
}

#[test]
fn compiler_unsafe_code_stays_inside_the_audited_boundary_inventory() {
    let root = repository_root();
    let compiler = root.join("compiler");
    let allowed = BTreeSet::from([
        "crypto-native/src/lib.rs".to_owned(),
        "src/data_store.rs".to_owned(),
        "src/interpreter.rs".to_owned(),
        "src/process_sandbox.rs".to_owned(),
        "src/sqlite_compat.rs".to_owned(),
    ]);
    let mut files = Vec::new();
    rust_files(&compiler.join("src"), &mut files);
    rust_files(&compiler.join("crypto-native/src"), &mut files);

    let mut actual = BTreeSet::new();
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        assert!(
            !source.contains("allow(unsafe_code)"),
            "{} suppresses unsafe review",
            path.display()
        );
        if source.contains("unsafe {")
            || source.contains("unsafe fn ")
            || source.contains("unsafe impl ")
            || source.contains("unsafe extern ")
        {
            assert!(
                source.contains("SAFETY:"),
                "{} has unsafe code without a safety rationale",
                path.display()
            );
            actual.insert(
                path.strip_prefix(&compiler)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    assert_eq!(actual, allowed);

    let model = fs::read_to_string(root.join("docs/security/THREAT_MODEL_0.1.md")).unwrap();
    for path in allowed {
        assert!(model.contains(&format!("compiler/{path}")));
    }
}

#[test]
fn independence_contract_quarantines_database_compatibility_debt() {
    let root = repository_root();
    let contract = fs::read_to_string(root.join("docs/INDEPENDENCE.md")).unwrap();
    for required in [
        "Platform boundary",
        "Bootstrap-only",
        "Optional connector",
        "DISP-owned core",
        "Universal support contract",
        "First-class connector",
        "Compatibility or migration",
        "not called supported merely because syntax exists",
        "reproducible fixed point",
        "no database server, SQLite library",
        "PostgreSQL passes first-class typed interoperability",
        "must never become a fallback implementation",
    ] {
        assert!(
            contract.contains(required),
            "independence contract lacks {required}"
        );
    }

    let manifest = fs::read_to_string(root.join("compiler/Cargo.toml")).unwrap();
    for forbidden_database_crate in ["rusqlite", "sqlx", "tokio-postgres", "mysql_async"] {
        assert!(
            !manifest.contains(forbidden_database_crate),
            "core bootstrap manifest directly depends on {forbidden_database_crate}"
        );
    }

    let compiler = root.join("compiler/src");
    let permitted_sqlite_boundary = BTreeSet::from([
        "backend/linker.rs".to_owned(),
        "backend/runtime.rs".to_owned(),
        "backend/typed_codegen.rs".to_owned(),
        "interpreter.rs".to_owned(),
        "lib.rs".to_owned(),
        "sqlite_compat.rs".to_owned(),
    ]);
    let mut files = Vec::new();
    rust_files(&compiler, &mut files);
    let mut actual_sqlite_boundary = BTreeSet::new();
    for path in files {
        let source = fs::read_to_string(&path).unwrap();
        if source.to_ascii_lowercase().contains("sqlite") {
            actual_sqlite_boundary.insert(
                path.strip_prefix(&compiler)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    assert_eq!(actual_sqlite_boundary, permitted_sqlite_boundary);

    let data_store = fs::read_to_string(compiler.join("data_store.rs")).unwrap();
    assert!(!data_store.to_ascii_lowercase().contains("sqlite"));

    let interpreter = fs::read_to_string(compiler.join("interpreter.rs")).unwrap();
    assert!(!interpreter.contains("link(name = \"winsqlite3\")"));
    assert!(!interpreter.contains("link(name = \"sqlite3\")"));
    assert!(interpreter.contains("load_sqlite_api()?"));
    let connector = fs::read_to_string(compiler.join("sqlite_compat.rs")).unwrap();
    for required in [
        "winsqlite3.dll",
        "libsqlite3.so.0",
        "libsqlite3.dylib",
        "SQLite compatibility connector is unavailable",
        "static SQLITE_API: OnceLock<Result<SqliteApi, String>>",
    ] {
        assert!(connector.contains(required));
    }
}
