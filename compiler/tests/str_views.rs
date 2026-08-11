use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn native(name: &str, source: &str) -> Option<std::process::Output> {
    let path = std::env::temp_dir().join(format!("disp-str-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    match Command::new(artifact.executable).output() {
        Ok(output) => Some(output),
        Err(error) if error.raw_os_error() == Some(4551) => None,
        Err(error) => panic!("native execution failed: {error}"),
    }
}

#[test]
fn utf8_str_views_slice_without_copying() {
    let source = r#"
    fn inspect(value: &str) { print(*value) }
    fn as_str(value: &String) -> &str { return value }
    fn main() {
        let text = "héllo DISP"
        inspect(&text)
        print(*as_str(&text))
        let word: &str = &text[0..6]
        let tail: &str = &(*word)[1..6]
        print(*word)
        print(*tail)
        print((*word).len())
        print((*word).contains("éll"))
        print((*word).starts_with("hé"))
        print((*word).ends_with("lo"))
    }"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    assert_eq!(
        expected,
        "héllo DISP\nhéllo DISP\nhéllo\néllo\n6\ntrue\ntrue\ntrue\n"
    );
    if let Some(output) = native("utf8", source) {
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
}

#[test]
fn str_rejects_invalid_utf8_boundaries_mutation_and_escape() {
    let source = "fn main(){ let text=\"éx\"; let invalid: &str=&text[0..1]; print(*invalid) }";
    let error = run_source(source).unwrap_err();
    assert!(error.message.contains("UTF-8"), "{}", error.message);
    if let Some(output) = native("boundary", source) {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("UTF-8"));
    }

    let mutation = check_source(
        "fn main(){ var text=\"hello\"; let view: &str=&text[0..2]; text.push('!'); print(*view) }",
    )
    .unwrap_err();
    assert!(mutation.message.contains("borrow"));

    let mutable =
        check_source("fn main(){ var text=\"hello\"; let view=&mut text[0..2] }").unwrap_err();
    assert!(mutable.message.contains("immutable UTF-8"));

    let bare = check_source("fn main(){ let text=\"hello\"; let view=text[0..2] }").unwrap_err();
    assert!(bare.message.contains("held through `&str`"));

    let annotated =
        check_source("fn main(){ let text=\"hello\"; let view: str=text[0..2] }").unwrap_err();
    assert!(annotated.message.contains("must be written as `&str`"));

    let mutable_annotation =
        check_source("fn main(){ let text=\"hello\"; let view: &mut str=&mut text }").unwrap_err();
    assert!(
        mutable_annotation
            .message
            .contains("must be written as `&str`")
    );

    let escape =
        check_source("fn bad()->&str { let text=\"hello\"; return &text[0..2] } fn main(){}")
            .unwrap_err();
    assert!(escape.message.contains("local"));

    let reslice = check_source(
        "fn main(){ var text=\"hello\"; let view: &str=&text; let sub: &str=&(*view)[0..2]; text.push('!'); print(*sub) }",
    )
    .unwrap_err();
    assert!(reslice.message.contains("borrow"));
}
