use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-ergonomics-{name}.disp"));
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
fn concise_functions_are_only_surface_sugar() {
    let source = r#"
fn double(value: int) -> int = value * 2
fn label(ok: bool) -> String = match ok { true => "ready", false => "waiting" }
fn main() {
    print(double(21))
    print(label(true))
}
"#;
    assert_eq!(run_source(source).unwrap(), ["42", "ready"]);
    let (_, mir) = lower_source(source).unwrap();
    assert!(disp::mir::dump(&mir).contains("return"));
    differential("concise-functions", source);
}

#[test]
fn concise_unit_functions_are_rejected_with_an_exact_span() {
    let error = check_source("fn bad() = 1\nfn main() {}").unwrap_err();
    assert!(error.message.contains("explicit return type"));
    assert_eq!(error.span.start.line, 1);
    assert_eq!(error.span.start.column, 10);
}

#[test]
fn plain_language_string_and_collection_operations_are_differential() {
    let source = r#"
fn main() {
    var title = "Data"
    title.append(" Intelligence")
    title.add(" System Page")
    print(title)

    var parts = List.of("Data", "Intelligence")
    parts.add("System")
    parts.add("Page")
    print(parts.count())
    print(parts.empty())
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["Data Intelligence System Page", "4", "false"]
    );
    differential("plain-operations", source);
}

#[test]
fn collection_iteration_is_safe_simple_and_zero_copy_for_owned_values() {
    let source = r#"
fn main() {
    let numbers = [10, 20, 30]
    let number_view = numbers.slice(0, 3)
    var total = 0
    for number in number_view {
        total += number
    }
    for number in List.of(1, 2) {
        total += number
    }
    print(total)

    let words = List.of("Data", "Intelligence", "System", "Page")
    for word in words {
        print(word)
    }
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["63", "Data", "Intelligence", "System", "Page"]
    );
    let (_, mir) = lower_source(source).unwrap();
    let dump = disp::mir::dump(&mir);
    assert!(dump.contains("Len(") && dump.contains("Index {"), "{dump}");
    differential("collection-for", source);
}

#[test]
fn collection_iteration_holds_a_shared_loan() {
    let error = check_source(
        "fn main(){ var values=List.of(1,2,3); for value in values { values.add(4); print(value) } }",
    )
    .unwrap_err();
    assert!(error.message.contains("borrow"), "{}", error.message);
}

#[test]
fn first_assignment_declares_an_inferred_type_checked_variable() {
    let source = r#"
fn main() {
    message = "Hello"
    message.append(" from DISP")
    count = 2
    count += 3
    values = List.of(10, 20)
    values.add(30)
    print(message)
    print(count)
    print(values.count())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["Hello from DISP", "5", "3"]);
    differential("inferred-assignment", source);

    let mismatch = check_source("fn main(){ value=1; value=true }").unwrap_err();
    assert!(mismatch.message.contains("assignment"));

    let compound = check_source("fn main(){ missing += 1 }").unwrap_err();
    assert!(compound.message.contains("unknown"));
}

#[test]
fn data_construction_supports_safe_field_shorthand() {
    let source = r#"
struct Project { name: String, passes: int }
fn project(name: String, passes: int) -> Project = Project { name, passes }
fn main() {
    current = project("DISP", 3)
    print(current.name)
    print(current.passes)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["DISP", "3"]);
    differential("data-shorthand", source);
}

#[test]
fn shared_function_arguments_borrow_implicitly_but_mutable_borrows_stay_explicit() {
    let source = r#"
fn size<T>(values: &List<T>) -> uint = (*values).len()
fn show(value: &String) { print(*value) }
fn main() {
    values = List.of(10, 20, 30)
    text = "simple borrowing"
    print(size(values))
    show(text)
    values.add(40)
    print(values.count())
}
"#;
    assert_eq!(run_source(source).unwrap(), ["3", "simple borrowing", "4"]);
    differential("implicit-shared-borrow", source);

    let overlap = check_source(
        "fn inspect(values:&List<int>, changed:Unit){} fn main(){ values=List.of(1,2); inspect(values, values.add(3)) }",
    )
    .unwrap_err();
    assert!(overlap.message.contains("borrow"), "{}", overlap.message);

    let mutable = check_source(
        "fn edit(values:&mut List<int>){} fn main(){ values=List.of(1,2); edit(values) }",
    )
    .unwrap_err();
    assert!(mutable.message.contains("function argument"));
}
