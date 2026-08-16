use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, hir, lower_source, run_source,
};
use std::{fs, process::Command, thread, time::Duration};

fn native(
    name: &str,
    source: &str,
    emit_c: bool,
) -> Option<(std::process::Output, Option<String>)> {
    let path = std::env::temp_dir().join(format!("disp-checked-pointer-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = artifact
        .backend_ir
        .map(|path| fs::read_to_string(path).unwrap());
    match Command::new(artifact.executable).output() {
        Ok(output) => Some((output, generated)),
        Err(error) if error.raw_os_error() == Some(4551) => None,
        Err(error) => panic!("native execution failed: {error}"),
    }
}

#[test]
fn memory_pointer_keeps_its_allocation_live() {
    let source = r#"
fn test() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(4, 8)?
    pointer = memory.as_ptr()
    moved = memory
    unsafe uses RawMemory { print(pointer.read()) }
    return Ok(0)
}
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("borrow"), "{}", error.message);
}

#[test]
fn const_and_mutable_memory_pointers_hold_the_correct_loans() {
    let shared = r#"
fn test() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(4, 8)?
    pointer = memory.as_ptr()
    memory.write(0, u8(1))
    unsafe uses RawMemory { print(pointer.read()) }
    return Ok(0)
}
fn main() {}
"#;
    let error = check_source(shared).unwrap_err();
    assert!(error.message.contains("borrow"), "{}", error.message);

    let exclusive = r#"
fn test() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(4, 8)?
    pointer = memory.as_mut_ptr()
    print(memory.read(0))
    unsafe uses RawMemory { pointer.write(u8(1)) }
    return Ok(0)
}
fn main() {}
"#;
    let error = check_source(exclusive).unwrap_err();
    assert!(error.message.contains("borrow"), "{}", error.message);
}

#[test]
fn pointer_offsets_preserve_provenance() {
    let source = r#"
fn test() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(4, 8)?
    unsafe uses RawMemory {
        shifted = memory.as_ptr().offset(1)
        moved = memory
        print(shifted.read())
    }
    return Ok(0)
}
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("borrow"), "{}", error.message);
}

#[test]
fn raw_pointer_provenance_propagates_through_calls() {
    let source = r#"
fn identity(value: MemoryPtr<u8>) -> MemoryPtr<u8> = value
fn test() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(4, 8)?
    pointer = identity(memory.as_ptr())
    moved = memory
    unsafe uses RawMemory { print(pointer.read()) }
    return Ok(0)
}
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("borrow"), "{}", error.message);
}

#[test]
fn local_memory_pointer_cannot_escape_by_return_or_assignment() {
    let returned = r#"
fn leak() -> Result<MemoryPtr<u8>, String> {
    memory = Memory.allocate(4, 8)?
    return Ok(memory.as_ptr())
}
fn main() {}
"#;
    let error = check_source(returned).unwrap_err();
    assert!(
        error.message.contains("cannot return") || error.message.contains("escape"),
        "{}",
        error.message
    );

    let assigned = r#"
fn test() -> Result<int, String> uses RawMemory {
    var pointer: MemoryPtr<u8>
    if true {
        memory = Memory.allocate(4, 8)?
        pointer = memory.as_ptr()
    } else {
        return Ok(0)
    }
    unsafe uses RawMemory { print(pointer.read()) }
    return Ok(0)
}
fn main() {}
"#;
    let error = check_source(assigned).unwrap_err();
    assert!(error.message.contains("escapes"), "{}", error.message);
}

#[test]
fn non_lexical_pointer_loans_end_after_last_use() {
    let source = r#"
fn test() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(4, 8)?
    pointer = memory.as_ptr()
    unsafe uses RawMemory { print(pointer.read()) }
    memory.write(0, u8(9))
    return Ok(0)
}
fn read_external(pointer: MemoryPtr<u8>) -> u8 uses RawMemory {
    unsafe uses RawMemory { return pointer.read() }
}
fn main() {}
"#;
    check_source(source).unwrap();
}

#[test]
fn checked_offsets_and_accesses_fail_before_native_undefined_behavior() {
    for (name, operation, expected) in [
        (
            "past-end-offset",
            "pointer.offset(5)",
            "offset is out of bounds",
        ),
        (
            "before-base-offset",
            "pointer.offset(-1)",
            "offset is out of bounds",
        ),
        (
            "one-past-read",
            "pointer.offset(4).read()",
            "access is out of bounds",
        ),
    ] {
        let source = format!(
            r#"
fn fail() -> Result<int, String> uses RawMemory {{
    memory = Memory.allocate(4, 8)?
    pointer = memory.as_ptr()
    unsafe uses RawMemory {{ {operation} }}
    return Ok(0)
}}
fn main() {{ match fail() {{ Ok(_) => print("bad") Err(error) => print(error) }} }}
"#
        );
        let interpreted = run_source(&source).unwrap_err();
        assert!(
            interpreted.message.contains(expected),
            "{}: {}",
            name,
            interpreted.message
        );
        if let Some((output, _)) = native(name, &source, false) {
            assert_eq!(output.status.code(), Some(101), "{name}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(expected),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn checked_memory_pointer_has_fat_layout_and_native_guards() {
    let (program, _) = lower_source("fn main() {}").unwrap();
    let target = Target::host().unwrap();
    let word = u64::from(target.pointer_width) / 8;
    let ty = hir::Type::MemoryPointer {
        mutable: true,
        inner: Box::new(hir::Type::Int {
            signed: false,
            width: Some(8),
        }),
    };
    let mut layouts = LayoutEngine::new(target, &program);
    let layout = layouts.layout(&ty).unwrap();
    assert_eq!((layout.size, layout.align), (word * 5, word));
    assert_eq!(abi::classify(&ty, &layout, target), abi::PassMode::Indirect);

    let source = r#"
fn exercise() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(2, 8)?
    unsafe uses RawMemory { print(memory.as_ptr().read()) }
    return Ok(0)
}
fn main() { match exercise() { Ok(_) => print("done") Err(error) => print(error) } }
"#;
    if let Some((output, generated)) = native("guard-layout", source, true) {
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .replace("\r\n", "\n")
                .trim(),
            "0\ndone"
        );
        let generated = generated.unwrap();
        assert!(generated.contains("disp_native_memory_pointer"));
        assert!(generated.contains("disp_memory_pointer_offset"));
        assert!(generated.contains("disp_memory_pointer_access"));
    }
}

#[test]
fn checked_memory_pointer_executes_under_native_sanitizers() {
    let source = r#"
fn exercise() -> Result<int, String> uses RawMemory {
    memory = Memory.allocate(2, 8)?
    unsafe uses RawMemory {
        pointer = memory.as_mut_ptr()
        pointer.write(u8(7))
        pointer.offset(1).write(u8(9))
        print(pointer.read())
        print(pointer.offset(1).read())
    }
    return Ok(0)
}
fn main() { print(match exercise() { Ok(value) => value Err(_) => -1 }) }
"#;
    let path = std::env::temp_dir().join("disp-checked-pointer-sanitized.disp");
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
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
        Err(error) => panic!("sanitized checked-pointer build failed: {error}"),
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
            Err(error) => panic!("could not execute sanitized checked-pointer program: {error}"),
        }
    }
    let Some(output) = output else {
        eprintln!("Windows Application Control blocked the sanitized native executable");
        return;
    };
    assert!(
        output.status.success(),
        "sanitized checked-pointer program failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "7\n9\n0\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("AddressSanitizer"), "{stderr}");
    assert!(!stderr.contains("runtime error:"), "{stderr}");
}
