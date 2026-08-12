use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-ip-dns-{label}-{}-{}.disp",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ))
}

fn native(name: &str, source: &str, emit_c: bool) -> Option<(String, Option<String>)> {
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
                return Some((
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .replace("\r\n", "\n"),
                    generated,
                ));
            }
            Err(error) if error.raw_os_error() == Some(4551) => {
                std::thread::sleep(Duration::from_millis(100));
            }
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
fn ipv4_ipv6_parse_format_classification_and_copy_are_differential() {
    let source = r#"fn inspect() -> Result<bool, NetworkError> {
v4 = IpAddress.parse("127.0.0.1")?
v6 = IpAddress.parse("2001:0db8:0:0:0:0:0:1")?
loop6 = IpAddress.parse("::1")?
zero = IpAddress.parse("::")?
address = SocketAddress(v4, 8080)
print(v4.as_string())
print(v6.as_string())
print(v4)
print(IpAddress.parse("192.0.2.1"))
print(v4.is_ipv4())
print(v6.is_ipv6())
print(loop6.is_loopback())
print(zero.is_unspecified())
print(v4.as_string())
return Ok(true)
}
fn main() { print(inspect()) }"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "127.0.0.1",
            "2001:db8::1",
            "127.0.0.1",
            "Result.Ok(192.0.2.1)",
            "true",
            "true",
            "true",
            "true",
            "127.0.0.1",
            "Result.Ok(true)"
        ]
    );
    let generated = differential("ip-values", source).unwrap();
    assert!(generated.contains("uint8_t bytes[16]"));
    assert!(generated.contains("disp_ip_address_parse"));
    assert!(generated.contains("disp_socket_address_from_ip"));
}

#[test]
fn invalid_ip_and_dns_failures_are_typed_results() {
    let source = r#"fn main() {
print(match IpAddress.parse("999.1.1.1") { Ok(ip) => false, Err(error) => true })
print(match IpAddress.parse("1.2.3.4 trailing") { Ok(ip) => false, Err(error) => true })
print(match Dns.resolve("invalid host name with spaces") { Ok(values) => false, Err(error) => true })
}"#;
    assert_eq!(run_source(source).unwrap(), ["true", "true", "true"]);
    differential("typed-errors", source).unwrap();
}

#[test]
fn synchronous_and_asynchronous_dns_are_sorted_deduplicated_and_differential() {
    let source = r#"fn sync_lookup() -> Result<uint, NetworkError> {
addresses = Dns.resolve("localhost")?
print(addresses.is_empty() == false)
first = addresses[0]
print(first.as_string().is_empty() == false)
return Ok(addresses.len())
}
async fn async_lookup() -> Result<uint, NetworkError> {
addresses = (await Async.resolve_timeout("localhost", Duration.from_seconds(2)))?
print(addresses.is_empty() == false)
first = addresses[0]
print(first.is_loopback())
return Ok(addresses.len())
}
async fn main() {
sync = sync_lookup()
print(match sync { Ok(count) => count > 0, Err(error) => false })
async_result = await async_lookup()
print(match async_result { Ok(count) => count > 0, Err(error) => false })
}"#;
    assert_eq!(
        run_source(source).unwrap(),
        ["true", "true", "true", "true", "true", "true"]
    );
    let generated = differential("dns", source).unwrap();
    assert!(generated.contains("disp_dns_worker"));
    assert!(generated.contains("qsort"));
    assert!(generated.contains("disp_dns_poll"));
}

#[test]
fn dns_futures_are_lazy_and_zero_deadlines_are_deterministic() {
    let source = r#"async fn main() {
unused = Async.resolve("invalid host name with spaces")
timed = await Async.resolve_timeout("localhost", Duration.from_millis(0))
print(match timed { Ok(addresses) => false, Err(error) => true })
}"#;
    assert_eq!(run_source(source).unwrap(), ["true"]);
    differential("lazy-timeout", source).unwrap();
}

#[test]
fn dns_and_ip_type_errors_have_exact_source_spans() {
    let parse = check_source("fn main() { value = IpAddress.parse(42) }").unwrap_err();
    assert!(parse.message.contains("String or str"), "{parse}");
    assert_eq!(parse.span.start.line, 1);

    let resolve = check_source("fn main() { value = Dns.resolve(Path(\"x\")) }").unwrap_err();
    assert!(resolve.message.contains("DNS host"), "{resolve}");
    assert_eq!(resolve.span.start.line, 1);

    let timeout =
        check_source("async fn main() { value = Async.resolve_timeout(\"localhost\", 10) }")
            .unwrap_err();
    assert!(timeout.message.contains("Duration"), "{timeout}");
    assert_eq!(timeout.span.start.line, 1);

    let socket = check_source("fn main() { value = SocketAddress(Path(\"x\"), 80) }").unwrap_err();
    assert!(socket.message.contains("IpAddress"), "{socket}");
    assert_eq!(socket.span.start.line, 1);

    let bad_method = check_source(
        r#"fn inspect() -> Result<bool, NetworkError> {
ip = IpAddress.parse("127.0.0.1")?
return ip.is_ipv4(true)
}
fn main() {}"#,
    )
    .unwrap_err();
    assert!(
        bad_method.message.contains("argument") || bad_method.message.contains("call"),
        "{bad_method}"
    );
    assert_eq!(bad_method.span.start.line, 3);
}
