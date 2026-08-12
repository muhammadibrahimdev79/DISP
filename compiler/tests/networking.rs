use disp::{
    backend::{self, BuildOptions},
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
    time::{Duration, Instant},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-networking-{label}-{}-{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

fn native(name: &str, source: &str, emit_c: bool) -> Option<(String, Option<String>)> {
    let path = unique_path(&format!("{name}.disp"));
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
    let generated = artifacts
        .backend_ir
        .map(|path| fs::read_to_string(path).unwrap());
    for _ in 0..4 {
        match Command::new(&artifacts.executable).output() {
            Ok(output) => {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return Some((
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .replace("\r\n", "\n"),
                    generated,
                ));
            }
            Err(error) if error.raw_os_error() == Some(4551) => {}
            Err(error) => panic!("native execution failed: {error}"),
        }
    }
    None
}

fn echo_server(connection_count: usize) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut served = 0;
        while served < connection_count && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = [0; 3];
                    stream.read_exact(&mut request).unwrap();
                    assert_eq!(request, [1, 2, 3]);
                    stream.write_all(&[9, 8, 7]).unwrap();
                    served += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("local TCP server failed: {error}"),
            }
        }
        served
    });
    (port, handle)
}

fn close_observer(connection_count: usize) -> (u16, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut closed = 0;
        while closed < connection_count && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut byte = [0];
                    assert_eq!(stream.read(&mut byte).unwrap(), 0);
                    closed += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("local TCP observer failed: {error}"),
            }
        }
        closed
    });
    (port, handle)
}

fn exchange_source(port: u16) -> String {
    format!(
        r#"
async fn exchange(address: SocketAddress) -> Result<uint, NetworkError> {{
    connected = await Async.connect(address)
    var stream = connected?
    let outgoing: List<u8> = List.of(u8(1), u8(2), u8(3))
    stream.write(outgoing[0..3])?
    print(outgoing.len())
    incoming = stream.read(3)
    bytes = incoming?
    print(bytes[0])
    stream.close()
    closed = stream.read(1)
    print(match closed {{ Ok(extra) => false, Err(error) => true }})
    return Ok(bytes.len())
}}

async fn main() {{
    print(await exchange(SocketAddress("127.0.0.1", {port})))
}}
"#
    )
}

#[test]
fn tcp_connect_read_write_close_are_native_interpreter_differential() {
    let (port, server) = echo_server(2);
    let source = exchange_source(port);
    assert_eq!(
        run_source(&source).unwrap(),
        ["3", "9", "true", "Result.Ok(3)"]
    );
    let Some((actual, generated)) = native("exchange", &source, true) else {
        return;
    };
    assert_eq!(actual, "3\n9\ntrue\nResult.Ok(3)\n");
    assert_eq!(server.join().unwrap(), 2);
    let generated = generated.unwrap();
    assert!(generated.contains("getaddrinfo"));
    assert!(generated.contains("disp_tcp_stream_read"));
    assert!(generated.contains("disp_tcp_stream_write"));
    assert!(generated.contains("disp_tcp_stream_drop"));
}

#[test]
fn connect_failure_is_a_typed_result() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let source = format!(
        r#"async fn main() {{
result = await Async.connect(SocketAddress("127.0.0.1", {port}))
print(match result {{ Ok(stream) => true, Err(error) => false }})
}}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["false"]);
    if let Some((actual, _)) = native("refused", &source, false) {
        assert_eq!(actual, "false\n");
    }
}

#[test]
fn dropping_a_connected_stream_closes_the_native_resource() {
    let (port, observer) = close_observer(2);
    let source = format!(
        r#"async fn main() {{
result = await Async.connect(SocketAddress("127.0.0.1", {port}))
print(match result {{ Ok(stream) => true, Err(error) => false }})
}}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    let Some((actual, _)) = native("implicit-close", &source, false) else {
        return;
    };
    assert_eq!(actual, "true\n");
    assert_eq!(observer.join().unwrap(), 2);
}

#[test]
fn networking_ownership_and_mutability_are_checked() {
    let moved = check_source(
        r#"async fn main() {
address = SocketAddress("127.0.0.1", 80)
future = Async.connect(address)
print(address)
}"#,
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{}", moved.message);
    assert_eq!(moved.span.start.line, 4);

    let immutable = check_source(
        r#"async fn use_stream(address: SocketAddress) -> Result<uint, NetworkError> {
connected = await Async.connect(address)
let stream = connected?
stream.close()
return Ok(0)
}
async fn main() {}
"#,
    )
    .unwrap_err();
    assert!(
        immutable.message.contains("mutable") || immutable.message.contains("mutably"),
        "{}",
        immutable.message
    );
    assert_eq!(immutable.span.start.line, 4);
}

#[test]
fn networking_rejects_invalid_types_with_source_spans() {
    let host = check_source("fn main() { address = SocketAddress(7, 80) }").unwrap_err();
    assert!(host.message.contains("host"), "{}", host.message);
    assert_eq!(host.span.start.column, 37);

    let port =
        check_source("fn main() { address = SocketAddress(\"localhost\", true) }").unwrap_err();
    assert!(port.message.contains("port"), "{}", port.message);

    let connect =
        check_source("async fn main() { future = Async.connect(Path(\"x\")) }").unwrap_err();
    assert!(connect.message.contains("address"), "{}", connect.message);

    let write = check_source(
        r#"async fn work(address: SocketAddress) -> Result<uint, NetworkError> {
connected = await Async.connect(address)
var stream = connected?
stream.write(List.of(1, 2))?
return Ok(0)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(write.message.contains("u8"), "{}", write.message);
    assert_eq!(write.span.start.line, 4);
}

#[test]
fn socket_addresses_validate_runtime_values() {
    let empty = run_source("fn main() { print(SocketAddress(\"\", 80)) }").unwrap_err();
    assert!(empty.message.contains("empty"), "{}", empty.message);

    let port = run_source("fn main() { print(SocketAddress(\"localhost\", 70000)) }").unwrap_err();
    assert!(port.message.contains("65535"), "{}", port.message);
}

#[test]
fn unpolled_connect_is_lazy() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let source = format!(
        "async fn main() {{ future = Async.connect(SocketAddress(\"127.0.0.1\", {port})) print(true) }}"
    );
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    if let Some((actual, _)) = native("lazy", &source, false) {
        assert_eq!(actual, "true\n");
    }
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}
