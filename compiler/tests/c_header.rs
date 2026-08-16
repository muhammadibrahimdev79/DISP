use disp::{backend::c_header, check_source, lower_source};
use std::{fs, process::Command};

const SOURCE: &str = r#"
export C struct FixturePoint {
    x: i32,
    y: i32,
}
impl Copy for FixturePoint {}
extern C("fixture") {
    fn fixture_add(left: CInt, right: CInt) -> CInt
    fn fixture_count(text: CStr) -> CSize
    fn fixture_fill(output: mut ptr<u8>, length: CSize)
    fn fixture_peek(input: ptr<u8>) -> u8
    fn fixture_indirect(output: ptr<mut ptr<u8>>)
    fn fixture_shift(value: FixturePoint) -> FixturePoint
}
fn main() {}
"#;

#[test]
fn c_import_header_is_versioned_bounded_and_deterministic() {
    let (hir, _) = lower_source(SOURCE).unwrap();
    let first = c_header::generate(&hir).unwrap();
    let second = c_header::generate(&hir).unwrap();
    assert_eq!(first, second);
    for required in [
        "#ifndef DISP_C_ABI_V1_H",
        "#define DISP_C_ABI_VERSION 1u",
        "#if CHAR_BIT != 8",
        "extern \"C\" {",
        "_Static_assert(sizeof(bool) == 1",
        "typedef const uint8_t *disp_c_ptr_u8;",
        "typedef uint8_t *disp_c_mut_ptr_u8;",
        "typedef const disp_c_mut_ptr_u8 *disp_c_ptr_mp_u8;",
        "typedef struct disp_t_S0 disp_t_S0;",
        "typedef disp_t_S0 disp_c_FixturePoint;",
        "int32_t x;",
        "DISP_C_STATIC_ASSERT(offsetof(disp_c_FixturePoint, y) == 4",
        "DISP_C_STATIC_ASSERT(sizeof(disp_c_FixturePoint) == 8",
        "/* DISP library: fixture */",
        "int32_t fixture_add(int32_t arg1, int32_t arg2);",
        "uintptr_t fixture_count(const char * arg1);",
        "void fixture_fill(disp_c_mut_ptr_u8 arg1, uintptr_t arg2);",
        "uint8_t fixture_peek(disp_c_ptr_u8 arg1);",
        "void fixture_indirect(disp_c_ptr_mp_u8 arg1);",
        "disp_c_FixturePoint fixture_shift(disp_c_FixturePoint arg1);",
        "#endif /* DISP_C_ABI_V1_H */",
    ] {
        assert!(
            first.contains(required),
            "header lacks `{required}`\n{first}"
        );
    }
    assert!(!first.contains("String"));
}

#[test]
fn generated_header_compiles_as_c_and_cpp_and_is_written_transactionally() {
    let root = std::env::temp_dir().join(format!(
        "disp-c-header-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("unnamed")
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("interop.disp");
    fs::write(&source_path, SOURCE).unwrap();
    let (hir, _) = lower_source(SOURCE).unwrap();
    let header_path = c_header::write(&hir, &source_path).unwrap();
    assert_eq!(header_path, root.join("interop.h"));
    assert_eq!(
        fs::read_to_string(&header_path).unwrap(),
        c_header::generate(&hir).unwrap()
    );

    let provider = root.join("provider.c");
    fs::write(
        &provider,
        "#include \"interop.h\"\nint32_t fixture_add(int32_t a,int32_t b){return a+b;}\nuintptr_t fixture_count(const char *v){(void)v;return 0;}\nvoid fixture_fill(disp_c_mut_ptr_u8 p,uintptr_t n){if(n)p[0]=1;}\nuint8_t fixture_peek(disp_c_ptr_u8 p){return p[0];}\nvoid fixture_indirect(disp_c_ptr_mp_u8 p){(void)p;}\ndisp_c_FixturePoint fixture_shift(disp_c_FixturePoint value){value.x++;value.y++;return value;}\n",
    )
    .unwrap();
    let c = Command::new("gcc")
        .current_dir(&root)
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-c",
            "provider.c",
            "-o",
            "provider.o",
        ])
        .output()
        .unwrap();
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));

    let cpp = Command::new("g++")
        .current_dir(&root)
        .args([
            "-std=c++17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-x",
            "c++",
            "-c",
            "provider.c",
            "-o",
            "provider-cpp.o",
        ])
        .output()
        .unwrap();
    assert!(
        cpp.status.success(),
        "{}",
        String::from_utf8_lossy(&cpp.stderr)
    );

    let target_headers = root.join("target-headers");
    fs::create_dir_all(&target_headers).unwrap();
    for (name, contents) in [
        (
            "stdbool.h",
            "#define bool _Bool\n#define true 1\n#define false 0\n",
        ),
        (
            "stddef.h",
            "typedef __SIZE_TYPE__ size_t;\n#define offsetof(type, member) __builtin_offsetof(type, member)\n",
        ),
        (
            "stdint.h",
            "typedef __INT8_TYPE__ int8_t;\ntypedef __UINT8_TYPE__ uint8_t;\ntypedef __INT16_TYPE__ int16_t;\ntypedef __UINT16_TYPE__ uint16_t;\ntypedef __INT32_TYPE__ int32_t;\ntypedef __UINT32_TYPE__ uint32_t;\ntypedef __INT64_TYPE__ int64_t;\ntypedef __UINT64_TYPE__ uint64_t;\ntypedef __INTPTR_TYPE__ intptr_t;\ntypedef __UINTPTR_TYPE__ uintptr_t;\n",
        ),
        ("limits.h", "#define CHAR_BIT __CHAR_BIT__\n"),
    ] {
        fs::write(target_headers.join(name), contents).unwrap();
    }
    for (mode, output) in [("-m64", "provider-x86_64.s"), ("-m32", "provider-i686.s")] {
        let cross = Command::new("gcc")
            .current_dir(&root)
            .args([
                mode,
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic",
                "-nostdinc",
                "-Itarget-headers",
                "-S",
                "provider.c",
                "-o",
                output,
            ])
            .output()
            .unwrap();
        assert!(
            cross.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&cross.stderr)
        );
    }
    let x86_64 = fs::read_to_string(root.join("provider-x86_64.s")).unwrap();
    let i686 = fs::read_to_string(root.join("provider-i686.s")).unwrap();
    assert!(x86_64.contains("fixture_shift:"));
    assert!(i686.contains("fixture_shift:"));
    let x86_64_shift = x86_64.split_once("fixture_shift:").unwrap().1;
    let i686_shift = i686.split_once("_fixture_shift:").unwrap().1;
    assert!(
        x86_64_shift.contains("movq\t%rcx, 16(%rbp)")
            && x86_64_shift.contains("movq\t16(%rbp), %rax"),
        "Windows x86-64 must pass and return the eight-byte record in its integer registers"
    );
    assert!(
        i686_shift.contains("movl\t8(%ebp), %eax") && i686_shift.contains("movl\t12(%ebp), %edx"),
        "Windows i686 must pass the record on its stack and return it through EDX:EAX"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn header_generation_rejects_opaque_disp_layouts_in_raw_pointer_signatures() {
    let (hir, _) =
        lower_source("extern C { fn unstable(value: ptr<String>) -> CInt } fn main() {}").unwrap();
    let error = c_header::generate(&hir).unwrap_err();
    assert!(
        error
            .message
            .contains("no stable DISP C header representation")
    );
}

#[test]
fn exported_c_structs_reject_unstable_or_ambiguous_layouts() {
    for (source, expected) in [
        (
            "export C struct Empty {} fn main() {}",
            "must declare at least one field",
        ),
        (
            "export C struct Generic<T> { value: T } fn main() {}",
            "cannot be generic",
        ),
        (
            "export C struct Owned { value: String } fn main() {}",
            "has no stable value representation",
        ),
        (
            "struct Private { value: i32 } export C struct Public { value: Private } fn main() {}",
            "has no stable value representation",
        ),
        (
            "export C struct Header { class: i32 } fn main() {}",
            "field must use a safe",
        ),
        (
            "struct Private { value: i32 } extern C { fn consume(value: Private) } fn main() {}",
            "not safe to pass",
        ),
    ] {
        let diagnostic = check_source(source).unwrap_err();
        assert!(
            diagnostic.message.contains(expected),
            "unexpected diagnostic: {diagnostic}"
        );
    }
}
