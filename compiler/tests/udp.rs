use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-udp-{label}-{}-{}",
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

fn differential(name: &str, source: &str) -> Option<String> {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native(name, source, true)?;
    assert_eq!(actual, expected);
    generated
}

#[test]
fn udp_async_datagrams_sender_addresses_and_owned_inputs_are_differential() {
    let source = r#"async fn exchange() -> Result<uint, NetworkError> {
server_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var server = server_bound?
client_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var client = client_bound?
server_address = SocketAddress("127.0.0.1", server.local_port()?)
var outgoing = List.of(u8(1), u8(2), u8(3))
pending = client.send_to_async_timeout(outgoing[0..3], server_address, Duration.from_seconds(1))
outgoing[0] = u8(99)
print((await pending)?)
request = (await server.receive_from_async_timeout(64, Duration.from_seconds(1)))?
first_copy = request.bytes()
first_copy[0] = u8(77)
second_copy = request.bytes()
print(second_copy[0])
print(request.len())
print(request.is_empty())
server.send_to(second_copy, request.source())?
response = (await client.receive_from_async(64))?
bytes = response.bytes()
server.close()
client.close()
return Ok(uint(bytes[2]))
}
async fn main() { print(await exchange()) }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["3", "1", "3", "false", "Result.Ok(3)"]
    );
    let Some(generated) = differential("async-round-trip", source) else {
        return;
    };
    assert!(generated.contains("disp_udp_io_poll"));
    assert!(generated.contains("receive_busy"));
    assert!(generated.contains("send_busy"));
    assert!(generated.contains("disp_socket_address_clone"));
    assert!(generated.contains("atomic_size_t refs"));
}

#[test]
fn udp_sync_zero_length_and_truncation_semantics_are_differential() {
    let source = r#"fn inspect() -> Result<bool, NetworkError> {
receiver_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var receiver = receiver_bound?
sender_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var sender = sender_bound?
target = SocketAddress("127.0.0.1", receiver.local_port()?)
sender.send_to(List.of(u8(1), u8(2), u8(3), u8(4)), target)?
small = receiver.receive_from(3)
print(match small { Ok(packet) => false, Err(error) => true })
let empty: List<u8> = List.new()
sender.send_to(empty, SocketAddress("127.0.0.1", receiver.local_port()?))?
packet = receiver.receive_from(0)?
print(packet.is_empty())
receiver.close()
sender.close()
return Ok(true)
}
fn main() { print(inspect()) }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["true", "true", "Result.Ok(true)"]
    );
    let _ = differential("sync-truncation", source);
}

#[test]
fn udp_futures_are_lazy_and_deadlines_start_on_first_poll() {
    let source = r#"async fn inspect() -> Result<bool, NetworkError> {
receiver_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var receiver = receiver_bound?
sender_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var sender = sender_bound?
target = SocketAddress("127.0.0.1", receiver.local_port()?)
send = sender.send_to_async(List.of(u8(8)), target)
missing = await receiver.receive_from_async_timeout(8, Duration.from_millis(5))
print(match missing { Ok(packet) => false, Err(error) => true })
await send
received = await receiver.receive_from_async_timeout(8, Duration.from_seconds(1))
print(match received { Ok(packet) => true, Err(error) => false })
late = receiver.receive_from_async_timeout(8, Duration.from_millis(5))
await Async.sleep(Duration.from_millis(10))
sender.send_to(List.of(u8(9)), SocketAddress("127.0.0.1", receiver.local_port()?))?
packet = (await late)?
packet_bytes = packet.bytes()
receiver.close()
sender.close()
return Ok(uint(packet_bytes[0]) == 9)
}
async fn main() { print(await inspect()) }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["true", "true", "Result.Ok(true)"]
    );
    let _ = differential("lazy-deadlines", source);
}

#[test]
fn udp_same_direction_operations_serialize_and_closed_sockets_fail_safely() {
    let source = r#"async fn inspect() -> Result<uint, NetworkError> {
receiver_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var receiver = receiver_bound?
sender_bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var sender = sender_bound?
first = Async.spawn(receiver.receive_from_async_timeout(8, Duration.from_seconds(1)))
second = Async.spawn(receiver.receive_from_async_timeout(8, Duration.from_seconds(1)))
target = SocketAddress("127.0.0.1", receiver.local_port()?)
sender.send_to(List.of(u8(4)), target)?
sender.send_to(List.of(u8(5)), SocketAddress("127.0.0.1", receiver.local_port()?))?
left = (await first)?
right = (await second)?
left_bytes = left.bytes()
right_bytes = right.bytes()
pending = receiver.receive_from_async(1)
receiver.close()
closed = await pending
print(match closed { Ok(packet) => false, Err(error) => true })
sender.close()
return Ok(uint(left_bytes[0]) + uint(right_bytes[0]))
}
async fn main() { print(await inspect()) }"#;
    assert_eq!(run_source(source).unwrap(), ["true", "Result.Ok(9)"]);
    let _ = differential("serialization-close", source);
}

#[test]
fn closing_a_natively_polled_udp_receive_is_cancellation_safe() {
    let source = r#"async fn inspect() -> Result<bool, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var socket = bound?
task = Async.spawn(socket.receive_from_async_timeout(8, Duration.from_seconds(2)))
await Async.yield()
socket.close()
result = await task
return Ok(match result { Ok(packet) => false, Err(error) => true })
}
async fn main() { print(await inspect()) }"#;
    let Some((actual, _)) = native("cancel-polled", source, false) else {
        return;
    };
    assert_eq!(actual, "Result.Ok(true)\n");
}

#[test]
fn explicit_cancellation_releases_a_started_udp_receive() {
    let source = r#"async fn inspect() -> Result<bool, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var socket = bound?
task = Async.spawn(socket.receive_from_async_timeout(8, Duration.from_seconds(30)))
await Async.yield()
task.cancel()
socket.close()
return Ok(true)
}
async fn main() { print(await inspect()) }"#;
    let Some((actual, generated)) = native("explicit-cancel-polled", source, true) else {
        return;
    };
    assert_eq!(actual, "Result.Ok(true)\n");
    let generated = generated.unwrap();
    assert!(generated.contains("disp_task_cancel"));
    assert!(generated.contains("disp_udp_io_drop"));
    assert!(generated.contains("state->socket->receive_busy"));
}

#[test]
fn udp_bind_conflicts_close_and_drop_cleanup_are_differential() {
    let source = r#"fn reserve() -> Result<uint, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var socket = bound?
return socket.local_port()
}
fn inspect() -> Result<bool, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var first = bound?
port = first.local_port()?
conflict = UdpSocket.bind(SocketAddress("127.0.0.1", port))
print(match conflict { Ok(socket) => false, Err(error) => true })
first.close()
rebound = UdpSocket.bind(SocketAddress("127.0.0.1", port))
var second = rebound?
second.close()
dropped_port = reserve()?
after_drop = UdpSocket.bind(SocketAddress("127.0.0.1", dropped_port))
var third = after_drop?
third.close()
closed = third.local_port()
print(match closed { Ok(value) => false, Err(error) => true })
return Ok(true)
}
fn main() { print(inspect()) }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["true", "true", "Result.Ok(true)"]
    );
    let _ = differential("bind-cleanup", source);
}

#[test]
fn udp_type_ownership_and_arity_errors_have_exact_spans() {
    let bad_bytes = check_source(
        r#"fn inspect() -> Result<uint, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var socket = bound?
return socket.send_to(List.of(1, 2), SocketAddress("127.0.0.1", 9))
}
fn main() {}"#,
    )
    .unwrap_err();
    assert!(bad_bytes.message.contains("List<u8>"), "{bad_bytes}");
    assert_eq!(bad_bytes.span.start.line, 4);

    let bad_address = check_source(
        r#"async fn inspect() -> Result<uint, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var socket = bound?
future = socket.send_to_async(List.of(u8(1)), Path("x"))
return Ok(0)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(bad_address.message.contains("SocketAddress"));
    assert_eq!(bad_address.span.start.line, 4);

    let bad_timeout = check_source(
        r#"async fn inspect() -> Result<uint, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
var socket = bound?
future = socket.receive_from_async_timeout(8, "soon")
return Ok(0)
}
async fn main() {}"#,
    )
    .unwrap_err();
    assert!(bad_timeout.message.contains("Duration"));
    assert_eq!(bad_timeout.span.start.line, 4);

    let immutable = check_source(
        r#"fn inspect() -> Result<uint, NetworkError> {
bound = UdpSocket.bind(SocketAddress("127.0.0.1", 0))
let socket = bound?
return socket.local_port()
}
fn main() {}"#,
    )
    .unwrap_err();
    assert!(immutable.message.contains("mutable"), "{immutable}");
    assert_eq!(immutable.span.start.line, 4);

    let moved = check_source(
        r#"fn main() {
address = SocketAddress("127.0.0.1", 0)
bound = UdpSocket.bind(address)
print(address)
}"#,
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{moved}");
    assert_eq!(moved.span.start.line, 4);
}
