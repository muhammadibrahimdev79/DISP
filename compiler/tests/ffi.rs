use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn native(name: &str, source: &str) -> Option<std::process::Output> {
    let path = std::env::temp_dir().join(format!("disp-ffi-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    match Command::new(artifact.executable).output() {
        Ok(output) => Some(output),
        Err(error) if error.raw_os_error() == Some(4551) => None,
        Err(error) => panic!("native execution failed: {error}"),
    }
}

const PROGRAM: &str = r#"
extern C {
    fn abs(value: CInt) -> CInt
    fn strlen(value: CStr) -> CSize
}
extern C("m") {
    fn sqrt(value: CDouble) -> CDouble
}
fn identity(value: CStr) -> CStr { return value }
fn inspect(text: &str) -> Result<CSize, String> {
    owned = CString.new(*text)?
    view = owned.as_c_str()
    unsafe {
        return Ok(strlen(identity(view)))
    }
}
fn main() {
    text = "Hello from C"
    print(text)
    match inspect(&text) {
        Ok(length) => print(length),
        Err(message) => print(message),
    }
    unsafe {
        print(abs(-7))
        print(sqrt(81.0))
    }
}
"#;

#[test]
fn extern_c_scalars_and_cstrings_are_differential() {
    let expected = run_source(PROGRAM).unwrap().join("\n") + "\n";
    assert_eq!(expected, "Hello from C\n12\n7\n9\n");
    if let Some(output) = native("standard-c", PROGRAM) {
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
fn cstring_and_cstr_methods_are_zero_copy_views_with_owned_conversion() {
    let source = r#"
fn show(value: CString) {
    view = value.as_c_str()
    print(value.len())
    print(value.is_empty())
    print(view.len())
    print(view.is_empty())
    print(view.to_string())
}
fn main() {
    match CString.new("") { Ok(value) => show(value), Err(error) => print(error) }
    match CString.new("UTF-8 ✓") { Ok(value) => show(value), Err(error) => print(error) }
}
"#;
    let expected = "0\ntrue\n0\ntrue\n\n9\nfalse\n9\nfalse\nUTF-8 ✓\n";
    assert_eq!(run_source(source).unwrap().join("\n") + "\n", expected);
    if let Some(output) = native("cstring-methods", source) {
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
fn cstring_rejects_interior_nul_and_owns_its_storage() {
    let source = r#"
fn make(text: String) -> Result<CString, String> { return CString.new(text) }
fn main() {
    match make("safe") { Ok(value) => print(value.to_string()), Err(error) => print(error) }
    match make("bad\0value") { Ok(value) => print(value.to_string()), Err(error) => print(error) }
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["safe", "CString source contains an interior NUL byte"]
    );
    if let Some(output) = native("cstring-validation", source) {
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout)
                .unwrap()
                .replace("\r\n", "\n"),
            "safe\nCString source contains an interior NUL byte\n"
        );
    }
}

#[test]
fn ffi_calls_require_unsafe_and_signatures_reject_owned_or_borrowed_abi_values() {
    let call_source = "extern C { fn abs(value: CInt) -> CInt } fn main() { print(abs(-1)) }";
    let call = check_source(call_source).unwrap_err();
    assert!(call.message.contains("requires an `unsafe` block"));
    assert_eq!(call.span.start.line, 1);
    assert_eq!(
        call.span.start.column,
        call_source.rfind("abs(-1)").unwrap() + 1
    );
    assert_eq!(call.span.end.column, call.span.start.column + 7);

    let owned =
        check_source("extern C { fn consume(value: String) -> CInt } fn main() {}").unwrap_err();
    assert!(owned.message.contains("not safe to pass"));

    let borrowed = check_source("extern C { fn borrowed() -> CStr } fn main() {}").unwrap_err();
    assert!(borrowed.message.contains("not safe to return"));
}

#[test]
fn external_declarations_reject_bodies_generics_bad_abis_and_library_injection() {
    let body = check_source("extern C { fn bad() { } } fn main() {}").unwrap_err();
    assert!(body.message.contains("cannot have a DISP body"));

    let generic = check_source("extern C { fn bad<T>(value: T) } fn main() {}").unwrap_err();
    assert!(generic.message.contains("cannot be generic"));

    let abi = check_source("extern Rust { fn bad() } fn main() {}").unwrap_err();
    assert!(abi.message.contains("unsupported external ABI"));

    let library = check_source("extern C(\"../evil\") { fn bad() } fn main() {}").unwrap_err();
    assert!(library.message.contains("library names"));

    let reserved = check_source("extern C { fn volatile() } fn main() {}").unwrap_err();
    assert!(reserved.message.contains("non-reserved"));
}

#[test]
fn cstr_views_block_owner_moves_and_cannot_escape_or_cross_threads() {
    check_source("fn keep(value: CStr) -> Result<CStr, String> { return Ok(value) } fn main() {}")
        .unwrap();

    let transitive = check_source(
        "fn keep(value: CStr) -> Result<CStr, String> { return Ok(value) } fn test() -> Result<CSize, String> { owner = CString.new(\"safe\")? held = keep(owner.as_c_str())? moved = owner return Ok(held.len()) } fn main() {}",
    )
    .unwrap_err();
    assert!(transitive.message.contains("borrow") || transitive.message.contains("overlap"));

    let moved = check_source(
        "fn test() -> Result<CInt, String> { value = CString.new(\"safe\")? view = value.as_c_str() other = value print(view.len()) return Ok(0) } fn main() {}",
    )
    .unwrap_err();
    assert!(moved.message.contains("borrow") || moved.message.contains("overlap"));

    let escape =
        check_source("fn bad(value: CString) -> CStr { return value.as_c_str() } fn main() {}")
            .unwrap_err();
    assert!(escape.message.contains("reference") || escape.message.contains("borrow"));

    let nested_escape = check_source(
        "fn bad() -> Result<CStr, String> { value = CString.new(\"safe\")? return Ok(value.as_c_str()) } fn main() {}",
    )
    .unwrap_err();
    assert!(nested_escape.message.contains("reference"));

    let send = check_source(
        "fn take(value: CStr) -> CSize { return value.len() } fn test() -> Result<CSize, String> { value = CString.new(\"safe\")? view = value.as_c_str() task = spawn take(view) return Ok(task.join()) } fn main() {}",
    )
    .unwrap_err();
    assert!(
        send.message.contains("thread boundary"),
        "unexpected diagnostic: {}",
        send.message
    );
}

#[test]
fn external_metadata_reaches_hir_mir_and_defined_native_prototypes() {
    let source = r#"
extern C {
    fn srand(seed: CUInt)
    fn rand() -> CInt
}
extern C("m") { fn sqrt(value: CDouble) -> CDouble }
extern C("unused_missing_library") { fn never_called() }
fn main() {
    unsafe {
        srand(1)
        rand()
        print(sqrt(16.0))
    }
}
"#;
    let (hir, mir) = lower_source(source).unwrap();
    let hir_sqrt = hir
        .functions
        .iter()
        .find(|function| function.name == "sqrt")
        .unwrap();
    let external = hir_sqrt.external.as_ref().unwrap();
    assert_eq!(external.abi, disp::hir::ExternalAbi::C);
    assert_eq!(external.library.as_deref(), Some("m"));
    let mir_sqrt = mir
        .functions
        .iter()
        .find(|function| function.name == "sqrt")
        .unwrap();
    assert_eq!(mir_sqrt.external.as_ref(), Some(external));

    let path = std::env::temp_dir().join("disp-ffi-defined-abi.disp");
    fs::write(&path, source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    assert!(generated.contains("extern void srand(uint32_t a1);"));
    assert!(generated.contains("extern int32_t rand(void);"));
    assert!(generated.contains("extern double sqrt(double a1);"));
    assert!(!generated.contains("static double sqrt("));

    match Command::new(artifacts.executable).output() {
        Ok(output) => {
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .replace("\r\n", "\n"),
                "4\n"
            );
        }
        Err(error) if error.raw_os_error() == Some(4551) => {}
        Err(error) => panic!("native execution failed: {error}"),
    }
}

#[test]
fn cstring_and_cstr_have_defined_native_layout_and_abi() {
    let (hir, _) = lower_source("fn main() {}").unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let owned = layouts.layout(&disp::hir::Type::CString).unwrap();
    let borrowed = layouts.layout(&disp::hir::Type::CStr).unwrap();
    assert_eq!((owned.size, owned.align), (24, 8));
    assert_eq!((borrowed.size, borrowed.align), (8, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::CStr, &borrowed, target),
        abi::PassMode::Direct
    );
}

#[test]
fn unknown_external_function_has_a_controlled_interpreter_diagnostic() {
    let error = run_source(
        "extern C { fn custom(value: CInt) -> CInt } fn main() { unsafe { print(custom(1)) } }",
    )
    .unwrap_err();
    assert!(error.message.contains("requires native execution"));
}
