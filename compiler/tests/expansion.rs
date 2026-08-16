use disp::{check_source, expansion_report_source, run_source};

#[test]
fn repeat_and_map_expand_structured_expressions() {
    let source = r#"
fn main() {
    let repeated = Meta.repeat(3, 7)
    let squares = Meta.map(5, |index: int| index * index)
    print(repeated)
    print(squares)
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["[7, 7, 7]", "[0, 1, 4, 9, 16]"]
    );
    let report = expansion_report_source(source).unwrap();
    assert_eq!(report.expansions.len(), 2);
    assert_eq!(report.expansions[0].name, "Meta.repeat");
    assert_eq!(report.expansions[1].name, "Meta.map");
    assert!(report.generated_nodes > 0);
}

#[test]
fn map_substitution_is_hygienic_and_preserves_call_site_names() {
    let source = r#"
fn main() {
    let index = 100
    let values = Meta.map(3, |slot: int| slot + index)
    let nested = Meta.map(2, |slot: int| (|slot: int| slot + 10)(slot))
    print(values)
    print(nested)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["[100, 101, 102]", "[10, 11]"]);
}

#[test]
fn expansion_limits_and_shapes_fail_closed_with_spans() {
    for (source, expected) in [
        (
            "fn main() { let values = Meta.repeat(4097, 0) }",
            "exceeds the limit",
        ),
        (
            "fn main() { let count = 2 let values = Meta.repeat(count, 0) }",
            "constant integer expression",
        ),
        (
            "fn main() { let values = Meta.map(2, |left: int, right: int| left) }",
            "exactly one mapper parameter",
        ),
        (
            "fn main() { let values = Meta.map(2, |index: int| -> int { return index }) }",
            "expression closure",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(error.span.start.line > 0 && error.span.start.column > 0);
    }
}

#[test]
fn generated_output_budget_counts_nested_expansion() {
    let source = "fn main() { let values = Meta.repeat(4096, Meta.repeat(32, 0)) }";
    let error = check_source(source).unwrap_err();
    assert!(
        error.message.contains("generated nodes"),
        "{}",
        error.message
    );
}

#[test]
fn compiler_owned_meta_namespace_cannot_be_shadowed() {
    for source in [
        "fn main() { let Meta = 1 print(Meta) }",
        "fn main(Meta: int) { print(Meta) }",
        "fn Meta() {} fn main() {}",
        "struct Meta { value: int } fn main() {}",
        "trait Meta { fn value() -> int } fn main() {}",
    ] {
        let error = check_source(source).unwrap_err();
        assert!(
            error.message.contains("Meta")
                && (error.message.contains("reserved") || error.message.contains("duplicate")),
            "{}",
            error.message
        );
    }
}
