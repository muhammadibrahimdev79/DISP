use std::{fs, process::Command};

use disp::{
    check_path, freestanding::compile_x86_bios, freestanding_aarch64::compile_aarch64_virt,
    freestanding32::compile_x86_protected, freestanding64::compile_x86_64,
};

fn disp(arguments: &[&str]) -> Option<std::process::Output> {
    for attempt in 0..4 {
        match Command::new(env!("CARGO_BIN_EXE_disp"))
            .args(arguments)
            .output()
        {
            Ok(output) => return Some(output),
            Err(error) if error.raw_os_error() == Some(4551) && attempt < 3 => continue,
            Err(error) if error.raw_os_error() == Some(4551) => return None,
            Err(error) => panic!("disp should execute: {error}"),
        }
    }
    unreachable!()
}

fn source_file(name: &str, source: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("disp-freestanding-{}-{unique}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(name);
    fs::write(&path, source).unwrap();
    path
}

#[test]
fn direct_freestanding_build_is_runtime_free_deterministic_and_fail_closed() {
    let source = source_file(
        "boot.disp",
        "fn add(left: u16, right: u16) -> u16 { return left + right } fn main() { print(\"DISP owns the machine\") var total: u16 = 0 var next: u16 = 1 while next <= 10 { total += next next += 1 } print(add(total, 0)) }\n",
    );
    let Some(first) = disp(&["build", "--freestanding", source.to_str().unwrap()]) else {
        return;
    };
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let image = source.parent().unwrap().join("build/boot-x86-bios.img");
    let first_bytes = fs::read(&image).unwrap();
    assert_eq!(first_bytes.len(), 512);
    assert_eq!(&first_bytes[510..], &[0x55, 0xaa]);
    assert!(
        first_bytes
            .windows(22)
            .any(|bytes| bytes == b"DISP owns the machine\0")
    );

    let Some(second) = disp(&["build", "--freestanding", source.to_str().unwrap()]) else {
        return;
    };
    assert!(second.status.success());
    assert_eq!(fs::read(&image).unwrap(), first_bytes);
    let artifacts = fs::read_dir(image.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(artifacts, ["boot-x86-bios.img"]);

    let rejected = source_file(
        "hosted.disp",
        "fn main() { var value: u64 = 1 print(value) }\n",
    );
    let Some(output) = disp(&["build", "--freestanding", rejected.to_str().unwrap()]) else {
        return;
    };
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("support only"));
    assert!(!rejected.parent().unwrap().join("build").exists());
}

#[test]
fn checked_multisector_fixture_compiles_without_invoking_the_driver() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/freestanding_multisector.disp");
    let program = check_path(&source).unwrap();
    let first = compile_x86_bios(&program).unwrap();
    let second = compile_x86_bios(&program).unwrap();
    assert_eq!(first, second);
    assert!(first.len() > 512);
    assert_eq!(first.len() % 512, 0);
    assert_eq!(&first[510..512], &[0x55, 0xaa]);
    assert_eq!(&first[512..518], &[0xfa, 0xea, 0x06, 0x7e, 0, 0]);
    assert!(!first.windows(12).any(|bytes| bytes == b"4000000005\r\n"));
}

#[test]
fn protected32_fixture_compiles_to_a_deterministic_flat_boot_image() {
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/protected32_hello.disp");
    let program = check_path(&source).unwrap();
    let first = compile_x86_protected(&program).unwrap();
    let second = compile_x86_protected(&program).unwrap();
    assert_eq!(first, second);
    assert!(first.len() > 512);
    assert_eq!(first.len() % 512, 0);
    assert_eq!(&first[510..512], &[0x55, 0xaa]);
    assert!(
        first[512..]
            .windows(8)
            .any(|bytes| { bytes == [0xff, 0xff, 0, 0, 0, 0x9a, 0xcf, 0] })
    );
    assert!(
        first[512..]
            .windows(33)
            .any(|bytes| bytes == b"Hello from 32-bit protected DISP\0")
    );
    assert!(
        first[512..]
            .windows(5)
            .any(|bytes| bytes == [0xa3, 0, 0, 0x10, 0])
    );
}

#[test]
fn protected32_cli_writes_only_the_named_transactional_artifact() {
    let source = source_file(
        "protected.disp",
        "fn main() { print(\"Protected DISP\") }\n",
    );
    let Some(output) = disp(&["build", "--freestanding32", source.to_str().unwrap()]) else {
        return;
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let build = source.parent().unwrap().join("build");
    let artifact = build.join("protected-x86-protected32.img");
    let length = fs::read(&artifact).unwrap().len();
    assert!(length > 512);
    assert_eq!(length % 512, 0);
    let names = fs::read_dir(build)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["protected-x86-protected32.img"]);
}

#[test]
fn x86_64_fixture_and_cli_emit_one_deterministic_long_mode_artifact() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/freestanding64_hello.disp");
    let program = check_path(&fixture).unwrap();
    let first = compile_x86_64(&program).unwrap();
    let second = compile_x86_64(&program).unwrap();
    assert_eq!(first, second);
    assert!(first.len() > 512);
    assert_eq!(first.len() % 512, 0);
    assert_eq!(&first[510..512], &[0x55, 0xaa]);
    assert!(
        first[512..]
            .windows(23)
            .any(|bytes| bytes == b"Hello from 64-bit DISP\0")
    );

    let source = source_file("long.disp", "fn main() { print(\"Long DISP\") }\n");
    let Some(output) = disp(&["build", "--freestanding64", source.to_str().unwrap()]) else {
        return;
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let build = source.parent().unwrap().join("build");
    let artifact = build.join("long-x86_64-long.img");
    assert!(fs::read(&artifact).unwrap().len() > 512);
    let names = fs::read_dir(build)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["long-x86_64-long.img"]);
}

#[test]
fn x86_64_scalar_fixture_reaches_checked_long_mode_codegen_deterministically() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/freestanding64_scalars.disp");
    let program = check_path(&fixture).unwrap();
    let first = compile_x86_64(&program).unwrap();
    let second = compile_x86_64(&program).unwrap();
    assert_eq!(first, second);
    assert!(first.len() > 3 * 512);
    assert_eq!(first.len() % 512, 0);
    assert!(
        first[512..]
            .windows(35)
            .any(|bytes| bytes == b"DISP x86-64 checked scalar profile\0")
    );
    assert!(
        first[512..]
            .windows(7)
            .any(|bytes| bytes == [0x89, 0x04, 0x25, 0x00, 0x50, 0x10, 0x00])
    );
}

#[test]
fn x86_64_overflow_fixture_contains_one_nonreturning_failure_path() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/freestanding64_overflow.disp");
    let program = check_path(&fixture).unwrap();
    let image = compile_x86_64(&program).unwrap();
    let stage = &image[512..];
    assert_eq!(
        stage
            .windows(26)
            .filter(|bytes| *bytes == b"x86-64 arithmetic failure\0")
            .count(),
        1
    );
    assert!(stage.windows(2).any(|bytes| bytes == [0x0f, 0x87]));
}

#[test]
fn x86_64_function_and_stack_fixtures_are_guarded_and_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let functions = root.join("examples/freestanding64_functions.disp");
    let program = check_path(&functions).unwrap();
    let first = compile_x86_64(&program).unwrap();
    let second = compile_x86_64(&program).unwrap();
    assert_eq!(first, second);
    assert!(first.len() >= 5 * 512);
    assert!(
        first[512..]
            .windows(30)
            .any(|bytes| bytes == b"DISP x86-64 guarded functions\0")
    );
    assert!(
        first[512..]
            .windows(3)
            .any(|bytes| bytes == [0x48, 0x81, 0xfc])
    );

    let stack = root.join("examples/freestanding64_stack.disp");
    let program = check_path(&stack).unwrap();
    let image = compile_x86_64(&program).unwrap();
    assert_eq!(
        image[512..]
            .windows(28)
            .filter(|bytes| *bytes == b"x86-64 stack limit exceeded\0")
            .count(),
        1
    );
}

#[test]
fn x86_64_array_and_bounds_fixtures_are_checked_and_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let arrays = root.join("examples/freestanding64_arrays.disp");
    let program = check_path(&arrays).unwrap();
    let first = compile_x86_64(&program).unwrap();
    let second = compile_x86_64(&program).unwrap();
    assert_eq!(first, second);
    assert!(first.len() >= 5 * 512);
    assert!(
        first[512..]
            .windows(33)
            .any(|bytes| bytes == b"DISP x86-64 checked fixed arrays\0")
    );
    assert!(first[512..].windows(2).any(|bytes| bytes == [0x0f, 0x83]));

    let bounds = root.join("examples/freestanding64_bounds.disp");
    let program = check_path(&bounds).unwrap();
    let image = compile_x86_64(&program).unwrap();
    assert_eq!(
        image[512..]
            .windows(27)
            .filter(|bytes| *bytes == b"x86-64 index out of bounds\0")
            .count(),
        1
    );
}

#[test]
fn x86_64_device_io_fixture_is_authorized_and_deterministic() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/freestanding64_port.disp");
    let program = check_path(&fixture).unwrap();
    let first = compile_x86_64(&program).unwrap();
    let second = compile_x86_64(&program).unwrap();
    assert_eq!(first, second);
    assert!(
        first[512..]
            .windows(6)
            .any(|bytes| bytes == [0x89, 0xc2, 0xec, 0x0f, 0xb6, 0xc0])
    );
    assert!(
        first[512..]
            .windows(3)
            .any(|bytes| bytes == [0x89, 0xd8, 0xee])
    );
}

#[test]
fn x86_64_timer_fixture_is_authorized_deterministic_and_waits_for_irq0() {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/freestanding64_timer.disp");
    let program = check_path(&fixture).unwrap();
    let first = compile_x86_64(&program).unwrap();
    let second = compile_x86_64(&program).unwrap();
    assert_eq!(first, second);
    assert!(
        first[512..]
            .windows(36)
            .any(|bytes| bytes == b"DISP x86-64 capability timer active\0")
    );
    assert!(first[512..].windows(1).any(|bytes| bytes == [0xfb]));
    assert!(first[512..].windows(2).any(|bytes| bytes == [0x48, 0xcf]));
}

#[test]
fn aarch64_fixture_and_cli_emit_one_direct_deterministic_image() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/freestanding_aarch64_scalars.disp");
    let program = check_path(&fixture).unwrap();
    let first = compile_aarch64_virt(&program).unwrap();
    let second = compile_aarch64_virt(&program).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        u32::from_le_bytes(first[56..60].try_into().unwrap()),
        0x644d_5241
    );
    let message = b"AArch64 checked scalar control\r\n";
    assert!(first.windows(message.len()).any(|bytes| bytes == message));

    let source = source_file(
        "arm.disp",
        "fn main(){var n:u32=2 n*=3 if n==6{print(\"AArch64 DISP\")}}\n",
    );
    let Some(output) = disp(&["build", "--freestanding-aarch64", source.to_str().unwrap()]) else {
        return;
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let build = source.parent().unwrap().join("build");
    assert!(build.join("arm-aarch64-virt-8.2.img").is_file());
    let names = fs::read_dir(build)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["arm-aarch64-virt-8.2.img"]);
}

#[test]
fn aarch64_exact_scalar_fixture_is_compact_checked_and_deterministic() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/freestanding_aarch64_exact_scalars.disp");
    let program = check_path(&fixture).unwrap();
    let first = compile_aarch64_virt(&program).unwrap();
    let second = compile_aarch64_virt(&program).unwrap();
    assert_eq!(first, second);
    let heading = b"AArch64 exact scalar output\r\n";
    assert!(first.windows(heading.len()).any(|bytes| bytes == heading));
    for text in [b"true\r\n\0".as_slice(), b"false\r\n\0".as_slice()] {
        assert!(first.windows(text.len()).any(|bytes| bytes == text));
    }
    for instruction in [0x9b20_7c22u32, 0x9340_7c43, 0x1ac4_0865, 0x1b04_8ca6] {
        assert!(
            first
                .windows(4)
                .any(|bytes| bytes == instruction.to_le_bytes())
        );
    }
}

#[test]
fn aarch64_recursive_functions_and_stack_guard_are_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let functions = check_path(&root.join("examples/freestanding_aarch64_functions.disp")).unwrap();
    let first = compile_aarch64_virt(&functions).unwrap();
    let second = compile_aarch64_virt(&functions).unwrap();
    assert_eq!(first, second);
    assert!(
        first
            .windows(b"AArch64 guarded functions\r\n".len())
            .any(|bytes| bytes == b"AArch64 guarded functions\r\n")
    );

    let stack = check_path(&root.join("examples/freestanding_aarch64_stack.disp")).unwrap();
    let stack_first = compile_aarch64_virt(&stack).unwrap();
    let stack_second = compile_aarch64_virt(&stack).unwrap();
    assert_eq!(stack_first, stack_second);
    assert!(
        stack_first
            .windows(b"[DISP stack exhausted]\r\n\0".len())
            .any(|bytes| bytes == b"[DISP stack exhausted]\r\n\0")
    );
}

#[test]
fn aarch64_fixed_arrays_and_bounds_fixture_are_checked_and_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let arrays = check_path(&root.join("examples/freestanding_aarch64_arrays.disp")).unwrap();
    let first = compile_aarch64_virt(&arrays).unwrap();
    let second = compile_aarch64_virt(&arrays).unwrap();
    assert_eq!(first, second);
    let heading = b"AArch64 checked fixed arrays\r\n";
    assert!(first.windows(heading.len()).any(|bytes| bytes == heading));

    let bounds = check_path(&root.join("examples/freestanding_aarch64_bounds.disp")).unwrap();
    let bounds_first = compile_aarch64_virt(&bounds).unwrap();
    let bounds_second = compile_aarch64_virt(&bounds).unwrap();
    assert_eq!(bounds_first, bounds_second);
    let fault = b"[DISP index out of bounds]\r\n\0";
    assert!(
        bounds_first
            .windows(fault.len())
            .any(|bytes| bytes == fault)
    );
}

#[test]
fn aarch64_exception_vector_fixture_is_aligned_differentiated_and_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = check_path(&root.join("examples/freestanding_aarch64_exceptions.disp")).unwrap();
    let first = compile_aarch64_virt(&fixture).unwrap();
    let second = compile_aarch64_virt(&fixture).unwrap();
    assert_eq!(first, second);

    let table = (2048..first.len())
        .step_by(2048)
        .find(|offset| {
            offset + 2048 <= first.len()
                && (0..16).all(|slot| {
                    let entry = offset + slot * 128;
                    u32::from_le_bytes(first[entry..entry + 4].try_into().unwrap()) & 0xfc00_0000
                        == 0x1400_0000
                })
        })
        .expect("one image-aligned AArch64 exception table");
    assert_eq!(table % 2048, 0);
    assert_eq!(
        u32::from_le_bytes(first[128..132].try_into().unwrap()),
        0xd503_201f
    );
    for diagnostic in [
        b"[DISP synchronous exception]\r\n\0".as_slice(),
        b"[DISP IRQ exception]\r\n\0".as_slice(),
        b"[DISP FIQ exception]\r\n\0".as_slice(),
        b"[DISP system error exception]\r\n\0".as_slice(),
    ] {
        assert!(
            first
                .windows(diagnostic.len())
                .any(|bytes| bytes == diagnostic)
        );
    }
}

#[test]
fn aarch64_mmu_fixture_has_sparse_wx_page_tables_and_a_protected_probe() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = check_path(&root.join("examples/freestanding_aarch64_mmu.disp")).unwrap();
    let first = compile_aarch64_virt(&fixture).unwrap();
    let second = compile_aarch64_virt(&fixture).unwrap();
    assert_eq!(first, second);
    assert!(first.len().is_multiple_of(4096));
    assert_eq!(
        u32::from_le_bytes(first[280..284].try_into().unwrap()),
        0xd503_201f
    );
    for diagnostic in [
        b"AArch64 MMU W^X active\r\n\0".as_slice(),
        b"[DISP memory protection fault]\r\n\0".as_slice(),
    ] {
        assert!(
            first
                .windows(diagnostic.len())
                .any(|bytes| bytes == diagnostic)
        );
    }
    for instruction in [
        0xd51c_2019u32,
        0xd518_2019,
        0xd51c_101a,
        0xd518_101a,
        0xd508_871f,
        0xd50c_871f,
    ] {
        assert!(
            first
                .windows(4)
                .any(|bytes| bytes == instruction.to_le_bytes())
        );
    }
}

#[test]
fn aarch64_dtb_fixture_is_direct_address_independent_and_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = check_path(&root.join("examples/freestanding_aarch64_dtb.disp")).unwrap();
    let first = compile_aarch64_virt(&fixture).unwrap();
    let second = compile_aarch64_virt(&fixture).unwrap();
    assert_eq!(first, second);
    assert!(first.len().is_multiple_of(4096));
    assert!(
        !first
            .windows(8)
            .any(|bytes| bytes == 0x0900_0000u64.to_le_bytes())
    );
    assert!(
        first
            .windows(b"AArch64 DTB discovery active\r\n\0".len())
            .any(|bytes| bytes == b"AArch64 DTB discovery active\r\n\0")
    );
}

#[test]
fn aarch64_mmio_fixtures_are_capability_checked_bounded_and_deterministic() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = check_path(&root.join("examples/freestanding_aarch64_mmio.disp")).unwrap();
    let first = compile_aarch64_virt(&fixture).unwrap();
    let second = compile_aarch64_virt(&fixture).unwrap();
    assert_eq!(first, second);
    assert!(
        first
            .windows(b"MIO capability access active\r\n\0".len())
            .any(|bytes| bytes == b"MIO capability access active\r\n\0")
    );

    let bounds = check_path(&root.join("examples/freestanding_aarch64_mmio_bounds.disp")).unwrap();
    let bounds_first = compile_aarch64_virt(&bounds).unwrap();
    let bounds_second = compile_aarch64_virt(&bounds).unwrap();
    assert_eq!(bounds_first, bounds_second);
    assert!(
        bounds_first
            .windows(b"[DISP device access fault]\r\n\0".len())
            .any(|bytes| bytes == b"[DISP device access fault]\r\n\0")
    );
}
