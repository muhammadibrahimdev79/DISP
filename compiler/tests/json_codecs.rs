use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-json-codecs-{label}-{}-{}.disp",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

fn native(name: &str, source: &str) -> String {
    let path = unique_path(name);
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    fs::remove_file(&path).unwrap();
    for _ in 0..20 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return String::from_utf8(output.stdout)
                    .unwrap()
                    .replace("\r\n", "\n");
            }
            Err(error) if error.raw_os_error() == Some(4551) => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("native JSON codec execution failed: {error}"),
        }
    }
    panic!("Windows Application Control repeatedly blocked JSON codec executable")
}

fn differential(name: &str, source: &str) -> String {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    assert_eq!(native(name, source), expected);
    expected
}

#[test]
fn nominal_struct_and_enum_codecs_are_native_interpreter_differential() {
    let source = r#"
struct Project { name: String, passes: uint, stable: bool, tags: List<String> }
enum Event { Started, Progress(uint, String) }
fn exercise() -> Result<bool, ConversionError> {
    project = Project { name: "DISP", passes: 25, stable: false, tags: List.of("safe", "native") }
    document = Json.from(project)?
    print(document.as_string())
    restored = Project.from_json(document)?
    print(restored.name)
    print(restored.tags.len())
    event = Event.Progress(25, "typed JSON")
    encoded = Json.from(event)?
    print(encoded.as_string())
    decoded = Event.from_json(encoded)?
    print(match decoded { Event.Started => "started", Event.Progress(pass, message) => message })
    return Ok(true)
}
fn main() { print(exercise()) }
"#;
    assert_eq!(
        differential("nominal", source),
        "{\"name\":\"DISP\",\"passes\":25,\"stable\":false,\"tags\":[\"safe\",\"native\"]}\nDISP\n2\n{\"Progress\":[25,\"typed JSON\"]}\ntyped JSON\nResult.Ok(true)\n"
    );
}

#[test]
fn nested_standard_data_types_round_trip() {
    let source = r#"
struct Data {
    maybe: Option<int>
    outcome: Result<bool, String>
    fixed: [u8; 3]
    labels: Map<String, int>
    symbol: char
}
fn failure() -> Result<bool, String> { return Err("retry") }
fn exercise() -> Result<bool, ConversionError> {
    value = Data {
        maybe: Some(7)
        outcome: failure()
        fixed: [u8(1), u8(2), u8(3)]
        labels: Map.of("a": 10, "b": 20)
        symbol: 'λ'
    }
    encoded = Json.from(value)?
    print(encoded.as_string())
    decoded = Data.from_json(encoded)?
    print(decoded.maybe)
    print(decoded.outcome)
    print(decoded.fixed[2])
    print(decoded.labels.len())
    print(decoded.symbol)
    return Ok(true)
}
fn main() { print(exercise()) }
"#;
    let output = differential("nested", source);
    assert!(output.contains("\"maybe\":7"));
    assert!(output.contains("\"outcome\":{\"Err\":\"retry\"}"));
    assert!(output.ends_with("Option.Some(7)\nResult.Err(retry)\n3\n2\nλ\nResult.Ok(true)\n"));
}

#[test]
fn strict_decoding_rejects_schema_and_range_mismatches() {
    let source = r#"
struct Byte { value: u8 }
enum Signal { Ready, Value(int) }
fn check() -> Result<bool, ConversionError> {
    print(match Byte.from_json(Json("{}")?) { Ok(value) => false, Err(error) => true })
    print(match Byte.from_json(Json("{\"value\":1,\"extra\":2}")?) { Ok(value) => false, Err(error) => true })
    print(match Byte.from_json(Json("{\"value\":256}")?) { Ok(value) => false, Err(error) => true })
    print(match Signal.from_json(Json("\"Unknown\"")?) { Ok(value) => false, Err(error) => true })
    print(match Signal.from_json(Json("{\"Value\":[]}")?) { Ok(value) => false, Err(error) => true })
    return Ok(true)
}
fn main() { check() }
"#;
    assert_eq!(
        differential("invalid", source),
        "true\ntrue\ntrue\ntrue\ntrue\n"
    );
}

#[test]
fn wide_numbers_and_concrete_nested_generics_round_trip() {
    let source = r#"
struct Box<T> { value: T }
struct Wide {
    signed: i128
    unsigned: u128
    small: f32
    precise: f64
    boxed: Box<String>
}
enum Message { Text(String), Empty }
fn exercise() -> Result<bool, ConversionError> {
    value = Wide {
        signed: -170141183460469231731687303715884105728
        unsigned: 340282366920938463463374607431768211455
        small: f32(1.25)
        precise: 2.5
        boxed: Box { value: "nested" }
    }
    encoded = Json.from(value)?
    print(encoded.as_string())
    decoded = Wide.from_json(encoded)?
    print(decoded.signed)
    print(decoded.unsigned)
    print(decoded.small)
    print(decoded.precise)
    print(decoded.boxed.value)
    message = Message.Text("hello")
    restored = Message.from_json(Json.from(message)?)?
    print(match restored { Message.Text(text) => text, Message.Empty => "empty" })
    return Ok(true)
}
fn main() { print(exercise()) }
"#;
    assert_eq!(
        differential("wide-generics", source),
        "{\"signed\":-170141183460469231731687303715884105728,\"unsigned\":340282366920938463463374607431768211455,\"small\":1.25,\"precise\":2.5,\"boxed\":{\"value\":\"nested\"}}\n-170141183460469231731687303715884105728\n340282366920938463463374607431768211455\n1.25\n2.5\nnested\nhello\nResult.Ok(true)\n"
    );
}

#[test]
fn unsupported_codec_types_and_bad_calls_have_source_diagnostics() {
    let invalid = [
        (
            "fn main(){ value=Mutex.new(1) encoded=Json.from(value) }",
            "Mutex<Int> cannot be used with automatic JSON conversion",
        ),
        (
            "struct Generic<T>{ value:T } fn main(){ value=Json.null() decoded=Generic.from_json(value) }",
            "generic nominal JSON decoding requires a concrete wrapper type",
        ),
        (
            "struct Item { value:int } fn main(){ decoded=Item.from_json(1) }",
            "JSON source",
        ),
        (
            "fn main(){ encoded=Json.from() }",
            "Json.from` received the wrong number of arguments",
        ),
        (
            "fn main(){ let value: Option<Json> = None encoded=Json.from(value) }",
            "Option<Json> cannot be used with automatic JSON conversion",
        ),
    ];
    for (source, message) in invalid {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, 1);
        assert!(error.span.start.column > 0);
    }
}
