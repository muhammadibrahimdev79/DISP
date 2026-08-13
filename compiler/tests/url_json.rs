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
    panic!("Windows Application Control repeatedly blocked URL/JSON test executable")
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
    let cases = [
        ("null".to_owned(), true),
        (" true \n".to_owned(), true),
        ("-12.5e+3".to_owned(), true),
        (r#""escaped\n\u0041""#.to_owned(), true),
        (r#"{"a":[1,false,null],"b":{}}"#.to_owned(), true),
        ("".to_owned(), false),
        ("01".to_owned(), false),
        ("1.".to_owned(), false),
        ("[1,]".to_owned(), false),
        (r#"{"a":1"#.to_owned(), false),
        ("true false".to_owned(), false),
        (nested_128, true),
        (nested_129, false),
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
}
