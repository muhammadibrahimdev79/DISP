use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-collections-{name}.disp"));
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
fn map_and_set_have_concrete_target_aware_layouts() {
    let (hir, _) = lower_source("fn main(){ let map: Map<int,bool> = Map.new(); let set: Set<int> = Set.new(); print(map.len()+set.len()) }").unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let int = disp::hir::Type::Int {
        signed: true,
        width: None,
    };
    let map = disp::hir::Type::Map(Box::new(int.clone()), Box::new(disp::hir::Type::Bool));
    let set = disp::hir::Type::Set(Box::new(int));
    let map_layout = layouts.layout(&map).unwrap();
    let set_layout = layouts.layout(&set).unwrap();
    assert_eq!((map_layout.size, map_layout.align), (40, 8));
    assert_eq!((set_layout.size, set_layout.align), (32, 8));
    assert_eq!(
        abi::classify(&map, &map_layout, target),
        abi::PassMode::Indirect
    );
    assert_eq!(
        abi::classify(&set, &set_layout, target),
        abi::PassMode::Indirect
    );
}

#[test]
fn map_construction_updates_queries_removal_and_clear_are_differential() {
    let source = r#"
fn main() {
    var scores = Map.of("Ali": 95, "Sara": 88, "Ali": 96)
    print(scores.len())
    print(scores.has("Ali"))
    print(scores.has("Noor"))
    print(scores.set("Sara", 90))
    print(scores.set("Mina", 100))
    print(scores.remove("Ali"))
    print(scores.remove("Ali"))
    print(scores.len())
    scores.clear()
    print(scores.empty())
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "2",
            "true",
            "false",
            "Option.Some(88)",
            "Option.None",
            "Option.Some(96)",
            "Option.None",
            "2",
            "true"
        ]
    );
    differential("map", source);
}

#[test]
fn map_value_borrows_and_non_copy_query_keys_are_safe_and_differential() {
    let source = r#"
fn replace(value: &mut int) -> int { *value = 42 return *value }
fn main() {
    var map = Map.of("answer": 10)
    key = "answer"
    print(map.has(key))
    print(key)
    found = map.get(key)
    print(match found { Some(value) => *value, None => 0 })
    editable = map.get_mut(key)
    print(match editable { Some(value) => replace(value), None => 0 })
    print(match map.get(key) { Some(value) => *value, None => 0 })
    print(map.remove(key))
    print(key)
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "true",
            "answer",
            "10",
            "42",
            "42",
            "Option.Some(42)",
            "answer"
        ]
    );
    differential("map-borrows", source);
}

#[test]
fn set_uniqueness_mutation_and_foreach_are_differential() {
    let source = r#"
fn main() {
    var values = Set.of(2, 3, 2, 5)
    print(values.len())
    print(values.add(3))
    print(values.add(7))
    print(values.remove(2))
    print(values.has(5))
    var total = 0
    for value in values { total += value }
    print(total)
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["3", "false", "true", "true", "true", "15"]
    );
    differential("set", source);
}

#[test]
fn unified_borrowed_iteration_views_are_differential() {
    let source = r#"
fn main() {
    scores = Map.of(1: 10, 2: 20)
    var keys = 0
    for key in scores.keys() { keys += key }
    var values = 0
    for value in scores.values() { values += value }
    print(keys)
    print(values)
    list = List.of(4, 5)
    for value in list.iter() { print(value) }
    array = [6, 7]
    for value in array.iter() { print(value) }
    set = Set.of(8, 9)
    for value in set.iter() { print(value) }
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["3", "30", "4", "5", "6", "7", "8", "9"]
    );
    differential("iteration-views", source);

    let conflict = check_source(
        "fn main(){ var map=Map.of(1:10); for key in map.keys(){ map.set(2,20); print(key) } }",
    )
    .unwrap_err();
    assert!(conflict.message.contains("borrow"));
}

#[test]
fn collection_type_errors_and_borrow_conflicts_have_source_spans() {
    let mixed = check_source("fn main(){ let values=Map.of(1: true, false: false) }").unwrap_err();
    assert!(mixed.message.contains("Map key"));
    assert_eq!(mixed.span.start.line, 1);

    let odd = check_source("fn main(){ let values=Map.of(1) }").unwrap_err();
    assert!(odd.message.contains("expected `:`"));

    let invalid_key =
        check_source("struct Key { value: int } fn main(){ let values=Set.of(Key { value: 1 }) }")
            .unwrap_err();
    assert!(invalid_key.message.contains("Set elements"));

    let borrowed = check_source("fn main(){ var values=Map.of(1: 10); let item=values.get(1); values.set(2,20); print(item) }").unwrap_err();
    assert!(borrowed.message.contains("borrow"));
}
