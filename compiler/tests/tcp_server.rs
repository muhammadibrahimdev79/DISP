use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-tcp-server-{label}-{}-{}",
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

fn available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn client(port: u16) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::NotConnected
                    ) && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("test client could not connect: {error}"),
            }
        };
        stream.write_all(&[1, 2, 3]).unwrap();
        let mut response = [0; 3];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(response, [9, 8, 7]);
    })
}

fn server_source(port: u16) -> String {
    format!(
        r#"
async fn serve(address: SocketAddress) -> Result<uint, NetworkError> {{
    bound = TcpListener.bind(address)
    var listener = bound?
    print(listener.local_port()? == {port})
    accepted = await listener.accept()
    var stream = accepted?
    request = stream.read(3)?
    stream.write(List.of(u8(9), u8(8), u8(7)))?
    stream.close()
    listener.close()
    return Ok(uint(request[0]))
}}

async fn main() {{
    print(await serve(SocketAddress("127.0.0.1", {port})))
}}
"#
    )
}

#[test]
fn listener_bind_accept_timeout_and_exchange_are_differential() {
    let port = available_port();
    let source = server_source(port);

    let interpreted_client = client(port);
    assert_eq!(run_source(&source).unwrap(), ["true", "Result.Ok(1)"]);
    interpreted_client.join().unwrap();

    let native_client = client(port);
    let Some((actual, generated)) = native("exchange", &source, true) else {
        return;
    };
    native_client.join().unwrap();
    assert_eq!(actual, "true\nResult.Ok(1)\n");
    let generated = generated.unwrap();
    assert!(generated.contains("disp_tcp_listener_bind"));
    assert!(generated.contains("disp_accept_poll"));
    assert!(generated.contains("disp_tcp_listener_drop"));
    assert!(generated.contains("DISP_NETWORKING"));
}

#[test]
fn accept_timeout_is_a_typed_error_and_does_not_block_shutdown() {
    let port = available_port();
    let source = format!(
        r#"async fn wait() -> Result<bool, NetworkError> {{
bound = TcpListener.bind(SocketAddress("127.0.0.1", {port}))
var listener = bound?
future = listener.accept_timeout(Duration.from_millis(8))
await Async.sleep(Duration.from_millis(12))
started = Time.now()
accepted = await future
waited = started.elapsed().millis() >= 4
listener.close()
timed_out = match accepted {{ Ok(stream) => false, Err(error) => true }}
return Ok(waited && timed_out)
}}
async fn main() {{ print(await wait()) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
    if let Some((actual, _)) = native("timeout", &source, false) {
        assert_eq!(actual, "Result.Ok(true)\n");
    }
}

#[test]
fn listener_bind_failures_are_typed_results() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port();
    let source = format!(
        r#"fn main() {{
result = TcpListener.bind(SocketAddress("127.0.0.1", {port}))
print(match result {{ Ok(listener) => false, Err(error) => true }})
}}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    if let Some((actual, _)) = native("bind-error", &source, false) {
        assert_eq!(actual, "true\n");
    }
}

#[test]
fn unpolled_accept_is_lazy_and_listener_cleanup_is_immediate() {
    let port = available_port();
    let source = format!(
        r#"async fn lazy() -> Result<bool, NetworkError> {{
bound = TcpListener.bind(SocketAddress("127.0.0.1", {port}))
var listener = bound?
future = listener.accept()
return Ok(true)
}}
async fn main() {{ print(await lazy()) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
    TcpListener::bind(("127.0.0.1", port)).unwrap();
    if let Some((actual, _)) = native("lazy-accept", &source, false) {
        assert_eq!(actual, "Result.Ok(true)\n");
    }
    TcpListener::bind(("127.0.0.1", port)).unwrap();
}

#[test]
fn closing_a_listener_wakes_its_pending_accept_safely() {
    let port = available_port();
    let source = format!(
        r#"async fn close_pending() -> Result<bool, NetworkError> {{
bound = TcpListener.bind(SocketAddress("127.0.0.1", {port}))
var listener = bound?
future = listener.accept()
listener.close()
accepted = await future
return Ok(match accepted {{ Ok(stream) => false, Err(error) => true }})
}}
async fn main() {{ print(await close_pending()) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
    if let Some((actual, _)) = native("close-pending", &source, false) {
        assert_eq!(actual, "Result.Ok(true)\n");
    }
}

#[test]
fn listener_ownership_mutability_and_types_are_checked() {
    let moved_address = check_source(
        r#"fn main() {
address = SocketAddress("127.0.0.1", 8000)
bound = TcpListener.bind(address)
print(address)
}"#,
    )
    .unwrap_err();
    assert!(
        moved_address.message.contains("moved"),
        "{}",
        moved_address.message
    );
    assert_eq!(moved_address.span.start.line, 4);

    let immutable = check_source(
        r#"fn invalid() -> Result<uint, NetworkError> {
bound = TcpListener.bind(SocketAddress("127.0.0.1", 8000))
let listener = bound?
listener.close()
return Ok(0)
}
fn main() {}"#,
    )
    .unwrap_err();
    assert!(
        immutable.message.contains("mutable") || immutable.message.contains("mutably"),
        "{}",
        immutable.message
    );
    assert_eq!(immutable.span.start.line, 4);

    let timeout = check_source(
        r#"fn invalid() -> Result<uint, NetworkError> {
bound = TcpListener.bind(SocketAddress("127.0.0.1", 8000))
listener = bound?
future = listener.accept_timeout(10)
return Ok(0)
}
fn main() {}"#,
    )
    .unwrap_err();
    assert!(timeout.message.contains("timeout"), "{}", timeout.message);
    assert_eq!(timeout.span.start.line, 4);
}

#[test]
fn closed_listener_operations_return_errors() {
    let port = available_port();
    let source = format!(
        r#"fn closed() -> Result<bool, NetworkError> {{
bound = TcpListener.bind(SocketAddress("127.0.0.1", {port}))
var listener = bound?
listener.close()
return Ok(match listener.local_port() {{ Ok(value) => false, Err(error) => true }})
}}
fn main() {{ print(closed()) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
    if let Some((actual, _)) = native("closed", &source, false) {
        assert_eq!(actual, "Result.Ok(true)\n");
    }
}
