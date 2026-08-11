use disp::{check_source, run_source};

#[test]
fn fixed_arrays_are_typed_and_bounds_checked() {
    let source =
        "fn main() { let values: [int; 3] = [10, 20, 30] print(values.len()) print(values[1]) }";
    assert_eq!(run_source(source).unwrap(), ["3", "20"]);

    let error = run_source("fn main() { let values = [1, 2] print(values[2]) }")
        .expect_err("out-of-bounds indexing must fail");
    assert!(error.message.contains("out of bounds"));
}

#[test]
fn arrays_reject_mixed_elements_and_wrong_lengths() {
    let mixed = check_source("fn main() { let values = [1, true] }").unwrap_err();
    assert!(mixed.message.contains("array element"));

    let length = check_source("fn main() { let values: [int; 2] = [1, 2, 3] }").unwrap_err();
    assert!(length.message.contains("binding initializer"));
}

#[test]
fn strings_have_owned_capacity_and_read_only_queries() {
    let source = r#"fn main() {
        let empty = String.new()
        let reserved = String.with_capacity(32)
        print(empty.len())
        print(empty.is_empty())
        print(reserved.capacity())
    }"#;
    assert_eq!(run_source(source).unwrap(), ["0", "true", "32"]);
}

#[test]
fn strings_mutate_as_utf8_through_mutable_borrows() {
    let source = r#"fn main() {
        var text = String.with_capacity(4)
        text.push('A')
        text.push('界')
        text.push_str("!")
        print(text)
        print(text.len())
        text.clear()
        print(text.is_empty())
    }"#;
    assert_eq!(run_source(source).unwrap(), ["A界!", "5", "true"]);

    let error = check_source("fn main() { let text = String.new() text.push('x') }")
        .expect_err("immutable strings must reject mutation");
    assert!(error.message.contains("mutable"));
}

#[test]
fn string_queries_borrow_without_moving() {
    let source = r#"fn main() {
        let text = "alphabet"
        print(text.contains("pha"))
        print(text.starts_with("alp"))
        print(text.ends_with("bet"))
        print(text)
    }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["true", "true", "true", "alphabet"]
    );
}

#[test]
fn borrowed_str_views_are_zero_copy_and_keep_the_owner_borrowed() {
    let source = r#"fn main() {
        let text = "alphabet"
        let view: &str = &text
        print((*view).len())
        print((*view).is_empty())
        print((*view).contains("pha"))
        print((*view).starts_with("alp"))
        print((*view).ends_with("bet"))
        print(text)
    }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["8", "false", "true", "true", "true", "alphabet"]
    );

    let error = check_source(
        "fn main() { var text=String.new(); let view: &str=&text; text.clear(); print((*view).len()) }",
    )
    .expect_err("a String cannot mutate while its str view is live");
    assert!(error.message.contains("overlap"), "{}", error.message);
}
