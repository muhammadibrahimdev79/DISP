use disp::{check_source, effect_report_source};

#[test]
fn explicit_and_inferred_effect_contracts_are_checked_and_reported() {
    let source = r#"
fn load(path: Path) -> Result<String, IoError> uses FileSystem {
    return File.read_text(path)
}

fn inferred(path: Path) -> Result<String, IoError> {
    return load(path)
}

fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "load uses FileSystem\ninferred uses FileSystem [inferred]\nmain uses Pure\n"
    );

    let underdeclared = source.replace(
        "fn inferred(path: Path) -> Result<String, IoError> {",
        "fn inferred(path: Path) -> Result<String, IoError> uses Pure {",
    );
    let error = check_source(&underdeclared).unwrap_err();
    assert!(error.message.contains("requires capability `FileSystem`"));
    assert!(error.span.start.line > 0 && error.span.start.column > 0);
}

#[test]
fn filesystem_network_process_foreign_and_data_authority_are_detected() {
    let source = r#"
extern C { fn abs(value: CInt) -> CInt }

fn file(path: Path) -> Result<String, IoError> uses FileSystem = File.read_text(path)
fn resolve() -> Result<uint, NetworkError> uses Network = Ok(Dns.resolve("localhost")?.len())
fn process() -> Result<bool, IoError> uses Process = Ok(Process.run(Path("tool"), List.of("--version"))?.success())
fn foreign(value: CInt) -> CInt uses Foreign { unsafe { return abs(value) } }
fn store(path: Path) -> Result<DataStore, DataError> uses FileSystem = data open path
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    let report = effect_report_source(source).unwrap().render();
    for expected in [
        "abs uses Foreign",
        "file uses FileSystem",
        "resolve uses Network",
        "process uses Process",
        "foreign uses Foreign",
        "store uses FileSystem",
        "main uses Pure",
    ] {
        assert!(report.contains(expected), "missing {expected} in {report}");
    }
}

#[test]
fn unsafe_does_not_grant_foreign_authority() {
    let source = r#"
extern C { fn abs(value: CInt) -> CInt }
fn call(value: CInt) -> CInt uses Pure { unsafe { return abs(value) } }
fn main() uses Pure {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("requires capability `Foreign`"));
}

#[test]
fn capability_bearing_functions_and_closures_cannot_hide_in_pure_function_types() {
    let function_value = r#"
fn load(path: Path) -> Result<String, IoError> uses FileSystem = File.read_text(path)
fn main() { let operation = load }
"#;
    let error = check_source(function_value).unwrap_err();
    assert!(
        error.message.contains("cannot be erased"),
        "{}",
        error.message
    );

    let closure = r#"
fn main() {
    let operation = || -> Result<String, IoError> { return File.read_text(Path("value.txt")) }
}
"#;
    let error = check_source(closure).unwrap_err();
    assert!(error.message.contains("closure requires capabilities"));
}

#[test]
fn malformed_capability_contracts_fail_with_parser_spans() {
    for (source, message) in [
        ("fn main() uses Unknown {}", "unknown capability"),
        ("fn main() uses Network, Network {}", "duplicate capability"),
        (
            "fn main() uses Pure, Network {}",
            "`Pure` cannot be combined",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}

#[test]
fn secure_randomness_has_a_distinct_explicit_capability_identity() {
    let source = "fn entropy_source() uses Random {}\nfn main() uses Pure {}";
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "entropy_source uses Random\nmain uses Pure\n"
    );
}

#[test]
fn device_io_is_distinct_explicit_unsafe_authority_and_propagates() {
    let source = r#"
fn probe() -> u8 {
    unsafe uses DeviceIo { return Port.read_u8(u16(146)) }
}
fn wrapper() -> u8 { return probe() }
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "probe uses DeviceIo [inferred]\nwrapper uses DeviceIo [inferred]\nmain uses Pure\n"
    );
}

#[test]
fn mmio_uses_the_same_distinct_device_authority_and_propagates() {
    let source = r#"
fn probe() -> u32 {
    unsafe uses DeviceIo { return Mmio.read_u32(u16(24)) }
}
fn wrapper() -> u32 { return probe() }
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "probe uses DeviceIo [inferred]\nwrapper uses DeviceIo [inferred]\nmain uses Pure\n"
    );
}

#[test]
fn timer_ticks_have_distinct_authority_and_propagate() {
    let source = r#"
fn clock() -> u32 uses Timer = Time.ticks()
fn wrapper() -> u32 { return clock() }
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "clock uses Timer\nwrapper uses Timer [inferred]\nmain uses Pure\n"
    );

    let pure = source.replace(
        "fn clock() -> u32 uses Timer",
        "fn clock() -> u32 uses Pure",
    );
    let error = check_source(&pure).unwrap_err();
    assert!(error.message.contains("requires capability `Timer`"));
}

#[test]
fn deterministic_cryptography_remains_pure_while_entropy_is_random() {
    let source = r#"
fn derive(salt: List<u8>, input: SecretBytes, info: List<u8>) -> Result<SecretBytes, CryptoError> uses Pure {
    return Crypto.hkdf_sha256(salt, input, info, 32)
}
fn entropy() -> Result<SecretBytes, CryptoError> = Crypto.random_secret(32)
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    let report = effect_report_source(source).unwrap().render();
    assert!(report.contains("derive uses Pure"), "{report}");
    assert!(
        report.contains("entropy uses Random [inferred]"),
        "{report}"
    );
}

#[test]
fn timeout_network_acquisition_cannot_bypass_a_pure_contract() {
    for operation in [
        "Async.connect_timeout(SocketAddress(\"localhost\", 443), Duration.from_millis(1))",
        "Async.resolve_timeout(\"localhost\", Duration.from_millis(1))",
    ] {
        let source = format!(
            "async fn acquire() uses Pure {{ let future = {operation} }}\nasync fn main() uses Pure {{}}"
        );
        let error = check_source(&source).unwrap_err();
        assert!(
            error.message.contains("requires capability `Network`"),
            "{}",
            error.message
        );
    }
}
