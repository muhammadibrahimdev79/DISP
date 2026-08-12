use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn native(
    name: &str,
    source: &str,
    emit_c: bool,
) -> Option<(std::process::Output, Option<String>)> {
    let path = std::env::temp_dir().join(format!("disp-memory-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = artifacts
        .backend_ir
        .map(|path| fs::read_to_string(path).unwrap());
    for _ in 0..4 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => return Some((output, generated)),
            Err(error) if error.raw_os_error() == Some(4551) => {}
            Err(error) => panic!("native execution failed: {error}"),
        }
    }
    None
}

const PROGRAM: &str = r#"
fn exercise() -> Result<int, String> {
    memory = Memory.allocate(8, 16)?
    print(memory.len())
    print(memory.alignment())
    print(memory.is_empty())
    print(memory.read(0))
    memory.write(0, u8(7))
    print(memory.read(0))
    memory.fill(u8(3))

    source = Memory.allocate(4, 8)?
    source.write(0, u8(10))
    source.write(1, u8(11))
    source.write(2, u8(12))
    source.write(3, u8(13))
    memory.copy_from(2, source, 1, 3)
    print(memory.read(2))
    print(memory.read(4))

    unsafe {
        pointer = memory.as_mut_ptr()
        pointer.write(u8(9))
        pointer.offset(1).write(u8(8))
        print(pointer.read())
        print(pointer.offset(2).read())
    }
    memory.copy_from(1, memory, 0, 4)
    print(memory.read(1))
    print(memory.read(4))
    return Ok(0)
}

fn main() {
    match exercise() {
        Ok(_) => print("done"),
        Err(error) => print(error),
    }
}
"#;

#[test]
fn aligned_memory_safe_operations_and_raw_pointers_are_differential() {
    let expected = "8\n16\nfalse\n0\n7\n11\n13\n9\n11\n9\n12\ndone\n";
    assert_eq!(run_source(PROGRAM).unwrap().join("\n") + "\n", expected);
    if let Some((output, _)) = native("operations", PROGRAM, false) {
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
}

#[test]
fn allocation_validation_is_recoverable_and_differential() {
    let source = r#"
fn show(size: uint, alignment: uint) {
    match Memory.allocate(size, alignment) {
        Ok(memory) => print(memory.len()),
        Err(error) => print(error),
    }
}
fn main() {
    show(4, 0)
    show(4, 3)
    show(4, 2097152)
    show(18446744073709551615, 8)
    show(0, 64)
}
"#;
    let expected = concat!(
        "Memory alignment must be a non-zero power of two\n",
        "Memory alignment must be a non-zero power of two\n",
        "Memory alignment exceeds the supported maximum\n",
        "Memory size overflow\n",
        "0\n"
    );
    assert_eq!(run_source(source).unwrap().join("\n") + "\n", expected);
    if let Some((output, _)) = native("validation", source, false) {
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .replace("\r\n", "\n"),
            expected
        );
    }
}

#[test]
fn memory_bounds_failures_are_controlled_in_both_engines() {
    let source = r#"
fn fail() -> Result<int, String> {
    memory = Memory.allocate(4, 8)?
    print(memory.read(4))
    return Ok(0)
}
fn main() { match fail() { Ok(_) => print("bad"), Err(error) => print(error) } }
"#;
    let interpreted = run_source(source).unwrap_err();
    assert!(interpreted.message.contains("out of bounds"));
    assert_eq!(interpreted.span.start.line, 4);
    if let Some((output, _)) = native("bounds", source, false) {
        assert_eq!(output.status.code(), Some(101));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Memory index out of bounds"), "{stderr}");
        assert!(stderr.contains("4:"), "{stderr}");
    }
}

#[test]
fn memory_and_pointer_misuse_is_rejected_with_source_spans() {
    let outside = check_source(
        "fn test()->Result<int,String>{ memory=Memory.allocate(1,8)? pointer=memory.as_ptr() return Ok(int(pointer.read())) } fn main(){}",
    )
    .unwrap_err();
    assert!(outside.message.contains("requires an `unsafe` block"));
    assert_eq!(outside.span.start.line, 1);

    let immutable = check_source(
        "fn test()->Result<int,String>{ let memory=Memory.allocate(1,8)? pointer=memory.as_mut_ptr() return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(
        immutable.message.contains("immutable") || immutable.message.contains("mutable"),
        "{}",
        immutable.message
    );

    let const_write = check_source(
        "fn test()->Result<int,String>{ memory=Memory.allocate(1,8)? pointer=memory.as_ptr() unsafe { pointer.write(u8(1)) } return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(const_write.message.contains("const raw pointer"));

    let moved = check_source(
        "fn test()->Result<int,String>{ memory=Memory.allocate(1,8)? other=memory print(memory.len()) return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"));

    let thread = check_source(
        "fn read(value: ptr<u8>)->u8 { unsafe { return value.read() } } fn test()->Result<int,String>{ memory=Memory.allocate(1,8)? pointer=memory.as_ptr() task=spawn read(pointer) return Ok(int(task.join())) } fn main(){}",
    )
    .unwrap_err();
    assert!(thread.message.contains("raw pointers") || thread.message.contains("thread"));
}

#[test]
fn memory_argument_types_are_checked_before_runtime() {
    let size = check_source("fn main() { value = Memory.allocate(\"large\", 8) }").unwrap_err();
    assert!(size.message.contains("size must be an integer"));

    let byte = check_source(
        "fn test()->Result<int,String>{ memory=Memory.allocate(1,8)? memory.write(0, u16(1)) return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(byte.message.contains("Memory byte"));

    let source = check_source(
        "fn test()->Result<int,String>{ memory=Memory.allocate(1,8)? memory.copy_from(0, \"bad\", 0, 1) return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(source.message.contains("Memory copy source"));

    let ffi = check_source("extern C { fn consume(value: Memory) } fn main() {}").unwrap_err();
    assert!(ffi.message.contains("not safe to pass"));
}

#[test]
fn memory_has_concrete_layout_abi_and_deterministic_native_drop() {
    let (hir, _) = lower_source("fn main() {}").unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let layout = layouts.layout(&disp::hir::Type::Memory).unwrap();
    assert_eq!((layout.size, layout.align), (24, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::Memory, &layout, target),
        abi::PassMode::Indirect
    );

    if let Some((output, generated)) = native("cleanup", PROGRAM, true) {
        assert!(output.status.success());
        let generated = generated.unwrap();
        assert!(generated.contains("disp_native_memory"));
        assert!(generated.contains("disp_memory_drop"));
        assert!(generated.contains("disp_alloc_zeroed"));
    }
}
