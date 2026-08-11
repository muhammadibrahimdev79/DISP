use disp::{check_source, run_source};

#[test]
fn exact_width_checked_wrapping_and_saturating_arithmetic_execute() {
    let source = r#"
fn main() {
    let a: i8 = 127
    print(a.wrapping_add(1))
    print(a.saturating_add(1))
    let b: u8 = 0
    print(b.wrapping_sub(1))
    print(b.saturating_sub(1))
    let c: f32 = 1.5
    print(c + 0.5)
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["-128", "127", "255", "0", "2"]
    );
}

#[test]
fn exact_width_checked_arithmetic_reports_overflow() {
    let source = "fn main() { let a: i8 = 127 print(a + 1) }";
    let error = run_source(source).unwrap_err();
    assert!(error.message.contains("i8 overflow"));
    assert!(error.span.start.line > 0 && error.span.start.column > 0);
}

#[test]
fn widening_is_implicit_but_narrowing_is_explicit_and_checked() {
    let valid = r#"
fn widen(value: i16) -> i16 { return value }
fn main() { let small: i8 = 12 print(widen(small)) print(i8(12)) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["12", "12"]);

    assert!(check_source("fn take(x: i8) {} fn main() { let x: i16 = 1 take(x) }").is_err());
    assert!(check_source("fn main() { let x: u8 = -1 }").is_err());
    assert!(
        run_source("fn main() { print(i8(128)) }")
            .unwrap_err()
            .message
            .contains("outside i8 range")
    );
}

#[test]
fn binary_operations_choose_the_lossless_common_numeric_type() {
    let source = r#"
fn main() {
    let small: i8 = 100
    let wide: i16 = 1000
    print(small + wide)
    print(wide + small)
    let byte: u8 = 200
    let signed: i16 = 1000
    print(byte + signed)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["1100", "1100", "1200"]);
}

#[test]
fn checked_conversion_integrates_with_result_and_question() {
    let source = r#"
fn narrow(value: int) -> Result<i8, ConversionError> {
    return Ok(i8.try_from(value)?)
}
fn show(value: int) -> int {
    return match narrow(value) { Ok(number) => int(number), Err(_) => -1 }
}
fn main() { print(show(42)) print(show(500)) }
"#;
    assert_eq!(run_source(source).unwrap(), ["42", "-1"]);
}

#[test]
fn i128_and_u128_literals_preserve_full_width() {
    let source = r#"
fn main() {
    let signed: i128 = 170141183460469231731687303715884105727
    let minimum: i128 = -170141183460469231731687303715884105728
    let unsigned: u128 = 340282366920938463463374607431768211455
    print(signed)
    print(minimum)
    print(unsigned)
    print(unsigned.wrapping_add(1))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "170141183460469231731687303715884105727",
            "-170141183460469231731687303715884105728",
            "340282366920938463463374607431768211455",
            "0",
        ]
    );
}

#[test]
fn illegal_numeric_conversions_and_coercions_fail() {
    let cases = [
        "fn main() { let x: i8 = 128 }",
        "fn main() { let x: u8 = -1 }",
        "fn take(x: u16) {} fn main() { let x: i8 = 1 take(x) }",
        "fn main() { print(i8(true)) }",
        "fn main() { let x = 340282366920938463463374607431768211455 }",
    ];
    for source in cases {
        let error = check_source(source).unwrap_err();
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}
