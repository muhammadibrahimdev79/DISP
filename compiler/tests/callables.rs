use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-callable-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        expected
    );
}

#[test]
fn named_functions_are_real_first_class_values() {
    let source = r#"
fn add(left: int, right: int) -> int = left + right
fn apply(operation: fn(int, int) -> int, left: int, right: int) -> int = operation(left, right)

fn main() {
    let operation: fn(int, int) -> int = add
    print(operation(20, 22))
    print(apply(add, 19, 23))
}
"#;
    differential("named", source);

    let (hir, mir) = lower_source(source).unwrap();
    assert!(
        mir.functions
            .iter()
            .flat_map(|function| &function.blocks)
            .any(|block| matches!(
                block.terminator,
                disp::mir::Terminator::Call {
                    target: disp::hir::CallTarget::Callable,
                    ..
                }
            ))
    );
    let target = Target::host().unwrap();
    let callable = disp::hir::Type::Function(
        vec![
            disp::hir::Type::Int {
                signed: true,
                width: None,
            },
            disp::hir::Type::Int {
                signed: true,
                width: None,
            },
        ],
        Box::new(disp::hir::Type::Int {
            signed: true,
            width: None,
        }),
    );
    let layout = LayoutEngine::new(target, &hir).layout(&callable).unwrap();
    assert_eq!(layout.size, 24);
    assert_eq!(layout.align, 8);
    assert_eq!(
        abi::classify(&callable, &layout, target),
        abi::PassMode::Indirect
    );
    let narrow_target = Target {
        pointer_width: 32,
        pointer_alignment: 4,
        ..target
    };
    let narrow_layout = LayoutEngine::new(narrow_target, &hir)
        .layout(&callable)
        .unwrap();
    assert_eq!(narrow_layout.size, 12);
    assert_eq!(narrow_layout.align, 4);
}

#[test]
fn generic_function_values_fail_until_concretized() {
    let error = disp::check_source(
        "fn identity<T>(value: T) -> T = value fn main() { let operation = identity }",
    )
    .unwrap_err();
    assert!(error.message.contains("needs concrete type arguments"));
    assert_eq!(error.span.start.line, 1);
}

#[test]
fn closures_capture_shared_mutable_and_moved_state_differentially() {
    let source = r#"
fn apply(operation: fn(int) -> int, value: int) -> int = operation(value)

fn main() {
    let offset = 2
    let add_offset = |value: int| value + offset
    print(apply(add_offset, 40))

    var count = 0
    let next = || -> int {
        count += 1
        return count
    }
    print(next())
    print(next())

    let label = "DISP"
    let owned = move || label.len()
    print(owned())
}
"#;
    differential("closures", source);
}

#[test]
fn closure_capture_errors_have_exact_source_locations() {
    let cases = [
        (
            "fn main() {\n    let text = \"x\"\n    let bad = || text\n}\n",
            "cannot move captured `text` out of a reusable closure",
            3,
        ),
        (
            "fn make() -> fn() -> int {\n    let value = 1\n    return || value\n}\nfn main() {}\n",
            "cannot escape",
            3,
        ),
        (
            "fn main() {\n    var value = 1\n    let read = || value\n    value = 2\n    print(read())\n}\n",
            "borrowed",
            4,
        ),
        (
            "fn main() {\n    let text = \"DISP\"\n    let length = move || text.len()\n    print(text)\n    print(length())\n}\n",
            "moved value `text`",
            4,
        ),
        (
            "fn main() {\n    var value = 0\n    let first = || -> int { value += 1 return value }\n    let second = || -> int { value += 1 return value }\n    print(first())\n    print(second())\n}\n",
            "second mutable borrow",
            4,
        ),
    ];
    for (source, message, line) in cases {
        let error = disp::check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, line);
    }
}

#[test]
fn malformed_closures_fail_with_parser_spans() {
    let cases = [
        (
            "fn main() {\n    let operation = |value| value\n}\n",
            "closure parameters require a type annotation",
            2,
        ),
        (
            "fn main() {\n    let operation = || { return 1 }\n}\n",
            "block closure requires an explicit return type",
            2,
        ),
    ];
    for (source, message, line) in cases {
        let error = disp::check_source(source).unwrap_err();
        assert_eq!(error.kind, disp::diagnostics::DiagnosticKind::Parse);
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, line);
    }
}

#[test]
fn nested_and_generic_closures_are_converted_differentially() {
    let source = r#"
fn apply<T>(operation: fn(T) -> T, value: T) -> T = operation(value)

fn main() {
    let double = |value: int| value * 2
    print(apply(double, 21))

    let base = 19
    let make = |offset: int| move |value: int| value + offset + base
    let add = make(2)
    print(add(21))

    let answer = || 42
    print(answer())
}
"#;
    differential("nested-generic", source);
}

#[test]
fn closure_signatures_are_checked_at_the_source_expression() {
    let cases = [
        (
            "fn main() {\n    let operation: fn(int) -> int = |value: bool| 1\n}\n",
            "binding initializer expected",
            2,
        ),
        (
            "fn main() {\n    let operation = |value: int| value + 1\n    print(operation(true))\n}\n",
            "function argument expected",
            3,
        ),
    ];
    for (source, message, line) in cases {
        let error = disp::check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, line);
    }
}

#[test]
fn callable_hidden_lifetimes_cannot_escape_through_aggregates() {
    let cases = [
        (
            "fn make(value: &int) -> fn() -> int {\n    return move || *value\n}\nfn main() {}\n",
            "cannot escape",
            2,
        ),
        (
            "struct Holder { operation: fn() -> int }\nfn make() -> Holder {\n    let value = 1\n    let operation = || value\n    return Holder { operation }\n}\nfn main() {}\n",
            "cannot escape",
            5,
        ),
        (
            "struct Holder { operation: fn() -> int }\nfn hide(operation: fn() -> int) -> Holder {\n    return Holder { operation }\n}\nfn main() {}\n",
            "cannot escape",
            3,
        ),
        (
            "fn make() -> fn() -> fn() -> int {\n    let value = 42\n    return move || || value\n}\nfn main() {}\n",
            "cannot escape through another closure",
            3,
        ),
    ];
    for (source, message, line) in cases {
        let error = disp::check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, line);
    }
}

#[test]
fn owned_returned_closures_are_safe_and_differential() {
    let source = r#"
fn make(offset: int) -> fn(int) -> int {
    return move |value: int| value + offset
}

fn main() {
    let add = make(20)
    print(add(22))
}
"#;
    differential("owned-return", source);
}

#[test]
fn closure_reassignment_transfers_and_releases_capture_loans() {
    let conflict = disp::check_source(
        "fn main() {\n    var value = 1\n    var operation: fn() -> int = move || 0\n    operation = || value\n    value = 2\n    print(operation())\n}\n",
    )
    .unwrap_err();
    assert!(
        conflict.message.contains("borrowed"),
        "{}",
        conflict.message
    );
    assert_eq!(conflict.span.start.line, 5);

    differential(
        "reassign-release",
        r#"
fn main() {
    var value = 1
    var operation: fn() -> int = || value
    operation = move || 42
    value = 2
    print(operation())
    print(value)
}
"#,
    );
}

#[test]
fn closure_loans_survive_control_flow_joins_and_loop_back_edges() {
    let cases = [
        "fn main() {\n    var value = 1\n    var operation: fn() -> int = move || 0\n    if true { operation = || value } else { operation = move || 2 }\n    value = 3\n    print(operation())\n}\n",
        "fn main() {\n    var value = 1\n    var operation: fn() -> int = move || 0\n    while false { operation = || value }\n    value = 3\n    print(operation())\n}\n",
        "fn main() {\n    var value = 1\n    var operation: fn() -> int = move || 0\n    loop {\n        value = 2\n        operation = || value\n    }\n}\n",
        "fn main() {\n    var value = 1\n    var operation: fn() -> int = move || 0\n    for index in 0..2 {\n        value = index\n        operation = || value\n    }\n}\n",
        "fn main() {\n    var value = 1\n    var operation: fn() -> int = move || 0\n    let values = [1, 2]\n    for item in values {\n        value = item\n        operation = || value\n    }\n}\n",
    ];
    for source in cases {
        let error = disp::check_source(source).unwrap_err();
        assert!(error.message.contains("borrowed"), "{}", error.message);
    }
}

#[test]
fn owned_callables_work_inside_aggregates_and_are_dropped() {
    let source = r#"
struct Handler { operation: fn(int) -> int }

fn main() {
    let handler = Handler { operation: move |value: int| value + 20 }
    print(handler.operation(22))
    print(handler.operation(22))

    let optional = Some(move |value: int| value * 2)
    print(match optional {
        Some(operation) => operation(21)
        None => 0
    })
}
"#;
    differential("aggregates", source);

    let (_, mir) = lower_source(source).unwrap();
    let main = mir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .unwrap();
    let callable_locals = main
        .locals
        .iter()
        .filter(|local| matches!(local.ty, disp::hir::Type::Function(_, _)))
        .map(|local| local.id)
        .collect::<std::collections::HashSet<_>>();
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.statements)
            .any(|statement| matches!(
                &statement.kind,
                disp::mir::StatementKind::Drop { place, .. }
                    if callable_locals.contains(&place.local)
            ))
    );
}

#[test]
fn mutable_closures_update_dynamic_places_differentially() {
    differential(
        "dynamic-place",
        r#"
fn main() {
    var values = [20, 40]
    let increment = |index: int| -> int {
        values[index] += 1
        return values[index]
    }
    print(increment(0))
    print(increment(1))
}
"#,
    );
}
