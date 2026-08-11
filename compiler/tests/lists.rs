use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-list-{name}.disp"));
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

fn native_failure(name: &str, source: &str) -> Option<String> {
    let path = std::env::temp_dir().join(format!("disp-list-fail-{name}.disp"));
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
fn list_and_str_have_concrete_target_aware_layouts() {
    let source = "fn main(){ var values: List<int> = List.new(); let text=\"x\"; let view: &str=&text; print(values.len()+(*view).len()) }";
    let (hir, _) = lower_source(source).unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let list = disp::hir::Type::List(Box::new(disp::hir::Type::Int {
        signed: true,
        width: None,
    }));
    let list_layout = layouts.layout(&list).unwrap();
    let str_layout = layouts.layout(&disp::hir::Type::Str).unwrap();
    assert_eq!(list_layout.size, 24);
    assert_eq!(list_layout.align, 8);
    assert_eq!(str_layout.size, 16);
    assert_eq!(str_layout.align, 8);
    assert_eq!(
        abi::classify(&list, &list_layout, target),
        abi::PassMode::Indirect
    );
    assert_eq!(
        abi::classify(&disp::hir::Type::Str, &str_layout, target),
        abi::PassMode::Indirect
    );
}

#[test]
fn primitive_lists_grow_mutate_borrow_and_slice() {
    let source = r#"
fn set(value: &mut int) -> int { *value = 99 return *value }
fn append<T>(values: &mut List<T>, value: T) { (*values).push(value) }
fn main() {
    var values: List<int> = List.with_capacity(1)
    values.push(10)
    values.push(30)
    append(&mut values, 40)
    print(values.pop())
    values.insert(1, 20)
    print(values.len())
    print(values.capacity())
    print(values.remove(2))
    let found = values.get(0)
    print(match found { Some(value) => *value, None => 0 })
    let editable = values.get_mut(0)
    values[1] = 21
    print(match editable { Some(value) => set(value), None => 0 })
    let mutable_view = &mut values[0..2];
    (*mutable_view)[1] = 22
    let view = &values[0..2];
    print((*view)[1])
    print(match values.pop() { Some(value) => value, None => 0 })
    print(values.is_empty())
    print(values.get(99))
    values.clear()
    print(values.is_empty())
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "Option.Some(40)",
            "3",
            "4",
            "30",
            "10",
            "99",
            "22",
            "22",
            "false",
            "Option.None",
            "true"
        ]
    );
    differential("primitive", source);
}

#[test]
fn ergonomic_list_construction_infers_storage_and_preserves_native_semantics() {
    let source = r#"
fn main() {
    var values = List.of(10, 20, 30)
    values.add(40)
    print(values.count())
    print(values.empty())
    print(values[2])

    var words = List.of("Data", "Intelligence", "System", "Page")
    words.add("DISP")
    print(words.count())
    print(words.pop())
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["4", "false", "30", "5", "Option.Some(DISP)"]
    );
    differential("ergonomic", source);
}

#[test]
fn non_copy_list_elements_move_and_drop_exactly_once() {
    let source = r#"
    fn make(value: String) -> List<String> {
        var result: List<String> = List.new()
        result.push(value)
        return result
    }
    fn main() {
        var values: List<String> = List.new()
        values.push("first")
        values.push("third")
        values.insert(1, "second")
        print(values.remove(0))
        print(match values.pop() { Some(value) => value, None => "missing" })
        values.clear()
        print(values.len())
        var inner: List<String> = List.new()
        inner.push("nested")
        var outer: List<List<String>> = List.new()
        outer.push(inner)
        outer.clear()
        var replaced = make("old")
        replaced = make("new")
        print(replaced.remove(0))
    }"#;
    assert_eq!(run_source(source).unwrap(), ["first", "third", "0", "new"]);
    differential("non-copy", source);
}

#[test]
fn list_bounds_types_and_aliases_are_rejected() {
    let wrong = check_source("fn main(){ var values: List<int> = List.new(); values.push(true) }")
        .unwrap_err();
    assert!(wrong.message.contains("List element"));

    let immutable = check_source(
        "fn main(){ let values: List<int> = List.new(); let item=values.get_mut(0); print(item) }",
    )
    .unwrap_err();
    assert!(immutable.message.contains("immutable"));

    let mismatch = check_source(
        "fn take(values: List<bool>){} fn main(){ let values: List<int> = List.new(); take(values) }",
    )
    .unwrap_err();
    assert!(mismatch.message.contains("function argument"));

    let borrowed = check_source(
        "fn main(){ var values: List<int> = List.new(); values.push(1); let view=&values[0]; values.push(2); print(*view) }",
    )
    .unwrap_err();
    assert!(borrowed.message.contains("borrow"));

    let optional_borrow = check_source(
        "fn main(){ var values: List<int> = List.new(); values.push(1); let found=values.get(0); values.push(2); print(match found { Some(value)=>*value, None=>0 }) }",
    )
    .unwrap_err();
    assert!(optional_borrow.message.contains("borrow"));

    let escape = check_source(
        "fn bad()->Option<&int>{ var values: List<int> = List.new(); values.push(1); return values.get(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(escape.message.contains("local"));

    let error = run_source(
        "fn main(){ var values: List<int> = List.new(); values.push(1); print(values[2]) }",
    )
    .unwrap_err();
    assert!(error.message.contains("out of bounds"));
    if let Some(stderr) = native_failure(
        "index",
        "fn main(){ var values: List<int> = List.new(); values.push(1); print(values[2]) }",
    ) {
        assert!(stderr.contains("out of bounds"), "{stderr}");
    }

    let insert = "fn main(){ var values: List<String> = List.new(); values.insert(1, \"x\") }";
    assert!(
        run_source(insert)
            .unwrap_err()
            .message
            .contains("out of bounds")
    );
    if let Some(stderr) = native_failure("insert", insert) {
        assert!(stderr.contains("out of bounds"), "{stderr}");
    }
}
