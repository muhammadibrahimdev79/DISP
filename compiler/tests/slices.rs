use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-slice-{name}.disp"));
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
fn shared_and_mutable_slices_cross_generic_function_boundaries() {
    let source = r#"
fn first<T>(values: &[T]) -> &T { return &(*values)[0] }
fn replace(values: &mut [int], value: int) { (*values)[0] = value }
fn main() {
    var numbers = [10, 20, 30, 40]
    let middle = &mut numbers[1..3];
    replace(middle, 25)
    print((*middle).len())
    print((*middle).is_empty())
    print(*first(middle))
    let tail = &numbers[2..4];
    print((*tail)[1])

    let words = ["Data", "Intelligence"]
    let word_view = &words[0..2];
    print(*first(word_view))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["2", "false", "25", "40", "Data"]
    );
    differential("functions", source);
}

#[test]
fn slice_lifetimes_mutability_aliases_and_types_are_enforced() {
    for (source, expected) in [
        (
            "fn bad()->&[int]{ let values=[1,2]; return &values[0..2] } fn main(){}",
            "local",
        ),
        (
            "fn main(){ var values=[1,2,3]; let view=&values[0..2]; values[0]=4; print((*view)[0]) }",
            "borrow",
        ),
        (
            "fn main(){ var values=[1,2,3]; let shared=&values[0..2]; let unique=&mut values[1..3]; print((*shared)[0]+(*unique)[0]) }",
            "overlap",
        ),
        (
            "fn take(values: &[bool]){} fn main(){ let values=[1,2]; take(&values[0..2]) }",
            "function argument",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn ergonomic_slice_methods_preserve_zero_copy_borrowing() {
    let source = r#"
fn main() {
    var values = [10, 20, 30, 40]
    let middle = values.slice_mut(1, 3);
    (*middle)[0] = 25
    print((*middle)[0])
    let tail = values.slice(2, 4);
    print((*tail).len())

    let text = "Data Intelligence System Page"
    let word = text.slice(5, 17)
    print(*word)
}
"#;
    assert_eq!(run_source(source).unwrap(), ["25", "2", "Intelligence"]);
    differential("ergonomic-methods", source);
}
