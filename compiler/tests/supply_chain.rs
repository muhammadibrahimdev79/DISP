use std::{fs, path::Path};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("compiler must be inside the repository")
}

#[test]
fn dependency_audit_is_pinned_scheduled_and_fail_closed() {
    let compiler_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let policy = fs::read_to_string(compiler_root.join(".cargo/audit.toml")).unwrap();
    assert!(policy.contains("ignore = []"));
    assert!(policy.contains("severity_threshold = \"low\""));
    assert!(policy.contains("stale = false"));
    assert!(policy.contains("deny = [\"warnings\"]"));
    assert!(policy.contains("enabled = true"));

    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/security-audit.yml")).unwrap();
    assert!(workflow.contains("cron: \"17 3 * * *\""));
    assert!(workflow.contains("cargo-audit --version 0.22.2 --locked"));
    assert!(workflow.matches("cargo audit").count() >= 2);
    assert!(workflow.contains("compiler/Cargo.lock"));
    assert!(workflow.contains("compiler/crypto-native/Cargo.toml"));
    assert!(workflow.contains("compiler/crypto-native/Cargo.lock"));
    assert!(workflow.contains("compiler/fuzz/Cargo.lock"));
    assert!(workflow.contains("cargo audit --file fuzz/Cargo.lock"));
    assert!(workflow.contains("cargo audit --file crypto-native/Cargo.lock"));
    assert!(!workflow.contains("--ignore"));
}

#[test]
fn libfuzzer_targets_are_pinned_and_continuously_exercised() {
    let compiler_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/security-audit.yml")).unwrap();
    assert!(workflow.contains("cargo-fuzz --version 0.13.2 --locked"));
    assert!(workflow.contains("cargo +${SECURITY_NIGHTLY} fuzz run lexer"));
    assert!(workflow.contains("cargo +${SECURITY_NIGHTLY} fuzz run frontend"));
    assert!(workflow.contains("cargo +${SECURITY_NIGHTLY} fuzz run security_frames"));
    assert!(workflow.contains("-timeout=5"));
    assert!(workflow.contains("security_frames.dict"));

    let target =
        fs::read_to_string(compiler_root.join("fuzz/fuzz_targets/security_frames.rs")).unwrap();
    for decoder in [
        "AeadEnvelope::decode",
        "decode_ed25519_public_key",
        "decode_ed25519_signature",
        "fuzz_decode_frame",
        "fuzz_decode_frames",
    ] {
        assert!(target.contains(decoder), "missing fuzz decoder {decoder}");
    }
    assert!(compiler_root.join("fuzz/Cargo.lock").is_file());
    assert!(
        compiler_root
            .join("fuzz/dictionaries/security_frames.dict")
            .is_file()
    );
}

#[test]
fn release_binaries_embed_and_verify_locked_dependency_provenance() {
    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/security-audit.yml")).unwrap();
    assert!(workflow.contains("cargo-auditable --version 0.7.4 --locked"));
    assert!(workflow.contains("cargo auditable build --release --locked"));
    assert!(workflow.contains("--manifest-path crypto-native/Cargo.toml"));
    assert!(workflow.contains("cargo audit bin target/release/disp"));
    assert!(
        workflow.contains("cargo audit bin crypto-native/target/release/libdisp_crypto_native.so")
    );
}

#[test]
fn rust_asan_regressions_are_pinned_and_fail_closed() {
    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/security-audit.yml")).unwrap();
    assert!(workflow.contains("SECURITY_NIGHTLY: nightly-2026-08-15"));
    assert!(workflow.contains("RUSTFLAGS: -Zsanitizer=address"));
    assert!(workflow.contains("RUSTDOCFLAGS: -Zsanitizer=address"));
    assert!(workflow.contains("detect_leaks=1:halt_on_error=1:abort_on_error=1"));
    assert!(workflow.contains("--target x86_64-unknown-linux-gnu --all-features"));
    assert!(workflow.contains("build-essential pkg-config libssl-dev"));
    for suite in [
        "--lib",
        "--test crypto",
        "--test crypto_native_abi",
        "--test fuzz_smoke",
        "--test security_governance",
    ] {
        assert!(workflow.contains(suite), "sanitizer gate lacks {suite}");
    }
}

fn assert_linux_release_sbom_policy() {
    let root = repository_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/security-audit.yml")).unwrap();
    for required in [
        "SOURCE_DATE_EPOCH",
        "generate_sbom.py --manifest Cargo.toml",
        "--artifact target/release/disp",
        "--manifest crypto-native/Cargo.toml",
        "--artifact crypto-native/target/release/libdisp_crypto_native.so",
        "--manifest fuzz/Cargo.toml",
        "verify_sbom.py --require-native target/sbom/disp.cdx.json",
        "verify_sbom.py --require-native target/sbom/crypto-native.cdx.json",
        "actions/upload-artifact@v7",
        "if-no-files-found: error",
    ] {
        assert!(
            workflow.contains(required),
            "SBOM workflow lacks {required}"
        );
    }

    let generator = fs::read_to_string(root.join("tools/security/generate_sbom.py")).unwrap();
    for required in [
        "cargo",
        "metadata",
        "--locked",
        "Cargo.lock",
        "ldd",
        "dpkg-query",
        "sha256_file",
        "CycloneDX",
        "specVersion",
        "serialNumber",
    ] {
        assert!(
            generator.contains(required),
            "SBOM generator lacks {required}"
        );
    }
    let verifier = fs::read_to_string(root.join("tools/security/verify_sbom.py")).unwrap();
    assert!(verifier.contains("--require-native"));
    assert!(verifier.contains("GitHub-attestable UUID serial number"));
    assert!(verifier.contains("dependency target is absent"));
    assert!(verifier.contains("malformed component hash"));
}

fn assert_windows_release_sbom_policy() {
    let root = repository_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/security-audit.yml")).unwrap();
    for required in [
        "windows-release-security:",
        "runs-on: windows-latest",
        "cargo audit bin target/release/disp.exe",
        "cargo audit bin crypto-native/target/release/disp_crypto_native.dll",
        "--artifact target/release/disp.exe",
        "--artifact crypto-native/target/release/disp_crypto_native.dll",
        "verify_sbom.py --require-native target/sbom/disp-windows.cdx.json",
        "name: disp-windows-cyclonedx-sboms",
    ] {
        assert!(
            workflow.contains(required),
            "Windows SBOM workflow lacks {required}"
        );
    }
    let generator = fs::read_to_string(root.join("tools/security/generate_sbom.py")).unwrap();
    for required in [
        "pe_imports",
        "PE\\0\\0",
        "windows_file_version",
        "System32",
        "windows-api-set-contract",
        "pe-import-resolved-release-artifact",
    ] {
        assert!(
            generator.contains(required),
            "PE SBOM generator lacks {required}"
        );
    }
}

fn assert_macos_release_sbom_policy() {
    let root = repository_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/security-audit.yml")).unwrap();
    for required in [
        "macos-release-security:",
        "runs-on: macos-latest",
        "cargo audit bin crypto-native/target/release/libdisp_crypto_native.dylib",
        "--artifact crypto-native/target/release/libdisp_crypto_native.dylib",
        "verify_sbom.py --require-native target/sbom/disp-macos.cdx.json",
        "name: disp-macos-cyclonedx-sboms",
        "test_generate_sbom.py",
    ] {
        assert!(
            workflow.contains(required),
            "macOS SBOM workflow lacks {required}"
        );
    }
    let generator = fs::read_to_string(root.join("tools/security/generate_sbom.py")).unwrap();
    for required in [
        "macho_imports",
        "MACHO_DYLIB_COMMANDS",
        "LC_RPATH",
        "@executable_path",
        "@loader_path",
        "@rpath/",
        "macos-dyld-shared-cache",
        "macho-load-command-release-artifact",
    ] {
        assert!(
            generator.contains(required),
            "Mach-O SBOM generator lacks {required}"
        );
    }
    assert!(root.join("tools/security/test_generate_sbom.py").is_file());
}

#[test]
fn release_sboms_cover_locked_native_graphs_on_every_desktop_platform() {
    assert_linux_release_sbom_policy();
    assert_windows_release_sbom_policy();
    assert_macos_release_sbom_policy();
}

#[test]
fn signed_release_provenance_is_scoped_and_binds_each_sbom() {
    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/security-audit.yml")).unwrap();
    assert_eq!(workflow.matches("uses: actions/attest@v4").count(), 9);
    assert_eq!(workflow.matches("id-token: write").count(), 3);
    assert_eq!(workflow.matches("attestations: write").count(), 3);
    assert_eq!(workflow.matches("artifact-metadata: write").count(), 3);
    assert_eq!(
        workflow
            .matches("if: github.event_name != 'pull_request'")
            .count(),
        9
    );
    for sbom in [
        "compiler/target/sbom/disp.cdx.json",
        "compiler/target/sbom/crypto-native.cdx.json",
        "compiler/target/sbom/disp-windows.cdx.json",
        "compiler/target/sbom/crypto-native-windows.cdx.json",
        "compiler/target/sbom/disp-macos.cdx.json",
        "compiler/target/sbom/crypto-native-macos.cdx.json",
    ] {
        assert!(
            workflow.contains(&format!("sbom-path: {sbom}")),
            "missing signed SBOM predicate for {sbom}"
        );
    }
    let fuzz_job = workflow
        .split_once("  fuzz-smoke:")
        .unwrap()
        .1
        .split_once("  rust-sanitizers:")
        .unwrap()
        .0;
    assert!(!fuzz_job.contains("id-token: write"));
}
