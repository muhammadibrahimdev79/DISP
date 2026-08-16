use disp::{
    backend::{self, BuildOptions},
    check_source, lower_path, run_source,
};
use std::{fs, path::PathBuf, process::Command, thread, time::Duration};

fn reject(source: &str, expected: &[&str]) {
    let error = check_source(source).unwrap_err();
    assert!(
        expected
            .iter()
            .any(|fragment| error.message.contains(fragment)),
        "expected one of {expected:?} in: {}",
        error.message
    );
}

fn temporary_source(name: &str, source: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("disp-memory-safety-{}-{name}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("main.disp");
    fs::write(&path, source).unwrap();
    path
}

#[test]
fn uninitialized_storage_is_never_a_value_or_borrow_origin() {
    reject(
        "fn main() { let value: int print(value) }",
        &["initialized"],
    );
    reject(
        "fn main() { let value: int let view = &value print(*view) }",
        &["initialized"],
    );
    reject(
        "struct Pair { left: int, right: int } fn main() { let pair = Pair { left: 1 } }",
        &["missing field"],
    );
}

#[test]
fn safe_unions_are_tagged_and_untagged_storage_fails_closed() {
    let source = r#"
enum Number { Integer(int), Decimal(f64) }
fn main() {
    let value = Number.Integer(7)
    print(match value { Number.Integer(number) => number Number.Decimal(_) => 0 })
}
"#;
    assert_eq!(run_source(source).unwrap(), ["7"]);
    reject(
        "union Bits { integer: int, decimal: f64 } fn main() {}",
        &["reserved", "expected", "unknown"],
    );
}

#[test]
fn unavailable_memory_escape_hatches_are_rejected_by_name() {
    for name in ["MaybeUninit", "Pin", "Cell", "UnsafeCell"] {
        reject(
            &format!("fn main() {{ let value: {name}<int> }}"),
            &["unknown type", "unknown"],
        );
    }
}

#[test]
fn interior_mutability_is_explicit_and_shared_field_mutation_is_rejected() {
    reject(
        "struct Point { value: int } fn change(point: &Point) { point.value = 2 } fn main() {}",
        &["shared reference", "immutable", "mutable"],
    );
    let source = r#"
fn main() {
    let counter = AtomicInt.new(1)
    counter.add(2)
    print(counter.load())
    let guarded = Mutex.new(4)
    let lock = guarded.lock()
    *lock += 1
    print(*lock)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["3", "5"]);
}

#[test]
fn representative_safe_memory_program_passes_native_sanitizers() {
    let source = r#"
enum Payload { Number(uint), Text(String) }
struct Holder<T> { value: T }
fn read(value: &int) -> int {
    let holder = Holder { value }
    return *holder.value
}
fn main() {
    let payload = Payload.Text("safe")
    print(match payload { Payload.Number(number) => number Payload.Text(text) => text.len() })
    let number = 7
    print(read(&number))
}
"#;
    let path = temporary_source("sanitized", source);
    let (hir, mir) = lower_path(&path).unwrap();
    let artifact = match backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            sanitizers: true,
            ..BuildOptions::default()
        },
    ) {
        Ok(artifact) => artifact,
        Err(error)
            if error.message.contains("cannot find -lasan")
                || error.message.contains("cannot find -lubsan") =>
        {
            eprintln!(
                "native toolchain has no sanitizer runtime: {}",
                error.message
            );
            return;
        }
        Err(error) => panic!("sanitized native build failed: {error}"),
    };
    if cfg!(windows) {
        assert!(
            artifact
                .executable
                .parent()
                .unwrap()
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_str().is_some_and(|name| name
                    .starts_with("clang_rt.asan_dynamic-")
                    && name.ends_with(".dll"))),
            "sanitized Windows artifacts must stage the ASan runtime"
        );
    }
    let cached = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            sanitizers: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert!(
        cached.reused,
        "complete sanitized artifacts should be cached"
    );
    let mut output = None;
    for _ in 0..4 {
        match Command::new(&artifact.executable).output() {
            Ok(result) => {
                output = Some(result);
                break;
            }
            Err(error) if error.raw_os_error() == Some(4551) => {
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => panic!("could not execute sanitized DISP program: {error}"),
        }
    }
    let Some(output) = output else {
        eprintln!("Windows Application Control blocked the sanitized native executable");
        return;
    };
    assert!(
        output.status.success(),
        "sanitized program failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "4\n7\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("AddressSanitizer"), "{stderr}");
    assert!(!stderr.contains("runtime error:"), "{stderr}");
}
