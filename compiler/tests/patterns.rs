use disp::{check_source, run_source};

#[test]
fn nested_finite_patterns_are_proven_exhaustive() {
    let source = r#"
enum Pair { Values(bool, bool) Empty }
fn classify(value: Pair) -> int {
    return match value {
        Pair.Values(true, true) => 1 + 0
        Pair.Values(true, false) => 2 + 0
        Pair.Values(false, _) => 3 + 0
        Pair.Empty => 0 + 0
    }
}
fn main() {
    print(classify(Pair.Values(true, false)))
    print(classify(Pair.Values(false, true)))
    print(classify(Pair.Empty))
}
"#;
    assert_eq!(run_source(source).unwrap(), ["2", "3", "0"]);
}

#[test]
fn nested_missing_cases_are_rejected() {
    let source = r#"
fn classify(value: Option<bool>) -> int {
    return match value { Some(true) => 1 + 0 None => 0 + 0 }
}
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("non-exhaustive match"));
    assert!(error.message.contains("Some"));
}

#[test]
fn redundant_nested_and_literal_patterns_are_rejected() {
    for source in [
        r#"
enum Pair { Values(bool, bool) Empty }
fn classify(value: Pair) -> int {
    return match value {
        Pair.Values(true, _) => 1
        Pair.Values(true, false) => 2
        _ => 0
    }
}
fn main() {}
"#,
        r#"
fn classify(value: int) -> int {
    return match value { 1 => 1 1 => 2 _ => 0 }
}
fn main() {}
"#,
    ] {
        let error = check_source(source).unwrap_err();
        assert!(
            error.message.contains("unreachable match arm"),
            "{}",
            error.message
        );
    }
}

#[test]
fn match_arm_numeric_literals_join_to_a_common_type() {
    let source = r#"
fn classify(value: bool) -> int { return match value { true => 1 false => 2 } }
fn main() { print(classify(false)) }
"#;
    assert_eq!(run_source(source).unwrap(), ["2"]);
}

#[test]
fn struct_patterns_destructure_fields_and_support_explicit_rest() {
    let source = r#"
struct Person { name: String, age: int, active: bool }
fn label(person: Person) -> String {
    return match person {
        Person { age: 0, .. } => "new"
        Person { name, age: _, active: _ } => name
    }
}
fn main() {
    print(label(Person { name: "Ada", age: 36, active: true }))
    print(label(Person { name: "Baby", age: 0, active: true }))
}
"#;
    assert_eq!(run_source(source).unwrap(), ["Ada", "new"]);
}

#[test]
fn malformed_struct_patterns_fail_with_field_diagnostics() {
    for (source, expected) in [
        (
            "struct Point { x: int, y: int } fn f(value: Point) -> int { return match value { Point { x } => x } } fn main() {}",
            "missing fields y",
        ),
        (
            "struct Point { x: int } fn f(value: Point) -> int { return match value { Point { bad, .. } => bad } } fn main() {}",
            "unknown field `bad`",
        ),
        (
            "struct Point { x: int } fn f(value: Point) -> int { return match value { Point { x, x } => x } } fn main() {}",
            "duplicate",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}

#[test]
fn typed_guards_refine_patterns_in_source_order() {
    let source = r#"
fn classify(value: Option<int>) -> String {
    return match value {
        Some(number) if number > 10 => "large"
        Some(number) if number > 0 => "positive"
        Some(_) => "other"
        None => "none"
    }
}
fn main() {
    print(classify(Some(20)))
    print(classify(Some(5)))
    print(classify(Some(0)))
    print(classify(None))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["large", "positive", "other", "none"]
    );
}

#[test]
fn failed_guards_preserve_non_copy_bindings_for_later_arms() {
    let source = r#"
fn choose(value: Option<String>) -> String {
    return match value {
        Some(text) if text.starts_with("A") => text
        Some(text) => text
        None => "none"
    }
}
fn main() { print(choose(Some("Beta"))) print(choose(Some("Ada"))) }
"#;
    assert_eq!(run_source(source).unwrap(), ["Beta", "Ada"]);
}

#[test]
fn guards_are_boolean_read_only_and_do_not_prove_coverage() {
    for (source, expected) in [
        (
            "fn f(value: bool) -> int { return match value { true if 1 => 1 false => 0 } } fn main() {}",
            "match guard expected Bool",
        ),
        (
            "fn f(value: int) -> int { return match value { _ if value > 0 => 1 } } fn main() {}",
            "non-exhaustive match",
        ),
        (
            "fn take(value: String) -> bool { return true } fn f(value: Option<String>) -> int { return match value { Some(text) if take(text) => 1 Some(_) => 2 None => 0 } } fn main() {}",
            "borrowed value",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn or_patterns_expand_nested_alternatives_with_one_binding_contract() {
    let source = r#"
enum Choice { Left(int) Right(int) Empty }
fn classify(value: Choice) -> String {
    return match value {
        Choice.Left(number) | Choice.Right(number) if number > 10 => "large"
        Choice.Left(0 | 1) | Choice.Right(0 | 1) => "small"
        Choice.Left(_) | Choice.Right(_) => "other"
        Choice.Empty => "empty"
    }
}
fn main() {
    print(classify(Choice.Left(20)))
    print(classify(Choice.Right(1)))
    print(classify(Choice.Left(5)))
    print(classify(Choice.Empty))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["large", "small", "other", "empty"]
    );
}

#[test]
fn or_pattern_binding_contracts_and_redundancy_fail_closed() {
    for (source, expected) in [
        (
            "enum Choice { Left(int) Right(int) } fn f(value: Choice) -> int { return match value { Choice.Left(left) | Choice.Right(right) => 1 } } fn main() {}",
            "same names",
        ),
        (
            "enum Mixed { Number(int) Text(String) } fn f(value: Mixed) -> int { return match value { Mixed.Number(item) | Mixed.Text(item) => 1 } } fn main() {}",
            "same names with the same types",
        ),
        (
            "fn f(value: bool) -> int { return match value { true | true => 1 false => 0 } } fn main() {}",
            "unreachable match arm",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn or_pattern_expansion_has_a_hard_limit() {
    let alternatives = std::iter::repeat_n("true", 4_097)
        .collect::<Vec<_>>()
        .join(" | ");
    let source = format!(
        "fn f(value: bool) -> int {{ return match value {{ {alternatives} => 1 false => 0 }} }} fn main() {{}}"
    );
    let error = check_source(&source).unwrap_err();
    assert!(
        error.message.contains("more than 4096 alternatives"),
        "{}",
        error.message
    );
}

#[test]
fn negative_integer_patterns_are_typed_and_ordered() {
    let source = r#"
fn classify(value: int) -> String {
    return match value { -2 | -1 => "negative" 0 => "zero" _ => "positive" }
}
fn main() { print(classify(-1)) print(classify(0)) print(classify(4)) }
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["negative", "zero", "positive"]
    );

    let error = check_source(
        "fn f(value: i128) -> int { return match value { -170141183460469231731687303715884105729 => 1 _ => 0 } } fn main() {}",
    )
    .unwrap_err();
    assert!(error.message.contains("outside i128 range"));
}
