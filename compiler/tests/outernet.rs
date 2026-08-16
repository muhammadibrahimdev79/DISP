use disp::{backend, lower_source, run_source};
use std::{fs, process::Command};

const SOURCE: &str = include_str!("../examples/outernet_packet_network.disp");

const EXPECTED: &[&str] = &[
    "DROP_LOSS",
    "3",
    "DROP_QUEUE_FULL",
    "6",
    "FORWARD",
    "2",
    "2",
    "FORWARD",
    "2",
    "2",
    "FORWARD",
    "1",
    "2",
    "DROP_AUTH",
    "4",
    "DROP_HOP_LIMIT",
    "5",
    "FORWARD",
    "2",
    "3",
    "FORWARD",
    "2",
    "3",
    "FORWARD",
    "1",
    "3",
    "DELIVER",
    "2",
    "22",
    "DROP_DUPLICATE",
    "2",
    "DELIVER",
    "1",
    "11",
    "SUMMARY",
    "2",
    "6",
    "5",
    "11",
    "Result.Ok(true)",
];

#[test]
fn deterministic_packet_network_covers_faults_routing_authentication_and_bounds() {
    assert_eq!(run_source(SOURCE).unwrap(), EXPECTED);
}

#[test]
fn native_packet_network_matches_the_interpreter_exactly() {
    let root = std::env::temp_dir().join(format!("disp-outernet-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("outernet.disp");
    fs::write(&source_path, SOURCE).unwrap();
    let (hir, mir) = lower_source(SOURCE).unwrap();
    let artifact =
        backend::build(&hir, &mir, &source_path, backend::BuildOptions::default()).unwrap();
    let output = match Command::new(&artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("could not execute Outernet fixture: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        EXPECTED.join("\n") + "\n"
    );
    fs::remove_dir_all(root).unwrap();
}
