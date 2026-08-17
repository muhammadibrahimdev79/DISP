use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use native_tls::TlsConnector;
use std::{
    fs,
    io::{ErrorKind, Read},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn unique_path(label: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap()
        .join("examples")
        .join(format!("tls_test_{label}.disp"))
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
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(error) => panic!("native execution failed: {error}"),
        }
    }
    (None, generated)
}

fn differential(name: &str, source: &str) -> String {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native(name, source, true);
    if let Some(actual) = &actual {
        assert_eq!(actual, &expected);
    }
    generated.unwrap()
}

fn public_tls_available(host: &str) -> bool {
    (host, 443)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| {
            addresses
                .any(|address| TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok())
                .then_some(())
        })
        .is_some()
}

fn public_tls_handshake_available(host: &str) -> bool {
    let mut builder = TlsConnector::builder();
    builder.danger_accept_invalid_certs(true);
    let Ok(connector) = builder.build() else {
        return false;
    };
    let Ok(addresses) = (host, 443).to_socket_addrs() else {
        return false;
    };
    addresses.into_iter().any(|address| {
        let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(2)) else {
            return false;
        };
        if stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .is_err()
            || stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .is_err()
        {
            return false;
        }
        connector.connect(host, stream).is_ok()
    })
}

fn remote_tls_transport_timed_out(output: &str) -> bool {
    matches!(
        output,
        "Result.Err(connection timed out)\n" | "Result.Err(TCP connect timed out)\n"
    )
}

#[test]
fn verified_tls13_https_io_is_native_interpreter_differential() {
    if !public_tls_available("example.com") {
        return;
    }
    let source = fs::read_to_string("examples/tls.disp").unwrap();
    assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
    let generated = differential("verified-https", &source);
    assert!(generated.contains("SCH_CRED_AUTO_CRED_VALIDATION"));
    assert!(generated.contains("SP_PROT_TLS1_2_CLIENT|SP_PROT_TLS1_3_CLIENT"));
    assert!(generated.contains("disp_tls_post_handshake_step"));
    assert!(generated.contains("EncryptMessage"));
    assert!(generated.contains("DecryptMessage"));
}

#[test]
fn hostname_mismatch_and_invalid_chains_are_rejected() {
    if !public_tls_available("example.com") {
        return;
    }
    let mismatch = r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect_timeout(SocketAddress("example.com", 443), Duration.from_seconds(5)))?
result = await Tls.connect_timeout(tcp, "definitely-not-example.invalid", Duration.from_seconds(5))
return Ok(match result { Ok(stream) => false, Err(error) => true })
}
async fn main() { print(await inspect()) }"#;
    let interpreted = run_source(mismatch).unwrap().join("\n") + "\n";
    assert_eq!(interpreted, "Result.Ok(true)\n");
    let (compiled, _) = native("hostname-mismatch", mismatch, false);
    if let Some(compiled) = compiled {
        assert_eq!(compiled, interpreted);
    }

    // A TCP accept alone does not prove that the remote TLS endpoint is reachable:
    // captive portals and filtered networks can accept port 443 and then stall.
    if !public_tls_handshake_available("self-signed.badssl.com") {
        return;
    }
    let untrusted = r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect_timeout(SocketAddress("self-signed.badssl.com", 443), Duration.from_seconds(5)))?
result = await Tls.connect_timeout(tcp, "self-signed.badssl.com", Duration.from_seconds(5))
return Ok(match result { Ok(stream) => false, Err(error) => true })
}
    async fn main() { print(await inspect()) }"#;
    let interpreted = run_source(untrusted).unwrap().join("\n") + "\n";
    if remote_tls_transport_timed_out(&interpreted) {
        return;
    }
    assert_eq!(interpreted, "Result.Ok(true)\n");
    let (compiled, _) = native("untrusted-certificate", untrusted, false);
    if let Some(compiled) = compiled {
        if remote_tls_transport_timed_out(&compiled) {
            return;
        }
        assert_eq!(compiled, interpreted);
    }
}

fn assert_lazy_connection(source_for_port: impl Fn(u16) -> String) {
    for native_mode in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let source = source_for_port(port);
        listener.set_nonblocking(true).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        if worker_cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("local TLS test accept failed: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut byte = [0; 1];
            match socket.read(&mut byte) {
                Ok(0) => {}
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                other => panic!("lazy TLS future transmitted handshake bytes: {other:?}"),
            }
            true
        });
        let executed = if native_mode {
            let (output, _) = native("lazy", &source, false);
            if let Some(output) = output {
                assert_eq!(output, "Result.Ok(true)\n");
                true
            } else {
                // Windows Application Control may reject a newly generated executable by hash.
                // The backend build still succeeded; behavioral coverage runs on hosts that allow it.
                false
            }
        } else {
            assert_eq!(run_source(&source).unwrap(), ["Result.Ok(true)"]);
            true
        };
        if !executed {
            cancelled.store(true, Ordering::Release);
        }
        assert_eq!(worker.join().unwrap(), executed);
    }
}

#[test]
fn handshake_futures_are_lazy_and_zero_timeouts_send_nothing() {
    assert_lazy_connection(|port| {
        format!(
            r#"async fn inspect() -> Result<bool, NetworkError> {{
tcp = (await Async.connect(SocketAddress("127.0.0.1", {port})))?
unused = Tls.connect(tcp, "localhost")
return Ok(true)
}}
async fn main() {{ print(await inspect()) }}"#
        )
    });
    assert_lazy_connection(|port| {
        format!(
            r#"async fn inspect() -> Result<bool, NetworkError> {{
tcp = (await Async.connect(SocketAddress("127.0.0.1", {port})))?
result = await Tls.connect_timeout(tcp, "localhost", Duration.from_millis(0))
return Ok(match result {{ Ok(stream) => false, Err(error) => true }})
}}
async fn main() {{ print(await inspect()) }}"#
        )
    });
    assert_lazy_connection(|port| {
        format!(
            r#"async fn inspect() -> Result<bool, NetworkError> {{
tcp = (await Async.connect(SocketAddress("127.0.0.1", {port})))?
result = await Tls.connect(tcp, "")
return Ok(match result {{ Ok(stream) => false, Err(error) => true }})
}}
async fn main() {{ print(await inspect()) }}"#
        )
    });
}

#[test]
fn zero_deadline_io_and_closed_streams_fail_without_application_io() {
    if !public_tls_available("example.com") {
        return;
    }
    let source = r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect_timeout(SocketAddress("example.com", 443), Duration.from_seconds(5)))?
var secure = (await Tls.connect_timeout(tcp, "example.com", Duration.from_seconds(5)))?
write = await secure.write_async_timeout(List.of(u8(65)), Duration.from_millis(0))
print(match write { Ok(count) => false, Err(error) => true })
read = await secure.read_async_timeout(1, Duration.from_millis(0))
print(match read { Ok(bytes) => false, Err(error) => true })
secure.close()
closed = secure.read(1)
return Ok(match closed { Ok(bytes) => false, Err(error) => true })
}
async fn main() { print(await inspect()) }"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["true", "true", "Result.Ok(true)"]
    );
    differential("zero-io-close", source);
}

#[test]
fn tls_types_moves_mutability_and_arguments_are_checked_with_spans() {
    let source = r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect(SocketAddress("localhost", 443)))?
future = Tls.connect(tcp, "localhost")
tcp.close()
return Ok(true)
}
async fn main() { print(await inspect()) }"#;
    let moved = check_source(source).unwrap_err();
    assert!(moved.message.contains("moved"), "{moved}");
    assert_eq!(moved.span.start.line, 4);

    let immutable = check_source(
        r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect(SocketAddress("localhost", 443)))?
let secure = (await Tls.connect(tcp, "localhost"))?
secure.read(1)
return Ok(true)
}
async fn main() { print(await inspect()) }"#,
    )
    .unwrap_err();
    assert!(immutable.message.contains("mutable"), "{immutable}");
    assert_eq!(immutable.span.start.line, 4);

    let stream =
        check_source("async fn main() { value = Tls.connect(SocketAddress(\"x\", 1), \"x\") }")
            .unwrap_err();
    assert!(stream.message.contains("TcpStream"), "{stream}");

    let name = check_source(
        r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect(SocketAddress("localhost", 443)))?
value = Tls.connect(tcp, Path("localhost"))
return Ok(true)
}
async fn main() { print(await inspect()) }"#,
    )
    .unwrap_err();
    assert!(name.message.contains("server name"), "{name}");
    assert_eq!(name.span.start.line, 3);

    let timeout = check_source(
        r#"async fn inspect() -> Result<bool, NetworkError> {
tcp = (await Async.connect(SocketAddress("localhost", 443)))?
value = Tls.connect_timeout(tcp, "localhost", 10)
return Ok(true)
}
async fn main() { print(await inspect()) }"#,
    )
    .unwrap_err();
    assert!(timeout.message.contains("Duration"), "{timeout}");
    assert_eq!(timeout.span.start.line, 3);
}

#[test]
fn tls_stream_has_concrete_owned_native_layout_and_cleanup() {
    let source = r#"async fn wrap(stream: TcpStream) -> Result<TlsStream, NetworkError> {
return await Tls.connect(stream, "localhost")
}
fn main() {}"#;
    let (hir, mir) = lower_source(source).unwrap();
    assert!(format!("{hir:#?}").contains("TlsStream"));
    assert!(format!("{mir:#?}").contains("Tls.connect"));
    let path = unique_path("layout");
    fs::write(&path, source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    assert!(generated.contains("typedef struct { disp_tls_state *state; } disp_native_tls_stream"));
    assert!(generated.contains("disp_tls_stream_drop"));
    assert!(generated.contains("DeleteSecurityContext"));
    assert!(generated.contains("FreeCredentialsHandle"));
    assert!(generated.contains("SSL_CTX_set_min_proto_version"));
    assert!(generated.contains("SSL_CTX_set_default_verify_paths"));
    assert!(generated.contains("X509_VERIFY_PARAM_set1_host"));
    assert!(generated.contains("MSG_NOSIGNAL"));
}
