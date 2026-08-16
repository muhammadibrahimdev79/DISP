use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, mono, target::Target},
    lower_source, run_source,
};
use std::{fs, path::PathBuf, process::Command};

fn try_run_native(path: &std::path::Path) -> Result<std::process::Output, std::io::Error> {
    let mut last = None;
    for _ in 0..4 {
        match Command::new(path).output() {
            Ok(output) => return Ok(output),
            Err(error) if error.raw_os_error() == Some(4551) => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap())
}

fn temp_source(name: &str, source: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("disp-native-tests-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}.disp"));
    fs::write(&path, source).unwrap();
    path
}

fn native_output(name: &str, source: &str) -> Option<(String, i32)> {
    let path = temp_source(name, source);
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c: true,
            emit_object: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert!(artifacts.executable.exists());
    let output = match try_run_native(&artifacts.executable) {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return None,
        Err(error) => panic!("could not execute native test program: {error}"),
    };
    Some((
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        output.status.code().unwrap_or(-1),
    ))
}

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let Some((actual, status)) = native_output(name, source) else {
        return;
    };
    assert_eq!(status, 0);
    assert_eq!(actual, expected);
}

#[test]
fn hosted_backend_rejects_privileged_port_io_before_codegen() {
    let source = r#"
fn main() {
    unsafe uses DeviceIo { Port.write_u8(u16(233), u8(80)) }
}
"#;
    let path = temp_source("hosted-port-io", source);
    let (hir, mir) = lower_source(source).unwrap();
    let interpreter_error = run_source(source).unwrap_err();
    assert!(
        interpreter_error
            .message
            .contains("cannot execute in the hosted interpreter")
    );
    let error = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap_err();
    assert!(
        error
            .message
            .contains("unavailable in hosted native processes")
    );
    assert!(
        error
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--freestanding32"))
    );
}

#[test]
fn hosted_engines_reject_privileged_mmio_before_execution_or_codegen() {
    let source = r#"
fn main() {
    unsafe uses DeviceIo { Mmio.write_u32(u16(0), u32(80)) }
}
"#;
    let path = temp_source("hosted-mmio", source);
    let (hir, mir) = lower_source(source).unwrap();
    let interpreter_error = run_source(source).unwrap_err();
    assert!(
        interpreter_error
            .message
            .contains("cannot execute in the hosted interpreter")
    );
    let error = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap_err();
    assert!(
        error
            .message
            .contains("unavailable in hosted native processes")
    );
    assert!(
        error
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--freestanding-aarch64"))
    );
}

#[test]
fn target_layouts_and_abi_are_concrete() {
    let source = "struct Mixed { byte: u8, wide: u64 } enum Maybe { None, Some(i32) } fn main() { let mixed = Mixed { byte: 1, wide: 2 } let maybe = Maybe.Some(3) print(mixed.wide) print(match maybe { Some(value) => value None => 0 }) }";
    let (hir, _) = lower_source(source).unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let mixed = layouts
        .layout(&disp::hir::Type::Struct(disp::hir::StructId(0), vec![]))
        .unwrap();
    assert_eq!(mixed.fields, [0, 8]);
    assert_eq!(mixed.size, 16);
    assert_eq!(
        layouts
            .layout(&disp::hir::Type::RawPointer {
                mutable: false,
                inner: Box::new(disp::hir::Type::Int {
                    signed: true,
                    width: Some(8),
                }),
            })
            .unwrap()
            .size,
        8
    );
    let maybe = layouts
        .layout(&disp::hir::Type::Enum(disp::hir::EnumId(0), vec![]))
        .unwrap();
    assert_eq!(maybe.discriminant_size, 1);
    assert!(maybe.payload_offset.unwrap() >= 1);
    let i64_ty = disp::hir::Type::Int {
        signed: true,
        width: Some(64),
    };
    assert_eq!(
        abi::classify(&i64_ty, &layouts.layout(&i64_ty).unwrap(), target),
        abi::PassMode::Direct
    );
    let mixed_ty = disp::hir::Type::Struct(disp::hir::StructId(0), vec![]);
    assert_eq!(
        abi::classify(&mixed_ty, &mixed, target),
        abi::PassMode::Indirect
    );

    let (_, mir) = lower_source(source).unwrap();
    let mono = mono::collect(&mir).unwrap();
    let declarations = disp::backend::native_types::generate(&hir, &mono, target).unwrap();
    assert!(declarations.contains("typedef struct disp_t_S0 disp_t_S0"));
    assert!(declarations.contains("offsetof(disp_t_S0,f1)==8"));
    assert!(declarations.contains(&format!("sizeof(disp_t_E0)=={}", maybe.size)));
    let path = temp_source("concrete_layout", source);
    assert!(
        backend::build(&hir, &mir, &path, BuildOptions::default())
            .unwrap()
            .executable
            .exists()
    );
}

#[test]
fn in_memory_programs_without_source_identity_never_share_native_cache_entries() {
    let first_source = "fn main() { print(1) }";
    let second_source = "fn main() { print(2) }";
    let path = temp_source("identityless_cache", first_source);
    let (first_hir, first_mir) = lower_source(first_source).unwrap();
    let first = backend::build(&first_hir, &first_mir, &path, BuildOptions::default()).unwrap();
    assert!(!first.reused);

    let (second_hir, second_mir) = lower_source(second_source).unwrap();
    let second = backend::build(&second_hir, &second_mir, &path, BuildOptions::default()).unwrap();
    assert!(!second.reused);
    let output = match try_run_native(&second.executable) {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("could not execute rebuilt in-memory program: {error}"),
    };
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "2");
}

#[test]
fn monomorphization_is_deterministic_and_deduplicated() {
    let source = "struct Box<T> { value: T } fn id<T>(value: T) -> T { return value } fn main() { let first = Box { value: id(1) } let second = Box { value: id(2) } print(first.value + second.value) }";
    let (_, mir) = lower_source(source).unwrap();
    let first = mono::collect(&mir).unwrap();
    let second = mono::collect(&mir).unwrap();
    assert_eq!(first.instances, second.instances);
    assert_eq!(first.instances.len(), 2);
    let symbols = first
        .instances
        .iter()
        .map(|instance| mono::mangle(&mir, instance))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(symbols.len(), first.instances.len());
    assert_eq!(first.types, second.types);
    assert_eq!(
        first
            .types
            .iter()
            .filter(|instance| matches!(instance.ty, disp::hir::Type::Struct(_, _)))
            .count(),
        1
    );
}

#[test]
fn scalar_codegen_uses_concrete_locals_and_function_abi() {
    let source =
        "fn double(value: i32) -> i32 { return value * 2 } fn main() { print(double(21)) }";
    let path = temp_source("typed_scalar_abi", source);
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c: true,
            emit_object: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifact.backend_ir.unwrap()).unwrap();
    let object = fs::read(artifact.object.unwrap()).unwrap();
    assert_eq!(
        &object[..2],
        &[0x64, 0x86],
        "expected an x86-64 COFF object"
    );
    assert!(generated.contains("static int32_t disp_f0_double(int32_t a1)"));
    assert!(generated.contains("int32_t l0=(int32_t){0}"));
    assert!(!generated.contains("DV l["));
}

#[test]
fn representative_types_have_concrete_codegen_representations() {
    let source = include_str!("../examples/static_types.disp");
    let (hir, mir) = lower_source(source).unwrap();
    let instances = mono::collect(&mir).unwrap();
    for instance in &instances.instances {
        let function = &mir.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        for local in &function.locals {
            let ty = disp::backend::layout::substitute(&local.ty, &substitutions);
            assert!(
                disp::backend::typed_codegen::supported(&mir, &ty),
                "unsupported concrete type in {}: {ty:?}",
                function.name
            );
        }
    }
    let target = Target::host().unwrap();
    let declarations = disp::backend::native_types::generate(&hir, &instances, target).unwrap();
    let abi = abi::lower(&hir, &mir, &instances, target).unwrap();
    let generated =
        disp::backend::typed_codegen::generate(&mir, &instances, &abi, &declarations, false)
            .unwrap()
            .expect("all representative types should use concrete codegen");
    assert!(generated.contains("static disp_t_Ri8_GConversionError disp_f1_narrow(int64_t a1)"));
    assert!(!generated.contains("DV l["));
}

#[test]
fn native_functions_control_flow_and_exact_numerics_match_interpreter() {
    differential(
        "control_numeric",
        r#"
fn factorial(value: int) -> int { if value <= 1 { return 1 } return value * factorial(value - 1) }
fn main() {
    var total = 0
    for i in 0..=5 { if i == 3 { continue } total += i }
    var count = 0
    while count < 2 { count += 1 }
    print(total)
    print(factorial(6))
    let byte: i8 = 127
    print(byte.wrapping_add(1))
    let unsigned: u8 = 255
    print(unsigned.saturating_add(1))
    print(byte.saturating_add(1))
    let negative: i8 = -127
    let minimum = negative - 1
    print(minimum.saturating_sub(1))
    print(byte.saturating_mul(2))
    print(minimum.saturating_mul(2))
    let huge: u128 = 340282366920938463463374607431768211455
    print(huge)
}
"#,
    );
}

#[test]
fn native_float_bool_char_and_string_values_match_interpreter() {
    differential(
        "scalar_values",
        r#"
fn main() {
    let left: f64 = 1.5
    let right: f64 = 2.25
    print(left + right)
    print(left < right && true)
    print('Z')
    print('界')
    print("native string")
    print("界A")
}
"#,
    );
}

#[test]
fn native_struct_enum_option_result_and_question_match_interpreter() {
    differential(
        "adt_result",
        r#"
struct User { id: int, name: String }
enum Status { Active, Disabled(String) }
fn find(valid: bool) -> Result<User, String> {
    if valid { return Ok(User { id: 7, name: "Ada" }) }
    return Err("missing")
}
fn name(valid: bool) -> Result<String, String> { let user = find(valid)? return Ok(user.name) }
fn main() {
    let status = Status.Disabled("maintenance")
    print(match status { Active => "active", Disabled(reason) => reason })
    print(name(true))
    print(name(false))
}
"#,
    );
}

#[test]
fn native_nested_patterns_preserve_order_and_payload_tests() {
    let source = r#"
enum Pair { Values(bool, bool) Empty }
enum Choice { Left(int) Right(int) Empty }
struct Point { x: int, y: int }
fn classify(value: Pair) -> int {
    return match value {
        Pair.Values(true, true) => 1 + 0
        Pair.Values(true, false) => 2 + 0
        Pair.Values(false, _) => 3 + 0
        Pair.Empty => 0 + 0
    }
}
fn label(value: Option<int>) -> String {
    return match value { Some(0) => "zero" Some(_) => "number" None => "none" }
}
fn guarded(value: Option<String>) -> String {
    return match value {
        Some(text) if text.starts_with("A") => text
        Some(text) => text
        None => "none"
    }
}
fn alternative(value: Choice) -> String {
    return match value {
        Choice.Left(number) | Choice.Right(number) if number > 10 => "large"
        Choice.Left(0 | 1) | Choice.Right(0 | 1) => "small"
        Choice.Left(_) | Choice.Right(_) => "other"
        Choice.Empty => "empty"
    }
}
fn signed_pattern(value: int) -> String {
    return match value { -2 | -1 => "negative" 0 => "zero" _ => "positive" }
}
fn main() {
    print(classify(Pair.Values(true, false)))
    print(classify(Pair.Values(false, true)))
    print(label(Some(0)))
    print(label(Some(9)))
    let point = Point { x: 4, y: 5 }
    print(match point { Point { x, y: _ } => x })
    print(guarded(Some("Beta")))
    print(guarded(Some("Ada")))
    print(alternative(Choice.Left(20)))
    print(alternative(Choice.Right(1)))
    print(signed_pattern(-1))
}
"#;
    differential("nested-patterns", source);
}

#[test]
fn native_generics_traits_references_and_moves_match_interpreter() {
    differential(
        "generic_trait",
        r#"
struct Box<T> { value: T }
trait Increment { fn get(&self) -> int fn add(&mut self) }
impl Increment for Box<int> {
    fn get(&self) -> int { return 8 }
    fn add(&mut self) { self.value += 1 }
}
fn identity<T>(value: T) -> T { return value }
fn main() {
    var boxed = Box { value: identity(8) }
    print(boxed.get())
    boxed.add()
    print(boxed.value)
    let text = "moved"
    let moved = text
    print(moved)
}
"#,
    );
}

#[test]
fn native_checked_overflow_has_controlled_failure() {
    let source = "fn main() { let seed: i8 = 126 let value = seed + 1 print(value + 1) }";
    let path = temp_source("overflow", source);
    let (hir, mir) = lower_source(source).unwrap();
    let interpreted = run_source(source).unwrap_err();
    assert!(interpreted.message.contains("overflow"));
    let executable = backend::build(&hir, &mir, &path, BuildOptions::default())
        .unwrap()
        .executable;
    let output = match try_run_native(&executable) {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("could not execute overflow program: {error}"),
    };
    assert_eq!(output.status.code(), Some(101));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("integer overflow")
    );
}

#[test]
fn native_match_payload_moves_drop_remaining_fields_exactly_once() {
    let source = r#"
enum Pair<T> { Both(T, T) One(T) }
enum Wrapped<T> { Value(Pair<T>, T) }
fn take(value: Wrapped<String>) -> String {
    return match value {
        Wrapped.Value(Pair.Both(first, _), _) => first
        Wrapped.Value(_, fallback) => fallback
    }
}
fn main() { print(take(Wrapped.Value(Pair.Both("kept", "inner drop"), "outer drop"))) }
"#;
    assert_eq!(run_source(source).unwrap(), ["kept"]);
    differential("variant-payload-cleanup", source);
}

#[test]
fn backend_rejects_unresolved_concrete_types_with_a_diagnostic() {
    let source = "fn main() {}";
    let (hir, _) = lower_source(source).unwrap();
    let mut layouts = LayoutEngine::new(Target::host().unwrap(), &hir);
    let error = layouts.layout(&disp::hir::Type::Unknown).unwrap_err();
    assert!(matches!(
        error.kind,
        disp::diagnostics::DiagnosticKind::Backend
    ));
    assert!(error.message.contains("type") || error.message.contains("layout"));
}

#[test]
fn representative_examples_match_the_interpreter_natively() {
    let mut executed = 0;
    for (name, source) in [
        (
            "control_flow_example",
            include_str!("../examples/control_flow.disp"),
        ),
        ("adts_example", include_str!("../examples/adts.disp")),
        (
            "static_types_example",
            include_str!("../examples/static_types.disp"),
        ),
        (
            "ownership_example",
            include_str!("../examples/ownership.disp"),
        ),
        (
            "concurrency_example",
            include_str!("../examples/concurrency.disp"),
        ),
        (
            "c_interop_example",
            include_str!("../examples/c_interop.disp"),
        ),
        (
            "system_memory_example",
            include_str!("../examples/system_memory.disp"),
        ),
        ("hello_example", include_str!("../examples/hello.disp")),
    ] {
        let expected = run_source(source).unwrap().join("\n") + "\n";
        let path = temp_source(name, source);
        let (hir, mir) = lower_source(source).unwrap();
        let executable = backend::build(&hir, &mir, &path, BuildOptions::default())
            .unwrap_or_else(|error| panic!("{name}: {error:?}"))
            .executable;
        match try_run_native(&executable) {
            Ok(output) => {
                executed += 1;
                assert_eq!(output.status.code(), Some(0), "{name}");
                assert_eq!(
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .replace("\r\n", "\n"),
                    expected,
                    "{name}"
                );
            }
            Err(error) if error.raw_os_error() == Some(4551) => {}
            Err(error) => panic!("could not execute {name}: {error}"),
        }
    }
    assert!(
        executed >= 2,
        "Application Control blocked too much native coverage"
    );
}
