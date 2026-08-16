use disp::{
    check_source,
    const_eval::{Limits, evaluate_with_limits},
    constant_report_source, lower_source,
};

#[test]
fn constants_are_evaluated_deterministically_with_lexical_references() {
    let source = r#"
struct Pair { left: int, right: int }
fn main() {
    const base = 7
    const answer = base * 6
    const selected = match answer { 42 => "yes", _ => "no" }
    const pair = Pair { right: answer, left: base }
    print(pair.right)
}
"#;
    let first = constant_report_source(source).unwrap();
    let second = constant_report_source(source).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first.render(),
        "main::base = 7\nmain::answer = 42\nmain::selected = \"yes\"\nmain::pair = Pair { left: 7, right: 42 }\n"
    );
}

#[test]
fn invalid_arithmetic_fails_during_check_not_runtime() {
    let error = check_source("fn main() { const value = 1 / 0 }").unwrap_err();
    assert!(
        error.message.contains("division by zero"),
        "{}",
        error.message
    );
    assert!(error.span.start.line > 0 && error.span.start.column > 0);
}

#[test]
fn compile_time_calls_and_ambient_authority_fail_closed() {
    for source in [
        "fn value() -> int = 42\nfn main() { const answer = value() }",
        "fn main() { const secret = File.read_text(Path(\"secret.txt\")) }",
        "fn main() { const response = Http.get(\"https://example.com\") }",
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains("compile-time"), "{}", error.message);
    }
}

#[test]
fn evaluator_budgets_are_deterministic_and_fail_closed() {
    let program = check_source("fn main() { const values = [1, 2, 3, 4] }").unwrap();
    let error = evaluate_with_limits(
        &program,
        Limits {
            steps: 3,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("exceeded 3 steps"));

    let error = evaluate_with_limits(
        &program,
        Limits {
            value_nodes: 4,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("exceeded 4 nodes"));
}

#[test]
fn evaluated_constants_are_folded_before_hir_and_mir_lowering() {
    let (_, mir) =
        lower_source("fn main() { const base = 7 const answer = base * 6 print(answer) }").unwrap();
    let dump = disp::mir::dump(&mir);
    assert!(!dump.contains("BinaryOp(Multiply"), "{dump}");
    assert!(dump.contains("Constant(Unsigned(42"), "{dump}");
}
