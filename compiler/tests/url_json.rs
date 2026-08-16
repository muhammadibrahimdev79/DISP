use disp::{
    backend::{
        self, BuildOptions,
        abi::{self, PassMode},
        layout::LayoutEngine,
        target::Target,
    },
    check_source, lower_source, run_source,
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-url-json-{label}-{}-{}.disp",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

fn native(name: &str, source: &str, emit_c: bool) -> (String, Option<String>) {
    let path = unique_path(name);
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    let generated = artifacts
        .backend_ir
        .map(|path| fs::read_to_string(path).unwrap());
    for _ in 0..20 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return (
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .replace("\r\n", "\n"),
                    generated,
                );
            }
            Err(error) if error.raw_os_error() == Some(4551) => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("native URL/JSON execution failed: {error}"),
        }
    }
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!(
            "disp-url-json-launch-{}-{}.exe",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
    fs::copy(&artifacts.executable, &fallback)
        .unwrap_or_else(|error| panic!("could not stage URL/JSON launch fallback: {error}"));
    let output = Command::new(&fallback)
        .output()
        .unwrap_or_else(|error| panic!("native URL/JSON fallback execution failed: {error}"));
    fs::remove_file(&fallback).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        generated,
    )
}

fn one_request_server() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&chunk[..count]);
        }
        let header_end = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while request.len() < header_end + length {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&chunk[..count]);
        }
        assert!(headers.starts_with("POST /api HTTP/1.1"));
        assert!(headers.contains("Content-Type: application/json"));
        assert_eq!(
            &request[header_end..header_end + length],
            br#"{"safe":true}"#
        );
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 23\r\nConnection: close\r\n\r\n{\"accepted\":true,\"n\":2}",
            )
            .unwrap();
    });
    (port, handle)
}

fn http_source(port: u16) -> String {
    format!(
        r#"async fn send(url: Url, document: Json) -> Result<bool, HttpError> {{
response = (await Http.post_json(url, document))?
parsed = response.json()?
print(parsed.kind())
print(parsed.is_object())
return Ok(parsed.as_string() == "{{\"accepted\":true,\"n\":2}}")
}}
async fn main() {{
match Url("http://127.0.0.1:{port}/api") {{
    Ok(url) => match Json("{{\"safe\":true}}") {{
        Ok(document) => print(await send(url, document))
        Err(error) => print(false)
    }}
    Err(error) => print(false)
}}
}}"#
    )
}

fn builder_source(port: u16) -> String {
    format!(
        r#"async fn send(document: Json) -> Result<bool, HttpError> {{
request = Http.request("POST", "http://127.0.0.1:{port}/api")?
request = request.json(document)?
response = (await request.send())?
parsed = response.json()?
return Ok(parsed.is_object())
}}
async fn main() {{
match Json("{{\"safe\":true}}") {{
    Ok(document) => print(await send(document))
    Err(error) => print(false)
}}
}}"#
    )
}

#[test]
fn structured_urls_and_validated_json_are_native_interpreter_differential() {
    let source = r#"fn inspect() -> Result<bool, NetworkError> {
url = Url("https://example.com:8443/api/items?limit=10")?
print(url.scheme())
print(url.host())
print(url.port())
print(url.path())
print(url.query())
print(url.is_secure())
joined = url.join_path("year reports/λ")?
print(joined.as_string())
queried = joined.query_param("sort by", "name&date")?
print(queried.as_string())
print(match url.join_path("..") { Ok(value) => false, Err(error) => true })
print(match url.query_param("", "value") { Ok(value) => false, Err(error) => true })
var huge = String()
for index in 0..8200 { huge.push('a') }
print(match url.join_path(huge) { Ok(value) => false, Err(error) => true })
spelled = Url("HTTPS://EXAMPLE.COM:443")?
print(spelled.scheme())
print(spelled.host())
print(spelled.port())
print(spelled.path())
print(spelled.is_secure())
return Ok(url.as_string() == "https://example.com:8443/api/items?limit=10")
}
fn main() {
document = Json("{\"name\":\"DISP\",\"safe\":true,\"passes\":[22,23]}")
print(match document { Ok(value) => value.kind() == "object" && value.is_object(), Err(error) => false })
print(inspect())
}"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native("values", source, true);
    assert_eq!(actual, expected);
    let generated = generated.unwrap();
    assert!(generated.contains("disp_native_url"));
    assert!(generated.contains("disp_native_json"));
    assert!(generated.contains("disp_json_parse"));
    assert!(generated.contains("JSON nesting exceeds 128 levels"));
    assert!(generated.contains("disp_url_join_path"));
    assert!(generated.contains("disp_url_query_param"));
    assert!(
        expected.contains("https://example.com:8443/api/items/year%20reports%2F%CE%BB?limit=10\n")
    );
    assert!(expected.contains(
        "https://example.com:8443/api/items/year%20reports%2F%CE%BB?limit=10&sort%20by=name%26date\n"
    ));
}

#[test]
fn url_and_json_have_concrete_owned_native_layouts() {
    let (program, _) = lower_source(
        "fn main() { url = Url(\"https://example.com\") json = Json(\"null\") print(url) print(json) }",
    )
    .unwrap();
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &program);
    for ty in [disp::hir::Type::Url, disp::hir::Type::Json] {
        let layout = layouts.layout(&ty).unwrap();
        assert_eq!((layout.size, layout.align), (24, 8));
        assert_eq!(abi::classify(&ty, &layout, target), PassMode::Indirect);
    }
}

#[test]
fn json_http_bodies_and_responses_are_native_interpreter_differential() {
    let (port, server) = one_request_server();
    assert_eq!(
        run_source(&http_source(port)).unwrap(),
        ["object", "true", "Result.Ok(true)"]
    );
    server.join().unwrap();

    let (port, server) = one_request_server();
    let (output, generated) = native("http", &http_source(port), true);
    assert_eq!(output, "object\ntrue\nResult.Ok(true)\n");
    server.join().unwrap();
    let generated = generated.unwrap();
    assert!(generated.contains("Content-Type: application/json"));
    assert!(generated.contains("disp_http_response_json"));

    let (port, server) = one_request_server();
    assert_eq!(
        run_source(&builder_source(port)).unwrap(),
        ["Result.Ok(true)"]
    );
    server.join().unwrap();

    let (port, server) = one_request_server();
    assert_eq!(
        native("builder", &builder_source(port), false).0,
        "Result.Ok(true)\n"
    );
    server.join().unwrap();
}

#[test]
fn malformed_urls_json_and_nominal_mismatches_are_rejected() {
    let source = r#"fn main() {
print(match Url("https://user:secret@example.com/") { Ok(value) => false, Err(error) => true })
print(match Json("{\"broken\":[1,]}") { Ok(value) => false, Err(error) => true })
print(match Json("01") { Ok(value) => false, Err(error) => true })
}"#;
    let expected = ["true", "true", "true"];
    assert_eq!(run_source(source).unwrap(), expected);

    let mismatch = check_source(
        "fn consume(value: Json) {} fn misuse(url: Url) { consume(url) } fn main() {}",
    )
    .unwrap_err();
    assert!(mismatch.message.contains("function argument"));
    assert_eq!(mismatch.span.start.line, 1);
    assert!(mismatch.span.start.column > 50);

    let wrong = check_source("fn main() { value = Json(42) }").unwrap_err();
    assert_eq!(wrong.span.start.column, 26);
    assert!(wrong.message.contains("JSON source"));
}

#[test]
fn json_grammar_and_nesting_limits_match_across_engines() {
    let nested_128 = format!("{}0{}", "[".repeat(128), "]".repeat(128));
    let nested_129 = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    let object_4096 = format!(
        "{{{}}}",
        (0..4096)
            .map(|index| format!("\"k{index}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let object_4097 = format!(
        "{{{}}}",
        (0..4097)
            .map(|index| format!("\"k{index}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let cases = [
        ("null".to_owned(), true),
        (" true \n".to_owned(), true),
        ("-12.5e+3".to_owned(), true),
        (r#""escaped\n\u0041""#.to_owned(), true),
        (r#""\uD83D\uDE80""#.to_owned(), true),
        (r#""\uD83D""#.to_owned(), false),
        (r#""\uDE80""#.to_owned(), false),
        (r#""\uD83D\u0041""#.to_owned(), false),
        (r#"{"a":[1,false,null],"b":{}}"#.to_owned(), true),
        ("".to_owned(), false),
        ("01".to_owned(), false),
        ("1.".to_owned(), false),
        ("[1,]".to_owned(), false),
        (r#"{"a":1"#.to_owned(), false),
        ("true false".to_owned(), false),
        (r#"{"a":1,"a":2}"#.to_owned(), false),
        (r#"{"a":1,"\u0061":2}"#.to_owned(), false),
        (r#"{"outer":{"x":1,"x":2}}"#.to_owned(), false),
        (nested_128.clone(), true),
        (nested_129, false),
        (object_4096, true),
        (object_4097, false),
    ];
    let mut source = String::from("fn main() {\n");
    for (document, valid) in &cases {
        let literal = document
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        source.push_str(&format!(
            "print(match Json(\"{literal}\") {{ Ok(value) => {valid}, Err(error) => {} }})\n",
            !valid
        ));
    }
    source.push_str("}\n");
    let expected = vec!["true"; cases.len()].join("\n") + "\n";
    assert_eq!(run_source(&source).unwrap().join("\n") + "\n", expected);
    assert_eq!(native("grammar", &source, false).0, expected);

    let construction = format!(
        "fn build() -> Result<bool, ConversionError> {{ child = Json(\"{nested_128}\")? values = List.of(child) return Ok(match Json.array(values) {{ Ok(value) => false, Err(error) => true }}) }} fn main() {{ print(build()) }}"
    );
    assert_eq!(run_source(&construction).unwrap(), ["Result.Ok(true)"]);
    assert_eq!(
        native("construction-depth", &construction, false).0,
        "Result.Ok(true)\n"
    );
}

#[test]
fn structured_json_navigation_and_typed_extraction_are_differential() {
    let source = r#"fn inspect() -> Result<bool, ConversionError> {
document = Json("{\"name\":\"DISP\",\"escaped\":\"line\\nA\\u03bb\",\"enabled\":true,\"signed\":-42,\"unsigned\":42,\"ratio\":1.25,\"items\":[null,{\"answer\":7}]}")?
print(match document.get("name") { Some(value) => value.as_text()?, None => "missing" })
print(match document.get("escaped") { Some(value) => value.as_text()?, None => "missing" })
print(match document.get("enabled") { Some(value) => value.as_bool()?, None => false })
print(match document.get("signed") { Some(value) => value.as_int()?, None => 0 })
print(match document.get("unsigned") { Some(value) => value.as_uint()?, None => uint(0) })
print(match document.get("ratio") { Some(value) => value.as_f64()?, None => 0.0 })
print(match document.get("items") { Some(items) => match items.at(1) { Some(item) => match item.get("answer") { Some(answer) => answer.as_int()?, None => 0 }, None => 0 }, None => 0 })
print(document.get("absent"))
print(match document.as_bool() { Ok(value) => false, Err(error) => true })
huge = Json("1e9999")?
print(match huge.as_f64() { Ok(value) => false, Err(error) => true })
return Ok(true)
}
fn main() { print(inspect()) }"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    assert_eq!(
        expected,
        "DISP\nline\nAλ\ntrue\n-42\n42\n1.25\n7\nOption.None\ntrue\ntrue\nResult.Ok(true)\n"
    );
    let (actual, generated) = native("structured-navigation", source, true);
    assert_eq!(actual, expected);
    let generated = generated.unwrap();
    assert!(generated.contains("disp_json_get"));
    assert!(generated.contains("disp_json_at"));
    assert!(generated.contains("disp_json_as_text"));
}

#[test]
fn safe_json_construction_is_native_interpreter_differential() {
    let source = r#"fn build() -> Result<Json, ConversionError> {
values = List.of(Json.int(-7), Json.uint(uint(8)), Json.float(1.5)?, Json.string("safe\nλ")?)
array = Json.array(values)?
print(values.len())
entries = Map.of("array": array, "enabled": Json.bool(true), "nothing": Json.null())
document = Json.object(entries)?
print(entries.len())
return Ok(document)
}
fn main() {
print(build())
print(Json.int(-9223372036854775807))
print(Json.uint(uint(18446744073709551615)))
print(match Json.float(1e308 * 1e308) { Ok(value) => false, Err(error) => true })
}"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    assert_eq!(
        expected,
        "4\n3\nResult.Ok({\"array\":[-7,8,1.5,\"safe\\nλ\"],\"enabled\":true,\"nothing\":null})\n-9223372036854775807\n18446744073709551615\ntrue\n"
    );
    let (actual, generated) = native("structured-construction", source, true);
    assert_eq!(actual, expected);
    let generated = generated.unwrap();
    assert!(generated.contains("disp_json_from_array"));
    assert!(generated.contains("disp_json_from_object"));
    assert!(generated.contains("disp_json_from_i128"));
}

#[test]
fn structured_json_api_rejects_invalid_types_with_exact_spans() {
    let invalid = [
        (
            "fn main() { value = Json.null() value.get(1) }",
            "Json.get expects a String or str key",
            43,
        ),
        (
            "fn main() { value = Json.int(1.5) }",
            "Json.int expects a signed integer",
            30,
        ),
        (
            "fn main() { values = List.of(Json.null()) value = Json.array(1) }",
            "Json.array values",
            62,
        ),
        (
            "fn main() { entries = Map.of(1: Json.null()) value = Json.object(entries) }",
            "Json.object entries",
            66,
        ),
        (
            "fn misuse(url: Url) { value = url.query_param(1, \"x\") } fn main() {}",
            "Url.query_param expects String or str name and value",
            47,
        ),
    ];
    for (source, message, column) in invalid {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!(error.span.start.line, 1);
        assert_eq!(error.span.start.column, column);
    }
}
