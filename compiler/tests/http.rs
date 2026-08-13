use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-http-{label}-{}-{}.disp",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

fn native(name: &str, source: &str, emit_c: bool) -> (Option<String>, Option<String>) {
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
                    Some(
                        String::from_utf8(output.stdout)
                            .unwrap()
                            .replace("\r\n", "\n"),
                    ),
                    generated,
                );
            }
            Err(error) if error.raw_os_error() == Some(4551) => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("native HTTP execution failed: {error}"),
        }
    }
    (None, generated)
}

fn server(
    requests: usize,
    response: impl Fn(&str) -> Vec<u8> + Send + 'static,
) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut served = 0;
        while served < requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                        let count = stream.read(&mut chunk).unwrap();
                        if count == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..count]);
                        assert!(request.len() <= 32 * 1024);
                    }
                    let header_end = request
                        .windows(4)
                        .position(|part| part == b"\r\n\r\n")
                        .map(|position| position + 4)
                        .unwrap();
                    let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    while request.len() < header_end + content_length {
                        let count = stream.read(&mut chunk).unwrap();
                        assert!(count > 0, "request body ended early");
                        request.extend_from_slice(&chunk[..count]);
                        assert!(request.len() <= 17 * 1024 * 1024);
                    }
                    let request = String::from_utf8(request).unwrap();
                    stream.write_all(&response(&request)).unwrap();
                    stream.flush().unwrap();
                    served += 1;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("local HTTP server failed: {error}"),
            }
        }
        served
    });
    (port, handle)
}

fn persistent_server(requests: usize) -> (u16, thread::JoinHandle<(usize, usize)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut connections = 0;
        let mut served = 0;
        while served < requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    connections += 1;
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    while served < requests {
                        let mut request = Vec::new();
                        let mut byte = [0; 1];
                        while !request.ends_with(b"\r\n\r\n") {
                            match stream.read(&mut byte) {
                                Ok(0) => break,
                                Ok(_) => request.push(byte[0]),
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        ErrorKind::TimedOut | ErrorKind::WouldBlock
                                    ) =>
                                {
                                    break;
                                }
                                Err(error) => panic!("persistent HTTP read failed: {error}"),
                            }
                            assert!(request.len() <= 32 * 1024);
                        }
                        if request.is_empty() || !request.ends_with(b"\r\n\r\n") {
                            break;
                        }
                        let headers = String::from_utf8(request).unwrap();
                        assert!(headers.starts_with("GET /"));
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        let mut body = vec![0; content_length];
                        stream.read_exact(&mut body).unwrap();
                        served += 1;
                        let connection = if served == requests {
                            "close"
                        } else {
                            "keep-alive"
                        };
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: {connection}\r\n\r\nok"
                        )
                        .unwrap();
                        stream.flush().unwrap();
                        if connection == "close" {
                            break;
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("persistent HTTP server failed: {error}"),
            }
        }
        (connections, served)
    });
    (port, handle)
}

fn response_source(port: u16) -> String {
    format!(
        r#"async fn inspect() -> Result<bool, HttpError> {{
response = (await Http.get_timeout("http://127.0.0.1:{port}/hello?x=1", Duration.from_seconds(3)))?
print(response.status())
print(response.is_success())
print(response.len())
print(response.is_empty())
print(response.header("CONTENT-TYPE"))
print(response.header("x-mixed"))
body = response.body()
print(body[0])
text = response.text()?
print(text)
return Ok(response.url().starts_with("http://127.0.0.1:") && text == "hello")
}}
async fn main() {{ print(await inspect()) }}"#
    )
}

fn basic_response(_: &str) -> Vec<u8> {
    b"HTTP/1.1 201 Created\r\nContent-Length: 5\r\nContent-Type: text/plain\r\nX-MiXeD: Value\r\nConnection: close\r\n\r\nhello".to_vec()
}

fn connection_reuse_source(port: u16) -> String {
    format!(
        r#"async fn exercise() -> Result<bool, HttpError> {{
first = (await Http.get("http://127.0.0.1:{port}/first"))?
second = (await Http.get("http://127.0.0.1:{port}/second"))?
return Ok(first.status() == 200 && second.status() == 200)
}}
async fn main() {{ print(await exercise()) }}"#
    )
}

#[test]
fn sequential_requests_reuse_connections_in_interpreter_and_native() {
    let (port, served) = persistent_server(2);
    let output = run_source(&connection_reuse_source(port)).unwrap();
    let server_stats = served.join().unwrap();
    assert_eq!(server_stats, (1, 2), "interpreter output: {output:?}");
    assert_eq!(output, ["Result.Ok(true)"]);

    let (port, served) = persistent_server(2);
    let (output, _) = native("connection-reuse", &connection_reuse_source(port), false);
    if let Some(output) = output {
        assert_eq!(output, "Result.Ok(true)\n");
        assert_eq!(served.join().unwrap(), (1, 2));
    }
}

#[test]
fn owned_responses_headers_text_and_bytes_are_native_interpreter_differential() {
    let expected = [
        "201",
        "true",
        "5",
        "false",
        "Option.Some(text/plain)",
        "Option.Some(Value)",
        "104",
        "hello",
        "Result.Ok(true)",
    ];
    let (port, served) = server(1, basic_response);
    assert_eq!(run_source(&response_source(port)).unwrap(), expected);
    assert_eq!(served.join().unwrap(), 1);

    let (port, served) = server(1, basic_response);
    let (output, generated) = native("response", &response_source(port), true);
    if let Some(output) = output {
        assert_eq!(output, expected.join("\n") + "\n");
        assert_eq!(served.join().unwrap(), 1);
    }
    let generated = generated.unwrap();
    assert!(generated.contains("disp_native_http_response"));
    assert!(generated.contains("disp_http_request_worker"));
    assert!(generated.contains("WinHttpOpenRequest"));
    assert!(generated.contains("WINHTTP_ENABLE_SSL_REVOCATION"));
    assert!(generated.contains("WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP"));
    assert!(generated.contains("DISP_HTTP_BODY_LIMIT"));
    assert!(generated.contains("disp_http_response_drop"));
}

fn request_response(request: &str) -> Vec<u8> {
    let (headers, body) = request.split_once("\r\n\r\n").unwrap();
    let first = headers.lines().next().unwrap();
    match first {
        "POST /direct HTTP/1.1" => {
            assert!(headers.contains("Content-Type: text/plain; charset=utf-8"));
            assert_eq!(body, "direct");
        }
        "PUT /bytes HTTP/1.1" => assert_eq!(body, "ABC"),
        "PATCH /patch HTTP/1.1" => assert_eq!(body, "patch"),
        "DELETE /remove HTTP/1.1" => assert!(body.is_empty()),
        "OPTIONS /custom HTTP/1.1" => {
            assert!(headers.contains("X-DISP: safe"));
            assert!(headers.contains("Content-Type: text/plain; charset=utf-8"));
            assert_eq!(body, "custom");
        }
        _ => panic!("unexpected HTTP request: {first}"),
    }
    b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec()
}

fn general_request_source(port: u16) -> String {
    format!(
        r#"async fn exercise() -> Result<bool, HttpError> {{
posted = (await Http.post("http://127.0.0.1:{port}/direct", "direct"))?
put = (await Http.put("http://127.0.0.1:{port}/bytes", List.of(u8(65), u8(66), u8(67))))?
patched = (await Http.patch("http://127.0.0.1:{port}/patch", "patch"))?
deleted = (await Http.delete("http://127.0.0.1:{port}/remove"))?
request = Http.request("OPTIONS", "http://127.0.0.1:{port}/custom")?
request = request.header("X-DISP", "safe")?
request = request.text("custom")?
custom = (await request.send_timeout(Duration.from_seconds(3)))?
return Ok(posted.status() == 202 && put.status() == 202 && patched.status() == 202 && deleted.status() == 202 && custom.text()? == "ok")
}}
async fn main() {{ print(await exercise()) }}"#
    )
}

#[test]
fn general_methods_headers_text_and_byte_bodies_are_differential() {
    let (port, served) = server(5, request_response);
    assert_eq!(
        run_source(&general_request_source(port)).unwrap(),
        ["Result.Ok(true)"]
    );
    assert_eq!(served.join().unwrap(), 5);

    let (port, served) = server(5, request_response);
    let (output, generated) = native("general-request", &general_request_source(port), true);
    if let Some(output) = output {
        assert_eq!(output, "Result.Ok(true)\n");
        assert_eq!(served.join().unwrap(), 5);
    }
    let generated = generated.unwrap();
    assert!(generated.contains("disp_native_http_request"));
    assert!(generated.contains("disp_http_builder_header"));
    assert!(generated.contains("disp_http_builder_body"));
    assert!(generated.contains("disp_http_request_from_builder"));
    assert!(generated.contains("WINHTTP_OPTION_REDIRECT_POLICY_NEVER"));
}

#[test]
fn request_futures_snapshot_inputs_and_non_get_redirects_are_not_replayed() {
    let response = |request: &str| {
        assert!(request.starts_with("POST /submit "));
        assert!(request.ends_with("\r\n\r\noriginal"));
        b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /danger\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
    };
    let source_for = |port| {
        format!(
            r#"async fn main() {{
var body = String.new()
body.push_str("original")
future = Http.post("http://127.0.0.1:{port}/submit", body)
body.clear()
result = await future
print(match result {{ Ok(response) => response.status() == 307, Err(error) => false }})
}}"#
        )
    };
    let (port, served) = server(1, response);
    assert_eq!(run_source(&source_for(port)).unwrap(), ["true"]);
    assert_eq!(served.join().unwrap(), 1);

    let (port, served) = server(1, response);
    let (output, _) = native("request-snapshot", &source_for(port), false);
    if let Some(output) = output {
        assert_eq!(output, "true\n");
        assert_eq!(served.join().unwrap(), 1);
    }
}

#[test]
fn requests_with_user_headers_do_not_forward_them_across_redirects() {
    let response = |request: &str| {
        assert!(request.starts_with("GET /secure "));
        assert!(request.contains("Authorization: Bearer secret"));
        b"HTTP/1.1 302 Found\r\nLocation: /other-origin-risk\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
    };
    let source_for = |port| {
        format!(
            r#"async fn inspect() -> Result<bool, HttpError> {{
request = Http.request("GET", "http://127.0.0.1:{port}/secure")?
request = request.header("Authorization", "Bearer secret")?
response = (await request.send())?
return Ok(response.status() == 302)
}}
async fn main() {{ print(await inspect()) }}"#
        )
    };
    let (port, served) = server(1, response);
    assert_eq!(run_source(&source_for(port)).unwrap(), ["Result.Ok(true)"]);
    assert_eq!(served.join().unwrap(), 1);

    let (port, served) = server(1, response);
    let (output, _) = native("header-redirect", &source_for(port), false);
    if let Some(output) = output {
        assert_eq!(output, "Result.Ok(true)\n");
        assert_eq!(served.join().unwrap(), 1);
    }
}

#[test]
fn request_validation_is_typed_bounded_and_source_spanned() {
    let method = check_source(
        "async fn main() { value = Http.request(\"TRACE\", \"https://example.com\") }",
    )
    .unwrap_err();
    assert!(method.message.contains("forbidden"), "{method}");

    let header = check_source(
        r#"async fn inspect() -> Result<bool, HttpError> {
request = Http.request("GET", "https://example.com")?
request = request.header("Content-Length", "99")?
return Ok(true)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(header.message.contains("controlled"), "{header}");
    assert_eq!(header.span.start.line, 3);

    let moved = check_source(
        r#"async fn inspect() -> Result<bool, HttpError> {
request = Http.request("GET", "https://example.com")?
next = request.header("X-Test", "one")?
future = request.send()
return Ok(true)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{moved}");
    assert_eq!(moved.span.start.line, 4);

    let body = check_source("async fn main() { future = Http.post(\"https://example.com\", 42) }")
        .unwrap_err();
    assert!(body.message.contains("HTTP body"), "{body}");

    let dynamic = r#"async fn inspect() -> Result<bool, HttpError> {
method = "TRACE"
request = Http.request(method, "https://example.com")
return Ok(match request { Ok(value) => false, Err(error) => true })
}
async fn main() { print(await inspect()) }"#;
    assert_eq!(run_source(dynamic).unwrap(), ["Result.Ok(true)"]);
    if let Some(output) = native("dynamic-method", dynamic, false).0 {
        assert_eq!(output, "Result.Ok(true)\n");
    }

    let unsafe_header = r#"async fn inspect() -> Result<bool, HttpError> {
request = Http.request("GET", "https://example.com")?
value = "line\nbreak"
result = request.header("X-Test", value)
return Ok(match result { Ok(next) => false, Err(error) => true })
}
async fn main() { print(await inspect()) }"#;
    assert_eq!(run_source(unsafe_header).unwrap(), ["Result.Ok(true)"]);
    if let Some(output) = native("unsafe-header", unsafe_header, false).0 {
        assert_eq!(output, "Result.Ok(true)\n");
    }

    let headers = r#"async fn inspect() -> Result<bool, HttpError> {
var request = Http.request("GET", "https://example.com")?
var index = 0
while index < 101 {
request = request.header("X-Test", "value")?
index += 1
}
return Ok(false)
}
async fn main() { print(await inspect()) }"#;
    assert!(
        matches!(run_source(headers).unwrap().as_slice(), [value] if value.starts_with("Result.Err("))
    );
    if let Some(output) = native("header-limit", headers, false).0 {
        assert!(output.starts_with("Result.Err("), "{output}");
    }
}

fn redirect_response(request: &str) -> Vec<u8> {
    if request.starts_with("GET /start ") {
        b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec()
    } else {
        assert!(request.starts_with("GET /final "), "{request}");
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nDIS\r\n1\r\nP\r\n0\r\n\r\n".to_vec()
    }
}

fn redirect_source(port: u16) -> String {
    format!(
        r#"async fn inspect() -> Result<bool, HttpError> {{
response = (await Http.get("http://127.0.0.1:{port}/start"))?
text = response.text()?
return Ok(response.status() == 200 && response.url().ends_with("/final") && text == "DISP")
}}
async fn main() {{ print(await inspect()) }}"#
    )
}

#[test]
fn bounded_redirects_and_chunked_bodies_match_across_engines() {
    let (port, served) = server(2, redirect_response);
    assert_eq!(
        run_source(&redirect_source(port)).unwrap(),
        ["Result.Ok(true)"]
    );
    assert_eq!(served.join().unwrap(), 2);

    let (port, served) = server(2, redirect_response);
    let (output, _) = native("redirect", &redirect_source(port), false);
    if let Some(output) = output {
        assert_eq!(output, "Result.Ok(true)\n");
        assert_eq!(served.join().unwrap(), 2);
    }
}

#[test]
fn futures_are_lazy_zero_timeouts_are_deterministic_and_urls_are_borrowed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let source = format!(
        r#"async fn main() {{
var url = String.new()
url.push_str("http://127.0.0.1:{port}/never")
unused = Http.get(url)
print(url.len() > 0)
timed = await Http.get_timeout(url, Duration.from_millis(0))
print(match timed {{ Ok(response) => false, Err(error) => true }})
timed_post = await Http.post_timeout(url, "body", Duration.from_millis(0))
print(match timed_post {{ Ok(response) => false, Err(error) => true }})
}}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["true", "true", "true"]);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
    let (output, _) = native("lazy", &source, false);
    if let Some(output) = output {
        assert_eq!(output, "true\ntrue\ntrue\n");
        assert!(matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
    }
}

#[test]
fn invalid_urls_and_invalid_utf8_are_typed_failures() {
    let invalid = r#"async fn main() {
print(match await Http.get("file:///secret") { Ok(response) => false, Err(error) => true })
print(match await Http.get("http://user:password@localhost/") { Ok(response) => false, Err(error) => true })
print(match await Http.get("http://localhost/#fragment") { Ok(response) => false, Err(error) => true })
}"#;
    assert_eq!(run_source(invalid).unwrap(), ["true", "true", "true"]);
    if let Some(output) = native("invalid-url", invalid, false).0 {
        assert_eq!(output, "true\ntrue\ntrue\n");
    }

    let response = |_: &str| {
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\xffx".to_vec()
    };
    let source_for = |port| {
        format!(
            r#"async fn main() {{
result = await Http.get("http://127.0.0.1:{port}/")
print(match result {{ Ok(response) => match response.text() {{ Ok(text) => false, Err(error) => true }}, Err(error) => false }})
}}"#
        )
    };
    let (port, served) = server(1, response);
    let source = source_for(port);
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    assert_eq!(served.join().unwrap(), 1);

    let (port, served) = server(1, response);
    let source = source_for(port);
    let (output, _) = native("invalid-utf8", &source, false);
    if let Some(output) = output {
        assert_eq!(output, "true\n");
        assert_eq!(served.join().unwrap(), 1);
    }
}

#[test]
fn http_types_ownership_and_diagnostics_have_source_spans() {
    let pipeline = r#"async fn fetch() -> Result<uint, HttpError> {
response = (await Http.get("https://example.com"))?
return Ok(response.status())
}
async fn main() {}"#;
    let (hir, mir) = lower_source(pipeline).unwrap();
    let hir = format!("{hir:#?}");
    let mir = format!("{mir:#?}");
    assert!(
        hir.contains("HttpResponse") && hir.contains("Http.get"),
        "{hir}"
    );
    assert!(
        mir.contains("HttpResponse") && mir.contains("Http.get"),
        "{mir}"
    );

    let url = check_source("async fn main() { value = Http.get(42) }").unwrap_err();
    assert!(
        url.message.contains("URL") && url.message.contains("String"),
        "{url}"
    );
    assert_eq!(url.span.start.line, 1);

    let timeout =
        check_source("async fn main() { value = Http.get_timeout(\"https://example.com\", 10) }")
            .unwrap_err();
    assert!(timeout.message.contains("Duration"), "{timeout}");
    assert_eq!(timeout.span.start.line, 1);

    let header = check_source(
        r#"async fn inspect() -> Result<bool, HttpError> {
response = (await Http.get("https://example.com"))?
value = response.header(42)
return Ok(true)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(header.message.contains("header name"), "{header}");
    assert_eq!(header.span.start.line, 3);

    let invalid_header = check_source(
        r#"async fn inspect() -> Result<bool, HttpError> {
response = (await Http.get("https://example.com"))?
value = response.header("bad name")
return Ok(true)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(invalid_header.message.contains("invalid characters"));
    assert_eq!(invalid_header.span.start.line, 3);

    let moved = check_source(
        r#"async fn inspect() -> Result<bool, HttpError> {
response = (await Http.get("https://example.com"))?
moved = response
print(response.status())
return Ok(true)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{moved}");
    assert_eq!(moved.span.start.line, 4);
}

#[test]
fn malformed_framing_and_oversized_responses_are_rejected() {
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\nabc".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 16777217\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nContent-Length: 0\r\n\r\n".as_slice(),
    ] {
        let owned = response.to_vec();
        let (port, served) = server(1, move |_| owned.clone());
        let source = format!(
            r#"async fn main() {{
result = await Http.get_timeout("http://127.0.0.1:{port}/", Duration.from_seconds(2))
print(match result {{ Ok(response) => false, Err(error) => true }})
}}"#
        );
        assert_eq!(run_source(&source).unwrap(), ["true"]);
        assert_eq!(served.join().unwrap(), 1);
    }

    let mut oversized_headers = b"HTTP/1.1 200 OK\r\nX-Large: ".to_vec();
    oversized_headers.extend(std::iter::repeat_n(b'a', 64 * 1024));
    oversized_headers.extend_from_slice(b"\r\n\r\n");
    let (port, served) = server(1, move |_| oversized_headers.clone());
    let source = format!(
        r#"async fn main() {{
result = await Http.get_timeout("http://127.0.0.1:{port}/", Duration.from_seconds(2))
print(match result {{ Ok(response) => false, Err(error) => true }})
}}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    assert_eq!(served.join().unwrap(), 1);
}
