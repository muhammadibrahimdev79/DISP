use disp::{ast::Capability, check_source, effect_report_source, hir, lower_source};

#[test]
fn legacy_unsafe_blocks_remain_source_compatible() {
    check_source(
        r#"
fn read(value: ptr<int>) -> int {
    unsafe { return *value }
}
fn main() {}
"#,
    )
    .unwrap();
}

#[test]
fn explicit_raw_memory_contract_is_retained_and_reported() {
    let source = r#"
fn read(value: ptr<int>) -> int uses RawMemory {
    unsafe uses RawMemory { return *value }
}
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "read uses RawMemory\nmain uses Pure\n"
    );

    let (program, _) = lower_source(source).unwrap();
    let read = program
        .functions
        .iter()
        .find(|function| function.name == "read")
        .unwrap();
    match &read.body.statements[0].kind {
        hir::StatementKind::Unsafe { capabilities, .. } => {
            assert_eq!(capabilities.as_deref(), Some(&[Capability::RawMemory][..]));
        }
        other => panic!("expected bounded unsafe statement, found {other:?}"),
    }
}

#[test]
fn explicit_contract_rejects_wrong_unsafe_authority() {
    let raw_memory = r#"
fn read(value: ptr<int>) -> int uses Foreign {
    unsafe uses Foreign { return *value }
}
fn main() {}
"#;
    let error = check_source(raw_memory).unwrap_err();
    assert!(
        error
            .message
            .contains("does not allow capability `RawMemory`")
    );

    let foreign = r#"
extern C { fn abs(value: CInt) -> CInt }
fn call(value: CInt) -> CInt uses RawMemory {
    unsafe uses RawMemory { return abs(value) }
}
fn main() {}
"#;
    let error = check_source(foreign).unwrap_err();
    assert!(
        error
            .message
            .contains("does not allow capability `Foreign`")
    );
}

#[test]
fn nested_blocks_cannot_widen_an_enclosing_contract() {
    let source = r#"
extern C { fn abs(value: CInt) -> CInt }
fn call(value: CInt) -> CInt uses RawMemory, Foreign {
    unsafe uses RawMemory {
        unsafe uses Foreign { return abs(value) }
    }
}
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(
        error
            .message
            .contains("does not allow capability `Foreign`")
    );
}

#[test]
fn unsafe_contracts_do_not_disable_type_or_effect_checks() {
    let bad_type = r#"
fn read(value: ptr<int>) -> int uses RawMemory {
    unsafe uses RawMemory { let invalid: int = "not an integer" return *value }
}
fn main() {}
"#;
    let error = check_source(bad_type).unwrap_err();
    assert!(
        error.message.contains("binding initializer"),
        "{}",
        error.message
    );

    let hidden_file_system = r#"
fn load(path: Path) -> Result<String, IoError> uses RawMemory {
    unsafe uses RawMemory { return File.read_text(path) }
}
fn main() {}
"#;
    let error = check_source(hidden_file_system).unwrap_err();
    assert!(error.message.contains("requires capability `FileSystem`"));
}

#[test]
fn raw_memory_effects_propagate_through_the_call_chain() {
    let source = r#"
fn raw(value: ptr<int>) -> int {
    unsafe uses RawMemory { return *value }
}
fn wrapper(value: ptr<int>) -> int {
    return raw(value)
}
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert_eq!(
        effect_report_source(source).unwrap().render(),
        "raw uses RawMemory [inferred]\nwrapper uses RawMemory [inferred]\nmain uses Pure\n"
    );

    let underdeclared = source.replace(
        "fn wrapper(value: ptr<int>) -> int {",
        "fn wrapper(value: ptr<int>) -> int uses Pure {",
    );
    let error = check_source(&underdeclared).unwrap_err();
    assert!(error.message.contains("requires capability `RawMemory`"));
}

#[test]
fn malformed_unsafe_contracts_fail_closed() {
    for (source, expected) in [
        ("fn main() { unsafe uses Unknown {} }", "unknown capability"),
        (
            "fn main() { unsafe uses RawMemory, RawMemory {} }",
            "duplicate capability",
        ),
        (
            "fn main() { unsafe uses Pure, RawMemory {} }",
            "`Pure` cannot be combined",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}
