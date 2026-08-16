use disp::{
    check_source,
    diagnostics::{DiagnosticKind, Position, Span},
};

#[test]
fn compiler_stages_have_stable_codes_and_exact_spans() {
    let cases = [
        (
            "fn main() {\n    print(\"unterminated)\n}\n",
            DiagnosticKind::Lex,
            "DISP-LEX-0001",
            Span::new(
                Position {
                    line: 2,
                    column: 11,
                },
                Position { line: 3, column: 1 },
            ),
        ),
        (
            "fn main() {\n    let = 1\n}\n",
            DiagnosticKind::Parse,
            "DISP-PARSE-0001",
            Span::new(
                Position { line: 2, column: 9 },
                Position {
                    line: 2,
                    column: 10,
                },
            ),
        ),
        (
            "fn main() {\n    print(missing)\n}\n",
            DiagnosticKind::Resolve,
            "DISP-RESOLVE-0001",
            Span::new(
                Position {
                    line: 2,
                    column: 11,
                },
                Position {
                    line: 2,
                    column: 18,
                },
            ),
        ),
        (
            "fn main() {\n    let value: bool = 1\n}\n",
            DiagnosticKind::Type,
            "DISP-TYPE-0001",
            Span::new(
                Position {
                    line: 2,
                    column: 23,
                },
                Position {
                    line: 2,
                    column: 24,
                },
            ),
        ),
    ];

    for (source, kind, code, span) in cases {
        let diagnostic = check_source(source).unwrap_err();
        assert_eq!(diagnostic.kind, kind);
        assert_eq!(diagnostic.kind.code(), code);
        assert_eq!(diagnostic.span, span);
        let json = diagnostic.render_json("case.disp");
        assert!(json.contains(&format!("\"code\":\"{code}\"")));
        assert!(json.contains(&format!("\"stage\":\"{}\"", kind.label())));
        assert!(json.contains(&format!(
            "\"start\":{{\"line\":{},\"column\":{}}}",
            span.start.line, span.start.column
        )));
    }
}

#[test]
fn every_diagnostic_category_has_a_unique_stable_code() {
    let kinds = [
        DiagnosticKind::Lex,
        DiagnosticKind::Parse,
        DiagnosticKind::Resolve,
        DiagnosticKind::Type,
        DiagnosticKind::Runtime,
        DiagnosticKind::Internal,
        DiagnosticKind::Backend,
    ];
    let mut codes = std::collections::BTreeSet::new();
    for kind in kinds {
        let code = kind.code();
        assert!(code.starts_with("DISP-") && code.ends_with("-0001"));
        assert!(codes.insert(code), "duplicate diagnostic code {code}");
    }
}

#[test]
fn compiler_help_and_source_override_survive_json_rendering() {
    let diagnostic = check_source("fn main() { let cafe\u{301} = 1 }").unwrap_err();
    assert_eq!(diagnostic.kind, DiagnosticKind::Lex);
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("café"))
    );
    let json = diagnostic
        .with_file("normalized/module.disp")
        .render_json("entry.disp");
    assert!(json.contains("\"file\":\"normalized/module.disp\""));
    assert!(json.contains("\"help\":\"write the identifier as `café`\""));
}
