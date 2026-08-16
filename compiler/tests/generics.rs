use disp::{check_source, run_source};

#[test]
fn generic_functions_and_structs_infer_and_execute() {
    let source = r#"
struct Box<T> { value: T }

fn identity<T>(value: T) -> T { return value }

fn main() {
    let boxed = Box { value: identity(42) }
    print(boxed.value)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["42"]);
}

#[test]
fn generic_instantiation_rejects_conflicting_inference() {
    let source = r#"
fn choose<T>(left: T, right: T) -> T { return left }
fn main() { print(choose(1, true)) }
"#;
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("conflicting inferred types"));
}

#[test]
fn generic_enums_substitute_payload_types() {
    let source = r#"
enum Maybe<T> { Present(T), Absent }
fn unwrap_or<T>(value: Maybe<T>, fallback: T) -> T {
    return match value { Maybe.Present(inner) => inner, Maybe.Absent => fallback }
}
fn main() { print(unwrap_or(Maybe.Present(7), 0)) }
"#;
    assert_eq!(run_source(source).unwrap(), ["7"]);
}

#[test]
fn trait_methods_dispatch_statically() {
    let source = r#"
struct Counter { value: int }
trait Value { fn value(self: Self) -> int }
impl Value for Counter {
    fn value(self: Self) -> int { return self.value }
}
fn main() { let counter = Counter { value: 9 } print(counter.value()) }
"#;
    assert_eq!(run_source(source).unwrap(), ["9"]);
}

#[test]
fn generic_constraints_are_enforced() {
    let valid = r#"
struct Counter { value: int }
trait Value { fn value(self: Self) -> int }
impl Value for Counter { fn value(self: Self) -> int { return self.value } }
fn read<T: Value>(item: T) -> int { return item.value() }
fn main() { print(read(Counter { value: 11 })) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["11"]);

    let invalid = r#"
trait Value { fn value(self: Self) -> int }
fn read<T: Value>(item: T) -> T { return item }
fn main() { print(read(1)) }
"#;
    let error = check_source(invalid).unwrap_err();
    assert!(error.message.contains("does not satisfy constraint"));
}

#[test]
fn conflicting_implementations_and_bad_methods_fail() {
    let conflict = r#"
struct Item { value: int }
trait Value { fn value(self: Self) -> int }
impl Value for Item { fn value(self: Self) -> int { return self.value } }
impl Value for Item { fn value(self: Self) -> int { return self.value } }
fn main() {}
"#;
    assert!(
        check_source(conflict)
            .unwrap_err()
            .message
            .contains("conflicting implementation")
    );

    let mismatch = r#"
struct Item { value: int }
trait Value { fn value(self: Self) -> int }
impl Value for Item { fn value(self: Self) -> bool { return true } }
fn main() {}
"#;
    assert!(
        check_source(mismatch)
            .unwrap_err()
            .message
            .contains("does not match trait")
    );
}

#[test]
fn generic_trait_implementations_instantiate_and_overlap_is_rejected() {
    let valid = r#"
struct Box<T> { value: T }
trait Extract { fn extract(self: Self) -> int }
impl<T> Extract for Box<T> { fn extract(self: Self) -> int { return 1 } }
fn main() { print(Box { value: true }.extract()) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["1"]);

    let overlap = r#"
struct Box<T> { value: T }
trait Extract { fn extract(self: Self) -> int }
impl<T> Extract for Box<T> { fn extract(self: Self) -> int { return 1 } }
impl Extract for Box<int> { fn extract(self: Self) -> int { return 2 } }
fn main() {}
"#;
    assert!(
        check_source(overlap)
            .unwrap_err()
            .message
            .contains("conflicting implementation")
    );
}

#[test]
fn associated_type_definitions_are_complete() {
    let valid = r#"
struct Item { value: int }
trait Source { type Output fn get(self: Self) -> Self.Output }
impl Source for Item { type Output = int fn get(self: Self) -> int { return self.value } }
fn main() { print(Item { value: 3 }.get()) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["3"]);

    let missing = r#"
struct Item { value: int }
trait Source { type Output fn get(self: Self) -> int }
impl Source for Item { fn get(self: Self) -> int { return self.value } }
fn main() {}
"#;
    assert!(
        check_source(missing)
            .unwrap_err()
            .message
            .contains("associated types")
    );
}

#[test]
fn associated_type_projection_is_resolved_by_the_selected_implementation() {
    let mismatch = r#"
struct Item { value: int }
trait Source { type Output fn get(self: Self) -> Self.Output }
impl Source for Item { type Output = int fn get(self: Self) -> bool { return true } }
fn main() {}
"#;
    let error = check_source(mismatch).unwrap_err();
    assert!(
        error.message.contains("does not match trait"),
        "{}",
        error.message
    );

    let undeclared = r#"
trait Source { fn get(self: Self) -> Self.Missing }
fn main() {}
"#;
    let error = check_source(undeclared).unwrap_err();
    assert!(
        error.message.contains("undeclared associated type"),
        "{}",
        error.message
    );

    let dependent = r#"
trait Source { type Output }
fn invalid<T: Source>(value: T) -> T.Output { return value }
fn main() {}
"#;
    let error = check_source(dependent).unwrap_err();
    assert!(
        error.message.contains("must start with `Self`"),
        "{}",
        error.message
    );
}

#[test]
fn ambiguous_and_invalid_method_resolution_fail() {
    let ambiguous = r#"
struct Item { value: int }
trait Left { fn get(self: Self) -> int }
trait Right { fn get(self: Self) -> int }
impl Left for Item { fn get(self: Self) -> int { return 1 } }
impl Right for Item { fn get(self: Self) -> int { return 2 } }
fn main() { print(Item { value: 0 }.get()) }
"#;
    assert!(
        check_source(ambiguous)
            .unwrap_err()
            .message
            .contains("ambiguous method")
    );

    let invalid = "struct Item { value: int } fn main() { Item { value: 0 }.missing() }";
    assert!(
        check_source(invalid)
            .unwrap_err()
            .message
            .contains("no method `missing`")
    );
}

#[test]
fn constrained_generic_adts_reject_invalid_instantiation() {
    let source = r#"
trait Mark { fn mark(self: Self) -> int }
struct Box<T: Mark> { value: T }
fn main() { let invalid = Box { value: 1 } }
"#;
    assert!(
        check_source(source)
            .unwrap_err()
            .message
            .contains("does not satisfy constraint")
    );
}

#[test]
fn malformed_generic_and_trait_declarations_fail_with_spans() {
    let cases = [
        (
            "struct Pair<T, T> { value: T } fn main() {}",
            "duplicate generic parameter",
        ),
        (
            "struct Box<T> { value: T } fn main() { let x: Box<int, bool> = Box { value: 1 } }",
            "expects 1 type arguments",
        ),
        (
            "fn id<T: Missing>(x: T) -> T { return x } fn main() {}",
            "unknown trait",
        ),
        (
            "struct Item { value: int } trait Get { fn get(self: Self) -> int } impl Get for Item {} fn main() {}",
            "must define exactly",
        ),
    ];
    for (source, expected) in cases {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}

#[test]
fn generic_trait_arguments_substitute_into_methods() {
    let source = r#"
struct Accumulator { value: int }
trait Add<T> { fn add(self: Self, value: T) -> int }
impl Add<int> for Accumulator {
    fn add(self: Self, value: int) -> int { return self.value + value }
}
fn main() { print(Accumulator { value: 5 }.add(7)) }
"#;
    assert_eq!(run_source(source).unwrap(), ["12"]);
}

#[test]
fn trait_method_generic_contracts_are_alpha_equivalent_and_exact() {
    let valid = r#"
struct Keeper { value: int }
trait Keep { fn keep<T>(self: Self, value: T) -> T }
impl Keep for Keeper { fn keep<U>(self: Self, value: U) -> U { return value } }
fn main() { let keeper = Keeper { value: 0 } let value: int = 42 print(keeper.keep(value)) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["42"]);

    let constraint_mismatch = r#"
struct Keeper { value: int }
trait Keep { fn keep<T: Copy>(self: Self, value: T) -> T }
impl Keep for Keeper { fn keep<U>(self: Self, value: U) -> U { return value } }
fn main() {}
"#;
    let error = check_source(constraint_mismatch).unwrap_err();
    assert!(
        error.message.contains("does not match trait"),
        "{}",
        error.message
    );
}

#[test]
fn trait_method_capability_contracts_must_match_exactly() {
    let source = r#"
struct Loader { value: int }
trait Load {
    fn load(self: Self, path: Path) -> Result<String, IoError> uses Pure
}
impl Load for Loader {
    fn load(self: Self, path: Path) -> Result<String, IoError> uses FileSystem {
        return File.read_text(path)
    }
}
fn main() uses Pure {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(
        error.message.contains("does not match trait"),
        "{}",
        error.message
    );
}

#[test]
fn associated_type_declarations_are_unique_and_definitions_are_resolved() {
    for (source, expected) in [
        (
            "trait Source { type Output type Output } fn main() {}",
            "associated type more than once",
        ),
        (
            "struct Item { value: int } trait Source { type Output } impl Source for Item { type Output = int type Output = int } fn main() {}",
            "associated type more than once",
        ),
        (
            "struct Item { value: int } trait Source { type Output } impl Source for Item { type Output = Missing } fn main() {}",
            "unknown type",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}

#[test]
fn constrained_implementations_apply_only_when_their_requirements_hold() {
    let valid = r#"
struct Tagged { value: int }
struct Box<T> { value: T }
trait Mark { fn mark(self: Self) -> int }
trait Extract { fn extract(self: Self) -> int }
impl Mark for Tagged { fn mark(self: Self) -> int { return self.value } }
impl<T: Mark> Extract for Box<T> { fn extract(self: Self) -> int { return 7 } }
fn main() { print(Box { value: Tagged { value: 1 } }.extract()) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["7"]);

    let invalid = r#"
struct Box<T> { value: T }
trait Mark { fn mark(self: Self) -> int }
trait Extract { fn extract(self: Self) -> int }
impl<T: Mark> Extract for Box<T> { fn extract(self: Self) -> int { return 7 } }
fn main() { print(Box { value: 1 }.extract()) }
"#;
    let error = check_source(invalid).unwrap_err();
    assert!(
        error.message.contains("no method `extract`"),
        "{}",
        error.message
    );
}

#[test]
fn cyclic_implementation_requirements_fail_without_recursion() {
    let source = r#"
struct Wrapper<T> { value: T }
trait Cycle { fn value(self: Self) -> int }
impl<T: Cycle> Cycle for Wrapper<T> { fn value(self: Self) -> int { return 1 } }
fn main() { print(Wrapper { value: 1 }.value()) }
"#;
    let error = check_source(source).unwrap_err();
    assert!(
        error.message.contains("no method `value`"),
        "{}",
        error.message
    );
}

#[test]
fn implementation_generics_must_be_constrained_by_the_header() {
    let source = r#"
struct Item { value: int }
trait Value { fn value(self: Self) -> int }
impl<T> Value for Item { fn value(self: Self) -> int { return 1 } }
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(
        error.message.contains("not constrained by the target"),
        "{}",
        error.message
    );
}

#[test]
fn trait_argument_constraints_are_checked_without_declaration_order_dependence() {
    let valid = r#"
struct Tagged { value: int }
trait Mark {}
trait Read<T: Mark> { fn read(self: Self) -> int }
impl Read<Tagged> for Tagged { fn read(self: Self) -> int { return self.value } }
impl Mark for Tagged {}
fn main() { print(Tagged { value: 8 }.read()) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["8"]);

    let invalid = r#"
struct Item { value: int }
trait Mark {}
trait Read<T: Mark> { fn read(self: Self) -> int }
impl Read<int> for Item { fn read(self: Self) -> int { return 1 } }
fn main() {}
"#;
    let error = check_source(invalid).unwrap_err();
    assert!(error.message.contains("does not satisfy constraint `Mark`"));
}

#[test]
fn copy_implementation_selection_respects_concrete_arguments_and_constraints() {
    let source = r#"
struct Wrapper<T> { marker: int }
impl<T: Copy> Copy for Wrapper<T> {}
fn require_copy<T: Copy>(value: T) -> T { return value }
fn probe(value: Wrapper<String>) { print(require_copy(value).marker) }
fn main() {}
"#;
    let error = check_source(source).unwrap_err();
    assert!(
        error.message.contains("does not satisfy constraint `Copy`"),
        "{}",
        error.message
    );
}
