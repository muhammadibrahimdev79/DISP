use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener},
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-async-tcp-{label}-{}-{}",
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

fn protocol_server(connection_count: usize) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for _ in 0..connection_count {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 3];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(request, [1, 2, 3]);
            stream.write_all(&[9, 8, 7]).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut trailing = [0; 1];
            assert_eq!(stream.read(&mut trailing).unwrap(), 0);
        }
    });
    (port, handle)
}

fn protocol_source(port: u16) -> String {
    format!(
        r#"async fn exchange(address: SocketAddress) -> Result<uint, NetworkError> {{
    connected = await Async.connect_timeout(address, Duration.from_seconds(2))
    var stream = connected?
    var outgoing = List.of(u8(1), u8(2), u8(3))
    pending_write = stream.write_async_timeout(outgoing[0..3], Duration.from_seconds(2))
    outgoing[0] = u8(99)
    written = await pending_write
    print(written?)
    stream.shutdown_write()?
    first = await stream.read_async_timeout(3, Duration.from_seconds(2))
    bytes = first?
    print(bytes[0])
    eof = await stream.read_async(1)
    end = eof?
    print(end.is_empty())
    stream.shutdown_read()?
    after_shutdown = await stream.read_async(1)
    print(match after_shutdown {{ Ok(extra) => false, Err(error) => true }})
    stream.close()
    return Ok(bytes.len())
}}

async fn main() {{
    print(await exchange(SocketAddress("127.0.0.1", {port})))
}}
"#
    )
}

#[test]
fn async_stream_io_eof_half_close_and_owned_write_are_differential() {
    let (port, server) = protocol_server(2);
    let source = protocol_source(port);
    assert_eq!(
        run_source(&source).unwrap(),
        ["3", "9", "true", "true", "Result.Ok(3)"]
    );
    let Some((actual, generated)) = native("protocol", &source, true) else {
        return;
    };
    assert_eq!(actual, "3\n9\ntrue\ntrue\nResult.Ok(3)\n");
    server.join().unwrap();
    let generated = generated.unwrap();
    assert!(generated.contains("disp_socket_io_poll"));
    assert!(generated.contains("read_busy"));
    assert!(generated.contains("write_busy"));
    assert!(generated.contains("disp_tcp_stream_shutdown"));
    assert!(generated.contains("atomic_size_t refs"));
}

#[test]
fn timeout_starts_when_the_lazy_read_future_is_awaited() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut marker = [0];
            stream.read_exact(&mut marker).unwrap();
            thread::sleep(Duration::from_millis(60));
            stream.write_all(&[7]).unwrap();
        }
    });
    let source = format!(
        r#"async fn probe(address: SocketAddress) -> Result<bool, NetworkError> {{
connected = await Async.connect_timeout(address, Duration.from_seconds(2))
var stream = connected?
future = stream.read_async_timeout(1, Duration.from_millis(10))
await Async.sleep(Duration.from_millis(20))
stream.write(List.of(u8(1)))?
result = await future
stream.close()
return Ok(match result {{ Ok(bytes) => true, Err(error) => false }})
}}
async fn main() {{ print(await probe(SocketAddress("127.0.0.1", {port}))) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(false)"]);
    let Some((actual, _)) = native("read-timeout", &source, false) else {
        return;
    };
    assert_eq!(actual, "Result.Ok(false)\n");
    server.join().unwrap();
}

#[test]
fn closing_a_stream_invalidates_pending_futures_without_dangling_state() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).unwrap(), 0);
        }
    });
    let source = format!(
        r#"async fn probe(address: SocketAddress) -> Result<bool, NetworkError> {{
connected = await Async.connect(address)
var stream = connected?
future = stream.read_async(1)
stream.close()
result = await future
return Ok(match result {{ Ok(bytes) => false, Err(error) => true }})
}}
async fn main() {{ print(await probe(SocketAddress("127.0.0.1", {port}))) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
    let Some((actual, _)) = native("close-pending", &source, false) else {
        return;
    };
    assert_eq!(actual, "Result.Ok(true)\n");
    server.join().unwrap();
}

#[test]
fn same_direction_async_operations_are_serialized() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&[4]).unwrap();
            thread::sleep(Duration::from_millis(3));
            stream.write_all(&[5]).unwrap();
        }
    });
    let source = format!(
        r#"async fn collect(address: SocketAddress) -> Result<uint, NetworkError> {{
connected = await Async.connect(address)
var stream = connected?
first = Async.spawn(stream.read_async_timeout(1, Duration.from_seconds(2)))
second = Async.spawn(stream.read_async_timeout(1, Duration.from_seconds(2)))
left = (await first)?
right = (await second)?
stream.close()
return Ok(uint(left[0]) + uint(right[0]))
}}
async fn main() {{ print(await collect(SocketAddress("127.0.0.1", {port}))) }}"#
    );
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(9)"]);
    let Some((actual, _)) = native("serialized-reads", &source, false) else {
        return;
    };
    assert_eq!(actual, "Result.Ok(9)\n");
    server.join().unwrap();
}

#[test]
fn closing_a_natively_polled_read_cancels_it_responsively() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let source = format!(
        r#"async fn cancel(address: SocketAddress) -> Result<bool, NetworkError> {{
connected = await Async.connect(address)
var stream = connected?
task = Async.spawn(stream.read_async_timeout(1, Duration.from_seconds(2)))
await Async.yield()
stream.close()
result = await task
return Ok(match result {{ Ok(bytes) => false, Err(error) => true }})
}}
async fn main() {{ print(await cancel(SocketAddress("127.0.0.1", {port}))) }}"#
    );
    let Some((actual, _)) = native("cancel-polled-read", &source, false) else {
        return;
    };
    assert_eq!(actual, "Result.Ok(true)\n");
    server.join().unwrap();
}

#[test]
fn async_stream_types_mutability_and_source_spans_are_checked() {
    let bad_timeout = check_source(
        "async fn main() { future = Async.connect_timeout(SocketAddress(\"x\", 1), 10) }",
    )
    .unwrap_err();
    assert!(bad_timeout.message.contains("Duration"), "{bad_timeout}");
    assert_eq!(bad_timeout.span.start.line, 1);

    let bad_write = check_source(
        r#"async fn inspect() -> Result<uint, NetworkError> {
connected = await Async.connect(SocketAddress("127.0.0.1", 1))
var stream = connected?
future = stream.write_async(List.of(1, 2))
return Ok(0)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(bad_write.message.contains("List<u8>"), "{bad_write}");
    assert_eq!(bad_write.span.start.line, 4);

    let immutable = check_source(
        r#"async fn inspect() -> Result<uint, NetworkError> {
connected = await Async.connect(SocketAddress("127.0.0.1", 1))
let stream = connected?
future = stream.read_async(1)
return Ok(0)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(immutable.message.contains("mutable"), "{immutable}");
    assert_eq!(immutable.span.start.line, 4);

    let bad_read_timeout = check_source(
        r#"async fn inspect() -> Result<uint, NetworkError> {
connected = await Async.connect(SocketAddress("127.0.0.1", 1))
var stream = connected?
future = stream.read_async_timeout(1, "soon")
return Ok(0)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(bad_read_timeout.message.contains("Duration"));
    assert_eq!(bad_read_timeout.span.start.line, 4);
}
