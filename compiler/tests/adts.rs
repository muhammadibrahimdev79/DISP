use disp::{check_source, diagnostics::DiagnosticKind, run_source};

fn type_error(source: &str, expected_message: &str) {
    let error = check_source(source).expect_err("program should fail static checking");
    assert_eq!(
        error.kind,
        DiagnosticKind::Type,
        "unexpected diagnostic: {error}"
    );
    assert!(
        error.message.contains(expected_message),
        "expected `{expected_message}` in `{}`",
        error.message
    );
    assert!(error.span.start.line >= 1);
    assert!(error.span.start.column >= 1);
}

#[test]
fn structs_construct_and_access_fields() {
    let output = run_source(
        "struct Point { x: int y: int } fn sum(point: Point) -> int { return point.x + point.y } fn main() { let point = Point { x: 20 y: 22 } print(sum(point)) }",
    )
    .expect("struct program should run");
    assert_eq!(output, ["42"]);
}

#[test]
fn rejects_invalid_struct_fields() {
    type_error(
        "struct Point { x: int } fn main() { let point = Point { y: 1 } }",
        "has no field `y`",
    );
    type_error(
        "struct Point { x: int y: int } fn main() { let point = Point { x: 1 } }",
        "missing field `y`",
    );
    type_error(
        "struct Point { x: int } fn main() { let point = Point { x: true } }",
        "struct field expected Int, found Bool",
    );
    type_error(
        "struct Point { x: int } fn main() { let point = Point { x: 1 } print(point.y) }",
        "has no field `y`",
    );
    type_error(
        "struct Point { x: int } fn main() { let point = Point { x: 1 x: 2 } }",
        "provided more than once",
    );
}

#[test]
fn enum_payloads_and_exhaustive_match_execute() {
    let output = run_source(
        "enum Message { Text(String) Number(int) Quit } fn render(message: Message) -> String { return match message { Message.Text(text) => text Message.Number(number) => \"number\" Message.Quit => \"quit\" } } fn main() { print(render(Message.Text(\"hello\"))) print(render(Message.Quit)) }",
    )
    .expect("enum program should run");
    assert_eq!(output, ["hello", "quit"]);
}

#[test]
fn rejects_wrong_enum_payloads_and_non_exhaustive_matches() {
    type_error(
        "enum Message { Text(String) Quit } fn main() { let message = Message.Text(1) }",
        "function argument expected String, found Int",
    );
    type_error(
        "enum Message { Text(String) Quit } fn main() { let message = Message.Text() }",
        "expects 1 arguments, found 0",
    );
    type_error(
        "enum Message { Text(String) Quit } fn show(message: Message) -> String { return match message { Message.Text(text) => text } } fn main() {}",
        "non-exhaustive match",
    );
    type_error(
        "enum Message { Text(String) Quit } fn show(message: Message) -> String { return match message { Message.Text() => \"bad\" Message.Quit => \"quit\" } } fn main() {}",
        "expects 1 payload patterns, found 0",
    );
    type_error(
        "enum Left { Value(int) } enum Right { Value(int) } fn show(value: Left) -> int { return match value { Right.Value(number) => number Left.Value(number) => number } } fn main() {}",
        "variant from `Right` cannot match",
    );
    type_error(
        "enum Choice { Yes No } fn choose(value: Choice) -> int { return match value { Choice.Yes => 1 Choice.No => false } } fn main() {}",
        "match arm expected Int, found Bool",
    );
}

#[test]
fn option_result_and_question_propagate() {
    let output = run_source(
        "enum Failure { Missing } fn maybe(found: bool) -> Option<int> { if found { return Some(21) } return None } fn doubled(found: bool) -> Option<int> { let value = maybe(found)? return Some(value * 2) } fn result(ok: bool) -> Result<int, Failure> { if ok { return Ok(7) } return Err(Failure.Missing) } fn tripled(ok: bool) -> Result<int, Failure> { let value = result(ok)? return Ok(value * 3) } fn main() { match doubled(true) { Some(value) => print(value) None => print(0) } match doubled(false) { Some(value) => print(value) None => print(0) } match tripled(true) { Ok(value) => print(value) Err(error) => print(0) } match tripled(false) { Ok(value) => print(value) Err(error) => print(-1) } }",
    )
    .expect("Option and Result propagation should run");
    assert_eq!(output, ["42", "0", "21", "-1"]);
}

#[test]
fn payload_patterns_can_refine_a_variant_before_its_catch_all() {
    let output = run_source(
        "fn label(value: Option<int>) -> String { return match value { Some(0) => \"zero\" Some(number) => \"number\" None => \"none\" } } fn main() { print(label(Some(0))) print(label(Some(4))) print(label(None)) }",
    )
    .expect("payload patterns should be ordered and exhaustive");
    assert_eq!(output, ["zero", "number", "none"]);
}

#[test]
fn rejects_invalid_question_usage() {
    type_error(
        "fn bad(value: int) -> int { return value? } fn main() {}",
        "requires Option or Result",
    );
    type_error(
        "fn maybe() -> Option<int> { return Some(1) } fn bad() -> int { return maybe()? } fn main() {}",
        "requires the enclosing function to return Option",
    );
    type_error(
        "enum FirstError { Bad } enum SecondError { Bad } fn source() -> Result<int, FirstError> { return Err(FirstError.Bad) } fn bad() -> Result<int, SecondError> { return Ok(source()?) } fn main() {}",
        "propagated error expected",
    );
}

#[test]
fn nominal_types_do_not_unify_structurally() {
    type_error(
        "struct UserId { value: int } struct AccountId { value: int } fn accept(value: UserId) {} fn main() { accept(AccountId { value: 1 }) }",
        "function argument expected UserId, found AccountId",
    );
    type_error(
        "enum Left { Value(int) } enum Right { Value(int) } fn accept(value: Left) {} fn main() { accept(Right.Value(1)) }",
        "function argument expected Left, found Right",
    );
}

#[test]
fn adt_diagnostics_preserve_exact_source_spans() {
    let source = "struct Point {\n    x: int\n}\nfn main() {\n    let point = Point { x: 1 }\n    print(point.missing)\n}";
    let error = check_source(source).expect_err("missing field should fail");
    assert_eq!(error.kind, DiagnosticKind::Type);
    assert_eq!(error.span.start.line, 6);
    assert_eq!(error.span.start.column, 17);
}
