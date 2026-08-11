use disp::{check_source, run_source};

fn rejects(source: &str, fragment: &str) {
    let error = check_source(source).expect_err("source should be rejected");
    assert!(
        error.message.contains(fragment),
        "expected `{fragment}` in `{}`",
        error.message
    );
    assert!(error.span.start.line > 0 && error.span.start.column > 0);
}

#[test]
fn moves_copy_and_reinitialization_execute() {
    let source = r#"
struct Ticket { label: String }
fn main() {
    let first = Ticket { label: "alpha" }
    let second = first
    print(second.label)
    let count = 7
    let copied = count
    print(count)
    print(copied)
    var current = Ticket { label: "old" }
    let previous = move current
    current = Ticket { label: "new" }
    print(previous.label)
    print(current.label)
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["alpha", "7", "7", "old", "new"]
    );
}

#[test]
fn moves_through_parameters_returns_and_enum_payloads() {
    let source = r#"
struct Token { text: String }
enum Wrapped { Token(Token), Empty }
fn consume(value: Token) -> Token { return value }
fn main() {
    let original = Token { text: "owned" }
    let returned = consume(original)
    let wrapped = Wrapped.Token(returned)
    let text = match wrapped { Token(token) => token.text, Empty => "empty" }
    print(text)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["owned"]);
}

#[test]
fn rejects_use_after_move_and_double_move() {
    rejects(
        r#"struct Boxed { text: String } fn main() { let a = Boxed { text: "x" } let b = a print(a.text) }"#,
        "moved",
    );
    rejects(
        r#"struct Boxed { text: String } fn main() { let a = Boxed { text: "x" } let b = move a let c = move a }"#,
        "moved",
    );
}

#[test]
fn move_diagnostic_points_at_the_illegal_use() {
    let source = "struct Item { text: String }\nfn main() {\n let item = Item { text: \"x\" }\n let moved = item\n print(item)\n}";
    let error = check_source(source).unwrap_err();
    assert!(error.message.contains("moved"));
    assert_eq!(error.span.start.line, 5);
    assert_eq!(error.span.start.column, 8);
    assert!(
        error
            .help
            .as_deref()
            .is_some_and(|help| help.contains("moved at"))
    );
}

#[test]
fn shared_and_mutable_references_obey_nll() {
    let source = r#"
fn set(value: &mut int) { *value = 9 }
fn read(value: &int) -> int { return *value }
fn main() {
    var number = 3
    let first = &number
    let second = &number
    print(read(first) + read(second))
    set(&mut number)
    print(number)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["6", "9"]);
}

#[test]
fn rejects_borrow_conflicts_and_mutation_while_borrowed() {
    rejects(
        r#"fn main() { var x = 1 let r = &x let m = &mut x print(r) print(m) }"#,
        "borrow",
    );
    rejects(
        r#"fn main() { var x = 1 let r = &x x = 2 print(r) }"#,
        "borrow",
    );
    rejects(
        r#"fn main() { var x = 1 let a = &mut x let b = &mut x print(a) print(b) }"#,
        "borrow",
    );
}

#[test]
fn rejects_dangling_and_escaping_references() {
    rejects(
        r#"fn bad() -> &int { let local = 1 return &local } fn main() {}"#,
        "reference",
    );
    rejects(
        r#"fn main() { var out: &int if true { let local = 1 out = &local } print(out) }"#,
        "escapes",
    );
}

#[test]
fn partial_moves_preserve_other_fields_but_not_the_whole_value() {
    let valid = r#"
struct Person { name: String, age: int }
fn main() {
    let person = Person { name: "Ada", age: 36 }
    let name = move person.name
    print(name)
    print(person.age)
}
"#;
    assert_eq!(run_source(valid).unwrap(), ["Ada", "36"]);
    rejects(
        r#"struct Person { name: String, age: int } fn main() { let p = Person { name: "Ada", age: 36 } let n = move p.name print(p) }"#,
        "partially moved",
    );
}

#[test]
fn definite_initialization_merges_control_flow() {
    let valid =
        r#"fn main() { var value: int if true { value = 1 } else { value = 2 } print(value) }"#;
    assert_eq!(run_source(valid).unwrap(), ["1"]);
    rejects(
        r#"fn main() { var value: int if true { value = 1 } print(value) }"#,
        "uninitialized",
    );
}

#[test]
fn copy_marker_is_structurally_validated() {
    let valid = r#"
struct Point { x: int, y: int }
impl Copy for Point {}
fn main() { let a = Point { x: 1, y: 2 } let b = a print(a.x + b.y) }
"#;
    assert_eq!(run_source(valid).unwrap(), ["3"]);
    rejects(
        r#"struct Text { value: String } impl Copy for Text {} fn main() {}"#,
        "Copy",
    );
}

#[test]
fn trait_reference_receivers_dispatch_and_mutate() {
    let source = r#"
struct Counter { value: int }
trait CounterOps {
    fn get(&self) -> int
    fn increment(&mut self)
}
impl CounterOps for Counter {
    fn get(&self) -> int { return self.value }
    fn increment(&mut self) { self.value += 1 }
}
fn main() {
    var counter = Counter { value: 4 }
    print(counter.get())
    counter.increment()
    print(counter.get())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["4", "5"]);
}

#[test]
fn references_integrate_with_generics_adts_and_question() {
    let source = r#"
fn identity<T>(value: &T) -> &T { return value }
fn require(value: &Option<int>) -> Result<int, String> {
    return match *value { Some(number) => Ok(number), None => Err("missing") }
}
fn pass(value: &Option<int>) -> Result<int, String> { return Ok(require(value)?) }
fn main() { let value = Some(8) print(*identity(&value)) print(pass(&value)) }
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["Option.Some(8)", "Result.Ok(8)"]
    );
}

#[test]
fn raw_pointer_dereference_requires_unsafe() {
    rejects(
        r#"fn read(value: ptr<int>) -> int { return *value } fn main() {}"#,
        "unsafe",
    );
    check_source(r#"fn read(value: ptr<int>) -> int { unsafe { return *value } } fn main() {}"#)
        .unwrap();
}
