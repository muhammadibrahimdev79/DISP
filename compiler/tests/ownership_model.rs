use disp::{check_source, ownership, run_source};

fn reject(source: &str, expected: &str) {
    let error = check_source(source).expect_err("ownership-invalid source must be rejected");
    assert!(
        error.message.contains(expected),
        "expected `{expected}` in `{}`",
        error.message
    );
    assert!(error.span.start.line > 0 && error.span.start.column > 0);
}

#[test]
fn initialization_move_partial_and_reinitialization_follow_the_state_machine() {
    let valid = r#"
struct Pair { left: String, right: String }
fn main() {
    var pair = Pair { left: "left", right: "right" }
    let first = move pair.left
    print(pair.right)
    pair.left = "again"
    let whole = move pair
    print(first)
    print(whole.left)
}
"#;
    assert_eq!(run_source(valid).unwrap(), ["right", "left", "again"]);

    reject(
        "struct Pair { left: String, right: String } fn main() { let pair = Pair { left: \"a\", right: \"b\" } let left = move pair.left print(pair) }",
        "partially moved",
    );
    reject(
        "struct Pair { left: String, right: String } fn main() { let pair = Pair { left: \"a\", right: \"b\" } let left = move pair.left print(pair.left) }",
        "moved",
    );
}

#[test]
fn control_flow_join_requires_initialization_on_every_predecessor() {
    check_source(
        "fn main() { var value: String if true { value = \"a\" } else { value = \"b\" } print(value) }",
    )
    .unwrap();
    reject(
        "fn main() { var value: String if true { value = \"a\" } print(value) }",
        "uninitialized",
    );
    reject(
        "fn main() { let value = \"a\" if true { let moved = move value } print(value) }",
        "moved",
    );
}

#[test]
fn loan_overlap_is_place_sensitive_and_fail_closed_for_uncertainty() {
    check_source(
        r#"
struct Pair { left: String, right: String }
fn main() {
    var pair = Pair { left: "a", right: "b" }
    let left = &mut pair.left
    let right = &mut pair.right
    print(left)
    print(right)
}
"#,
    )
    .unwrap();
    reject(
        r#"
struct Pair { left: String, right: String }
fn main() {
    var pair = Pair { left: "a", right: "b" }
    let left = &mut pair.left
    let whole = &pair
    print(left)
    print(whole)
}
"#,
        "overlap",
    );
    reject(
        "fn main() { var values = List.of(1, 2) var index = 0 let first = &mut values[index] let second = &mut values[1] print(first) print(second) }",
        "borrow",
    );
}

#[test]
fn non_lexical_loans_end_after_their_last_use() {
    let source = r#"
fn set(value: &mut int) { *value = 9 }
fn main() {
    var number = 1
    let shared = &number
    print(*shared)
    set(&mut number)
    print(number)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["1", "9"]);
    reject(
        "fn main() { var number = 1 let shared = &number number = 2 print(*shared) }",
        "borrowed",
    );
}

#[test]
fn borrowed_origins_cannot_escape_through_aggregates_or_closures() {
    reject(
        "struct Holder { value: &int } fn bad() -> Holder { let local = 1 return Holder { value: &local } } fn main() {}",
        "reference",
    );
    reject(
        "fn bad() -> Option<&int> { let local = 1 return Some(&local) } fn main() {}",
        "reference",
    );
    reject(
        "fn bad() -> fn() -> int { let local = 1 return || *(&local) } fn main() {}",
        "closure borrowing",
    );
}

#[test]
fn aggregate_borrows_preserve_safe_input_origins_and_active_loans() {
    let valid = r#"
struct Holder { value: &int }
fn hold(value: &int) -> Holder { return Holder { value } }
fn main() {
    let number = 42
    let holder = hold(&number)
    print(*holder.value)
}
"#;
    assert_eq!(run_source(valid).unwrap(), ["42"]);

    reject(
        "struct Holder { value: &int } fn main() { var number = 1 let holder = Holder { value: &number } number = 2 print(*holder.value) }",
        "borrowed",
    );
    reject(
        "struct PairRef { left: &int, right: &int } fn combine(left: &int, right: &int) -> PairRef { return PairRef { left, right } } fn main() {}",
        "borrowed return",
    );

    let enum_borrow = r#"
enum Borrowed { Value(&int) }
fn pass(value: &int) -> &int {
    return match Borrowed.Value(value) { Borrowed.Value(inner) => inner }
}
fn main() { let number = 9 print(*pass(&number)) }
"#;
    assert_eq!(run_source(enum_borrow).unwrap(), ["9"]);

    let generic_borrow = r#"
struct Holder<T> { value: T }
fn hold(value: &int) -> Holder<&int> { return Holder { value } }
fn main() { let number = 11 let holder = hold(&number) print(*holder.value) }
"#;
    assert_eq!(run_source(generic_borrow).unwrap(), ["11"]);
    let inferred_generic_borrow = r#"
struct Holder<T> { value: T }
fn main() { let number = 12 let holder = Holder { value: &number } print(*holder.value) }
"#;
    assert_eq!(run_source(inferred_generic_borrow).unwrap(), ["12"]);
    reject(
        "struct Holder<T> { value: T } fn bad() -> Holder<&int> { let local = 1 return Holder { value: &local } } fn main() {}",
        "local",
    );
    reject(
        "struct Holder<T> { value: T } fn main() { var number = 1 let holder = Holder { value: &number } number = 2 print(*holder.value) }",
        "borrowed",
    );
}

#[test]
fn every_structured_exit_has_a_drop_obligation() {
    let source = r#"
struct Resource { name: String }
fn unit() {}
fn fail() -> Result<Unit, String> { return Err("failure") }
fn exits() -> Result<Unit, String> {
    let scoped = Resource { name: "scope" }
    if false {
        let returning = Resource { name: "return" }
        return Ok(unit())
    }
    loop {
        let breaking = Resource { name: "break" }
        break
    }
    var repeat = true
    while repeat {
        repeat = false
        let continuing = Resource { name: "continue" }
        continue
    }
    let propagating = Resource { name: "propagate" }
    fail()?
    return Ok(unit())
}
fn main() { print(exits()) }
"#;
    let program = check_source(source).unwrap();
    let report = ownership::check(&program).unwrap();
    for reason in [
        ownership::DropReason::ScopeEnd,
        ownership::DropReason::Return,
        ownership::DropReason::Break,
        ownership::DropReason::Continue,
        ownership::DropReason::Propagation,
    ] {
        assert!(
            report.drops.iter().any(|drop| drop.reason == reason),
            "missing drop evidence for {reason:?}"
        );
    }
}
