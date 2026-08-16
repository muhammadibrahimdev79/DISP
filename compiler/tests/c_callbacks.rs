use disp::{
    backend::{self, BuildOptions, c_header},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

const PROGRAM: &str = r#"
extern C {
    fn abs(value: CInt) -> CInt
}
fn invoke(callback: CFunction<fn(CInt) -> CInt>, value: CInt) -> CInt uses Foreign {
    unsafe uses Foreign {
        return callback(value)
    }
}
fn main() uses Foreign {
    print(invoke(abs, -7))
}
"#;

#[test]
fn c_function_types_are_exact_and_require_explicit_foreign_authority() {
    check_source(PROGRAM).unwrap();

    let missing = check_source(
        "fn invoke(callback: CFunction<fn(CInt) -> CInt>, value: CInt) -> CInt uses Foreign { return callback(value) } fn main() {}",
    )
    .unwrap_err();
    assert!(missing.message.contains("unsafe"), "{missing}");

    let implicit = check_source(
        "fn invoke(callback: CFunction<fn(CInt) -> CInt>, value: CInt) -> CInt uses Foreign { unsafe { return callback(value) } } fn main() {}",
    )
    .unwrap_err();
    assert!(implicit.message.contains("explicit"), "{implicit}");

    let malformed =
        check_source("extern C { fn bad(callback: CFunction<CInt>) } fn main() {}").unwrap_err();
    assert!(
        malformed.message.contains("function-signature"),
        "{malformed}"
    );

    let owned =
        check_source("extern C { fn bad(callback: CFunction<fn(String) -> CInt>) } fn main() {}")
            .unwrap_err();
    assert!(owned.message.contains("not safe to pass"), "{owned}");
}

#[test]
fn imported_c_symbols_lower_to_thin_foreign_callable_values() {
    let (hir, mir) = lower_source(PROGRAM).unwrap();
    let invoke = hir
        .functions
        .iter()
        .find(|function| function.name == "invoke")
        .unwrap();
    assert!(matches!(
        invoke.locals[invoke.parameters[0].0].ty,
        disp::hir::Type::CFunction(_, _)
    ));
    let invoke_mir = &mir.functions[invoke.id.0];
    assert!(invoke_mir.blocks.iter().any(|block| matches!(
        block.terminator,
        disp::mir::Terminator::Call {
            target: disp::hir::CallTarget::ForeignCallable,
            ..
        }
    )));
}

#[test]
fn c_function_headers_define_exact_portable_callback_aliases() {
    let source = r#"
extern C {
    fn apply_callback(callback: CFunction<fn(CInt) -> CInt>, value: CInt) -> CInt
    fn visit_callback(callback: CFunction<fn(ptr<CInt>) -> Unit>)
}
fn main() {}
"#;
    let (hir, _) = lower_source(source).unwrap();
    let header = c_header::generate(&hir).unwrap();
    assert!(header.contains("typedef int32_t (*disp_c_fn_CFi32_i32)(int32_t);"));
    assert!(header.contains("int32_t apply_callback(disp_c_fn_CFi32_i32 arg1, int32_t arg2);"));
    let pointer = header
        .find("typedef const int32_t *disp_c_ptr_i32;")
        .unwrap();
    let callback = header
        .find("typedef void (*disp_c_fn_CFcp_i32_void)(disp_c_ptr_i32);")
        .unwrap();
    assert!(pointer < callback);

    let root = std::env::temp_dir().join(format!("disp-c-function-header-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let header_path = root.join("callbacks.h");
    fs::write(&header_path, header).unwrap();
    for (compiler, standard) in [("gcc", "c11"), ("g++", "c++17")] {
        let output = Command::new(compiler)
            .args([
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic",
                "-fsyntax-only",
                header_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn disp_invokes_an_imported_c_symbol_through_a_typed_callback() {
    assert_eq!(run_source(PROGRAM).unwrap(), ["7"]);
    let root = std::env::temp_dir().join(format!("disp-c-function-native-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("callback.disp");
    fs::write(&source_path, PROGRAM).unwrap();
    let (hir, mir) = lower_source(PROGRAM).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    assert!(generated.contains("typedef int32_t (*disp_c_fn_CFi32_i32)(int32_t);"));
    assert!(generated.contains("null C callback"));
    let execution = match Command::new(&artifacts.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("native callback fixture failed to launch: {error}"),
    };
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "7\n"
    );
    fs::remove_dir_all(root).unwrap();
}
