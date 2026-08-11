use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn rejects(source: &str, message: &str) {
    let error = check_source(source).expect_err("program must be rejected");
    assert!(error.message.contains(message), "{}", error.message);
}

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-dynamic-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact =
        backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap_or_else(|error| {
            panic!("{error}\n{}", disp::mir::dump(&mir));
        });
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        expected
    );
}

fn native_failure(name: &str, source: &str) -> Option<String> {
    let path = std::env::temp_dir().join(format!("disp-dynamic-fail-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return None,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(!output.status.success());
    Some(String::from_utf8(output.stderr).unwrap())
}

#[test]
fn indexed_reads_writes_nested_access_and_exactly_once_evaluation() {
    let source = r#"
fn next(counter: &mut int) -> int { *counter += 1 return 1 }
fn main() {
    var counter = 0
    var values = [[1, 2], [3, 4]]
    values[next(&mut counter)][0] = 9
    print(values[1][0])
    print(counter)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["9", "1"]);
    differential("index", source);
}

#[test]
fn subslices_are_bounds_checked_and_mutable_views_update_the_owner() {
    let source = r#"fn main() {
        var values = [10, 20, 30, 40]
        let view = &mut values[1..3];
        (*view)[0] = 88
        print((*view).len())
        print(values[1])
    }"#;
    assert_eq!(run_source(source).unwrap(), ["2", "88"]);
    differential("subslice", source);

    let error = run_source("fn main() { let a=[1,2] let x=a[1..3] print(x.len()) }")
        .expect_err("invalid range must fail at runtime");
    assert!(error.message.contains("out of bounds"));
}

#[test]
fn loan_regions_accept_disjoint_constants_and_reject_possible_aliases() {
    check_source("fn main() { var a=[1,2]; let x=&mut a[0]; let y=&mut a[1]; *x=3; *y=4 }")
        .unwrap();
    check_source("fn main() { var a=[1,2,3,4]; let x=&mut a[0..2]; let y=&mut a[2..4]; (*x)[0]=3; (*y)[0]=4 }").unwrap();

    rejects(
        "fn main() { var a=[1,2]; let i=0; let j=1; let x=&mut a[i]; let y=&mut a[j]; print(*x+*y) }",
        "second mutable borrow",
    );
    rejects(
        "fn main() { var a=[1,2,3]; let x=&a[0]; let y=&mut a[0]; print(*x+*y) }",
        "shared and mutable",
    );
    rejects(
        "fn main() { var a=[1,2,3,4]; let x=&mut a[0..3]; let y=&mut a[2..4]; print((*x)[0]+(*y)[0]) }",
        "second mutable borrow",
    );
}

#[test]
fn indexed_loans_block_owner_mutation_moves_and_lifetime_escape() {
    rejects(
        "fn main() { var a=[1,2]; let x=&a[0]; a[0]=3; print(*x) }",
        "borrowed value",
    );
    rejects(
        "fn main() { let a=[String.new(),String.new()]; let x=&a[0]; let moved=a; print(x) }",
        "borrowed value",
    );
    rejects(
        "fn bad() -> &[int] { let a=[1,2]; return &a[0..2] } fn main() {}",
        "cannot return a reference to local",
    );
    rejects(
        "fn main() { let a=[1,2]; let x=&mut a[0] print(*x) }",
        "immutable",
    );
}

#[test]
fn invalid_indices_have_source_diagnostics() {
    let error = check_source("fn main() { let a=[1,2]\n print(a[true]) }").unwrap_err();
    assert!(error.message.contains("index must be an integer"));
    assert_eq!(error.span.start.line, 2);
}

#[test]
fn runtime_bounds_failures_match_and_report_the_projection_source() {
    let source = "fn main() {\n let a=[1,2]\n print(a[3])\n}";
    let interpreted = run_source(source).unwrap_err();
    assert!(interpreted.message.contains("out of bounds"));
    assert_eq!(interpreted.span.start.line, 3);
    if let Some(stderr) = native_failure("index-oob", source) {
        assert!(stderr.contains("out of bounds"), "{stderr}");
        assert!(stderr.contains("3:"), "{stderr}");
    }
}

#[test]
fn dynamic_places_work_across_calls_loops_and_nll_boundaries() {
    let source = r#"
fn first<T>(values: &[T]) -> &T { return &(*values)[0] }
fn main() {
    var values = [4, 5, 6]
    let view = &values[0..2]
    print(*first(view))
    for i in 0..3 { values[i] += 1 }
    let selected = &values[2]
    print(*selected)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["4", "7"]);
    differential("calls-loops", source);
}

#[test]
fn moving_non_copy_dynamic_elements_is_rejected_safely() {
    rejects(
        "fn main() { let a=[String.new(),String.new()]; let i=0; let x=a[i]; print(x) }",
        "cannot move a non-Copy element through dynamic indexing",
    );
}

#[test]
fn ir_contains_first_class_dynamic_projections_and_single_operand_temps() {
    let source = "fn next(x: &mut int)->int { *x += 1 return 0 } fn main(){ var n=0; let a=[1,2]; print(a[next(&mut n)]); print(a[0..next(&mut n)]) }";
    let (hir, mir) = lower_source(source).unwrap();
    let hir_dump = disp::hir::dump(&hir);
    let mir_dump = disp::mir::dump(&mir);
    assert!(hir_dump.contains("Index") && hir_dump.contains("Subslice"));
    assert!(mir_dump.contains("Index") && mir_dump.contains("Subslice"));
    assert!(mir_dump.contains("FunctionId(0)"));
}

#[test]
fn mir_validation_rejects_invalid_dynamic_projection_identity_and_mutability() {
    let (_, mut program) = lower_source("fn main(){ var a=[1,2]; let i=0; a[i]=3 }").unwrap();
    let main = program
        .functions
        .iter_mut()
        .find(|function| function.name == "main")
        .unwrap();
    let projection = main
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.statements)
        .find_map(|statement| match &mut statement.kind {
            disp::mir::StatementKind::Assign(place, _) => place
                .projections
                .iter_mut()
                .find(|projection| matches!(projection, disp::mir::Projection::Index { .. })),
            _ => None,
        })
        .unwrap();
    if let disp::mir::Projection::Index { index, .. } = projection {
        *index = disp::mir::LocalId(999);
    }
    let error = disp::mir::validate(&program).unwrap_err();
    assert!(error.message.contains("invalid local"), "{error}");

    let (_, mut program) = lower_source("fn main(){ var a=[1,2]; let x=&mut a[0]; *x=3 }").unwrap();
    let main = program
        .functions
        .iter_mut()
        .find(|function| function.name == "main")
        .unwrap();
    let owner = main
        .locals
        .iter_mut()
        .find(|local| local.name == "a")
        .unwrap();
    owner.mutable = false;
    let error = disp::mir::validate(&program).unwrap_err();
    assert!(error.message.contains("immutable base"), "{error}");
}
