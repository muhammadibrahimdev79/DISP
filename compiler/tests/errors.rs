use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, mir, ownership, run_source,
};
use std::{fs, path::PathBuf, process::Command};

fn reject(source: &str, expected: &str) {
    let error = check_source(source).expect_err("program should be rejected");
    assert!(
        error.message.contains(expected),
        "expected `{expected}` in `{}`",
        error.message
    );
    assert!(error.span.start.line > 0 && error.span.start.column > 0);
}

fn try_run_native(path: &std::path::Path) -> Result<std::process::Output, std::io::Error> {
    let mut last = None;
    for _ in 0..4 {
        match Command::new(path).output() {
            Ok(output) => return Ok(output),
            Err(error) if error.raw_os_error() == Some(4551) => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("application policy failure should be retained"))
}

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let directory = std::env::temp_dir().join(format!("disp-errors-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path: PathBuf = directory.join(format!("{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c: true,
            emit_object: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let output = match try_run_native(&artifacts.executable) {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("could not execute native error test: {error}"),
    };
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        expected
    );
}

#[test]
fn typed_result_propagation_is_exact_and_differential() {
    let source = r#"
enum Failure { Message(String) }
fn source(ok: bool) -> Result<String, Failure> {
    print("source")
    if ok { return Ok("value") }
    return Err(Failure.Message("failed"))
}
fn layer(ok: bool) -> Result<String, Failure> {
    let prefix = "kept"
    let value = source(ok)?
    print(prefix)
    return Ok(value)
}
fn render(value: Result<String, Failure>) -> String {
    return match value {
        Ok(text) => text
        Err(Failure.Message(message)) => message
    }
}
fn main() {
    print(render(layer(true)))
    print(render(layer(false)))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["source", "kept", "value", "source", "failed"]
    );
    differential("typed-propagation", source);
}

#[test]
fn question_rejects_carrier_and_error_type_mismatches() {
    reject(
        "enum A { Bad } enum B { Bad } fn source() -> Result<int, A> { return Err(A.Bad) } fn bad() -> Result<int, B> { return Ok(source()?) } fn main() {}",
        "propagated error expected",
    );
    reject(
        "fn source() -> Option<int> { return None } fn bad() -> Result<int, String> { return Ok(source()?) } fn main() {}",
        "requires the enclosing function to return Option",
    );
    reject(
        "fn source() -> Result<int, String> { return Err(\"bad\") } fn bad() -> Option<int> { return Some(source()?) } fn main() {}",
        "requires the enclosing function to return Result",
    );
}

#[test]
fn propagation_records_reverse_lexical_cleanup() {
    let source = r#"
struct Resource { name: String }
fn source() -> Result<int, String> { return Err("failed") }
fn work() -> Result<int, String> {
    let outer = Resource { name: "outer" }
    if true {
        let inner = Resource { name: "inner" }
        let value = source()?
        return Ok(value)
    }
    return Err("unreachable")
}
fn main() { print(work()) }
"#;
    let program = check_source(source).unwrap();
    let report = ownership::check(&program).unwrap();
    let propagated = report
        .drops
        .iter()
        .filter(|drop| drop.reason == ownership::DropReason::Propagation)
        .map(|drop| drop.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(propagated, ["inner", "outer"]);
}

#[test]
fn mir_failure_edge_moves_error_before_exactly_once_cleanup() {
    let source = r#"
struct Resource { name: String }
fn source() -> Result<int, String> { return Err("failed") }
fn work() -> Result<int, String> {
    let outer = Resource { name: "outer" }
    let inner = Resource { name: "inner" }
    let value = source()?
    return Ok(value)
}
fn main() { print(work()) }
"#;
    let (_, program) = lower_source(source).unwrap();
    let function = program
        .functions
        .iter()
        .find(|function| function.name == "work")
        .unwrap();
    let failure = function
        .blocks
        .iter()
        .find_map(|block| match block.terminator {
            mir::Terminator::SwitchEnum { otherwise, .. } => Some(otherwise),
            _ => None,
        })
        .expect("`?` must create a failure edge");
    let statements = &function.blocks[failure.0].statements;
    let return_assignment = statements
        .iter()
        .position(|statement| {
            matches!(
                &statement.kind,
                mir::StatementKind::Assign(place, mir::Rvalue::Use(mir::Operand::Move(_)))
                    if place.local == function.return_local
            )
        })
        .expect("the propagated carrier must move into the return place");
    let user_drops = statements
        .iter()
        .enumerate()
        .filter_map(|(index, statement)| match &statement.kind {
            mir::StatementKind::Drop { place, .. }
                if function.locals[place.local.0].kind == mir::LocalKind::User =>
            {
                Some((index, function.locals[place.local.0].name.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        user_drops.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
        ["inner", "outer"]
    );
    assert!(
        user_drops
            .iter()
            .all(|(index, _)| *index > return_assignment)
    );
}

#[test]
fn partially_moved_storage_cleans_only_its_initialized_remainder() {
    let source = r#"
struct Pair { kept: String, taken: String }
fn source() -> Result<int, String> { return Err("failed") }
fn work() -> Result<int, String> {
    let pair = Pair { kept: "kept", taken: "taken" }
    let taken = move pair.taken
    let value = source()?
    print(taken)
    return Ok(value)
}
fn main() { print(work()) }
"#;
    let program = check_source(source).unwrap();
    let report = ownership::check(&program).unwrap();
    let propagated = report
        .drops
        .iter()
        .filter(|drop| drop.reason == ownership::DropReason::Propagation)
        .map(|drop| drop.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(propagated, ["taken", "pair"]);

    let (_, mir) = lower_source(source).unwrap();
    let function = mir
        .functions
        .iter()
        .find(|function| function.name == "work")
        .unwrap();
    let dump = disp::mir::dump(&mir);
    assert!(function.locals.iter().any(|local| local.name == "pair"));
    assert!(dump.contains("SetDropFlag"));
}

#[test]
fn typed_propagation_composes_through_closures_and_async() {
    let source = r#"
enum Failure { Message(String) }
fn immediate(ok: bool) -> Result<int, Failure> {
    if ok { return Ok(5) }
    return Err(Failure.Message("closure"))
}
async fn delayed(ok: bool) -> Result<int, Failure> {
    Async.yield()
    if ok { return Ok(7) }
    return Err(Failure.Message("async"))
}
fn closure_value(ok: bool) -> Result<int, Failure> {
    let calculate = |value: bool| -> Result<int, Failure> {
        let number = immediate(value)?
        return Ok(number * 2)
    }
    return calculate(ok)
}
async fn async_value(ok: bool) -> Result<int, Failure> {
    let pending = delayed(ok)
    let ready = await pending
    let number = ready?
    return Ok(number * 3)
}
async fn main() {
    print(match closure_value(true) { Ok(value) => value Err(_) => 0 })
    print(match closure_value(false) { Ok(value) => value Err(_) => 1 })
    print(match await async_value(true) { Ok(value) => value Err(_) => 0 })
    print(match await async_value(false) { Ok(value) => value Err(_) => 1 })
}
"#;
    assert_eq!(run_source(source).unwrap(), ["10", "1", "21", "1"]);
    differential("closure-async-propagation", source);
}
