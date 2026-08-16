use disp::{
    backend::{self, BuildOptions},
    check_source, effect_report_source, lower_source, run_source,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

fn unique_path(label: &str, extension: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "disp-crypto-language-{label}-{}-{}.{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed),
        extension
    ))
}

fn native(source: &str) -> (String, String) {
    let path = unique_path("source", "disp");
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
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
    let output = match Command::new(&artifacts.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "disp-crypto-language-launch-{}-{}.exe",
                    std::process::id(),
                    NEXT_PATH.fetch_add(1, Ordering::Relaxed)
                ));
            fs::copy(&artifacts.executable, &fallback).unwrap();
            let output = Command::new(&fallback).output().unwrap();
            fs::remove_file(fallback).unwrap();
            output
        }
        Err(error) => panic!("native cryptography execution failed: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        generated,
    )
}

#[test]
fn secure_randomness_is_typed_and_requires_the_random_capability() {
    let source = r#"
fn entropy(length: uint) -> Result<List<u8>, CryptoError> uses Random {
    return Crypto.random_bytes(length)
}
fn inferred() -> Result<List<u8>, CryptoError> = Crypto.random_bytes(32)
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    let report = effect_report_source(source).unwrap().render();
    assert!(report.contains("entropy uses Random"), "{report}");
    assert!(
        report.contains("inferred uses Random [inferred]"),
        "{report}"
    );

    let denied = check_source(
        "fn entropy() -> Result<List<u8>, CryptoError> uses Pure = Crypto.random_bytes(32)\nfn main() uses Pure {}",
    )
    .unwrap_err();
    assert!(
        denied.message.contains("requires capability `Random`"),
        "{}",
        denied.message
    );
    let wrong_type =
        check_source("fn main() uses Random { let value = Crypto.random_bytes(\"32\") }")
            .unwrap_err();
    assert!(wrong_type.message.contains("length must be an integer"));
}

#[test]
fn interpreter_randomness_is_bounded_and_fail_closed() {
    let source = r#"
fn sample() -> Result<uint, CryptoError> uses Random {
    let bytes = Crypto.random_bytes(32)?
    return Ok(bytes.len())
}
fn main() uses Random {
    print(sample())
    print(Crypto.random_bytes(0))
    print(Crypto.random_bytes(1048577))
    print(Crypto.random_bytes(-1))
    print(Crypto.random_bytes(340282366920938463463374607431768211455))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "Result.Ok(32)",
            "Result.Err(secure-random byte length must be between 1 and 1048576)",
            "Result.Err(secure-random byte length must be between 1 and 1048576)",
            "Result.Err(secure-random byte length must be a non-negative platform-sized integer)",
            "Result.Err(secure-random byte length must be a non-negative platform-sized integer)",
        ]
    );
}

#[test]
fn native_randomness_matches_interpreter_and_uses_the_os_provider() {
    let source = r#"
fn sample() -> Result<uint, CryptoError> uses Random {
    let bytes = Crypto.random_bytes(32)?
    return Ok(bytes.len())
}
fn rejects_zero() -> bool uses Random {
    return match Crypto.random_bytes(0) { Ok(bytes) => false, Err(error) => true }
}
fn rejects_negative() -> bool uses Random {
    return match Crypto.random_bytes(-1) { Ok(bytes) => false, Err(error) => true }
}
fn rejects_platform_overflow(value: u128) -> bool uses Random {
    return match Crypto.random_bytes(value) { Ok(bytes) => false, Err(error) => true }
}
fn main() uses Random {
    print(sample())
    print(rejects_zero())
    print(rejects_negative())
    print(rejects_platform_overflow(340282366920938463463374607431768211455))
}
"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native(source);
    assert_eq!(actual, expected);
    assert!(generated.contains("BCryptGenRandom"));
    assert!(generated.contains("SYS_getrandom"));
    assert!(!generated.contains("rand()"));
    assert!(!generated.contains("srand("));
}

#[test]
fn secret_bytes_are_opaque_noncopy_and_confined_to_the_current_thread() {
    let valid = r#"
fn secret(length: uint) -> Result<SecretBytes, CryptoError> uses Random {
    return Crypto.random_secret(length)
}
fn main() uses Pure {}
"#;
    check_source(valid).unwrap();
    assert!(
        effect_report_source(valid)
            .unwrap()
            .render()
            .contains("secret uses Random")
    );

    let pure = check_source(
        "fn secret() -> Result<SecretBytes, CryptoError> uses Pure = Crypto.random_secret(32) fn main() uses Pure {}",
    )
    .unwrap_err();
    assert!(pure.message.contains("requires capability `Random`"));

    let printed = check_source(
        "fn test()->Result<uint,CryptoError> uses Random { secret=Crypto.random_secret(8)? print(secret) return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(printed.message.contains("cannot be formatted or printed"));

    let compared = check_source(
        "fn test()->Result<bool,CryptoError> uses Random { left=Crypto.random_secret(8)? right=Crypto.random_secret(8)? return Ok(left == right) } fn main(){}",
    )
    .unwrap_err();
    assert!(compared.message.contains("constant_time_equals"));

    let indexed = check_source(
        "fn test()->Result<u8,CryptoError> uses Random { secret=Crypto.random_secret(8)? return Ok(secret[0]) } fn main(){}",
    )
    .unwrap_err();
    assert!(indexed.message.contains("cannot index SecretBytes"));

    let serialized = check_source(
        "fn test()->Result<Json,CryptoError> uses Random { secret=Crypto.random_secret(8)? return Ok(Json.from(secret)) } fn main(){}",
    )
    .unwrap_err();
    assert!(
        serialized.message.contains("not supported")
            || serialized.message.contains("cannot")
            || serialized.message.contains("Json"),
        "{}",
        serialized.message
    );

    let moved = check_source(
        "fn test()->Result<uint,CryptoError> uses Random { secret=Crypto.random_secret(8)? other=secret return Ok(secret.len()) } fn main(){}",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{}", moved.message);

    let thread = check_source(
        "fn consume(secret: SecretBytes)->uint { return secret.len() } fn test()->Result<uint,CryptoError> uses Random { secret=Crypto.random_secret(8)? task=spawn consume(secret) return Ok(task.join()) } fn main(){}",
    )
    .unwrap_err();
    assert!(
        thread.message.contains("thread") || thread.message.contains("transferred"),
        "{}",
        thread.message
    );
}

#[test]
fn interpreter_secret_bytes_are_bounded_comparable_and_always_redacted() {
    let source = r#"
fn inspect() -> Result<bool, CryptoError> uses Random {
    first = Crypto.random_secret(32)?
    shorter = Crypto.random_secret(31)?
    print(first.len())
    print(first.is_empty())
    print(first.constant_time_equals(first))
    print(first.constant_time_equals(shorter))
    return Ok(true)
}
fn main() uses Random {
    print(inspect())
    print(Crypto.random_secret(4))
    print(Crypto.random_secret(0))
    print(Crypto.random_secret(1048577))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "32",
            "false",
            "true",
            "false",
            "Result.Ok(true)",
            "Result.Ok(<SecretBytes:redacted>)",
            "Result.Err(secure-random secret length must be between 1 and 1048576)",
            "Result.Err(secure-random secret length must be between 1 and 1048576)",
        ]
    );
}

#[test]
fn native_secret_bytes_match_interpreter_and_zeroize_before_release() {
    let source = r#"
fn inspect() -> Result<bool, CryptoError> uses Random {
    first = Crypto.random_secret(32)?
    shorter = Crypto.random_secret(31)?
    print(first.len())
    print(first.is_empty())
    print(first.constant_time_equals(first))
    print(first.constant_time_equals(shorter))
    return Ok(true)
}
fn main() uses Random {
    print(inspect())
    print(Crypto.random_secret(4))
    print(Crypto.random_secret(0))
}
"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native(source);
    assert_eq!(actual, expected);
    assert!(generated.contains("disp_crypto_random_secret"));
    assert!(generated.contains("disp_secret_constant_time_equals"));
    assert!(generated.contains("disp_secret_drop"));
    let drop_start = generated
        .find("static void disp_secret_drop")
        .expect("secret drop helper");
    let drop_body = &generated[drop_start..drop_start + 300];
    assert!(drop_body.find("disp_crypto_zero").unwrap() < drop_body.find("disp_dealloc").unwrap());
    assert!(!generated.contains("rand()"));
    assert!(!generated.contains("srand("));
}

#[test]
fn hashing_and_authentication_are_typed_pure_and_borrow_secret_keys() {
    let source = r#"
fn authenticate(key: SecretBytes, message: List<u8>) -> Result<bool, CryptoError> uses Pure {
    authenticator = Crypto.hmac_sha256(key, message)?
    return Crypto.hmac_sha256_verify(key, message, authenticator)
}
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert!(
        effect_report_source(source)
            .unwrap()
            .render()
            .contains("authenticate uses Pure")
    );

    let bad_message = check_source("fn main(){ digest=Crypto.sha256(\"abc\") }").unwrap_err();
    assert!(bad_message.message.contains("List<u8>"));

    let bad_key = check_source(
        "fn main(){ let message: List<u8> = List.new() digest=Crypto.hmac_sha256(message,message) }",
    )
    .unwrap_err();
    assert!(bad_key.message.contains("SecretBytes"));

    let consumed = check_source(
        "fn test()->Result<uint,CryptoError>{ let bytes: List<u8> = List.of(u8(1)) secret=Crypto.import_secret(bytes)? return Ok(bytes.len()) } fn main(){}",
    )
    .unwrap_err();
    assert!(consumed.message.contains("moved"), "{}", consumed.message);
}

#[test]
fn interpreter_sha256_and_hmac_match_known_answers() {
    let source = r#"
fn sha_known() -> Result<bool, CryptoError> uses Pure {
    let message: List<u8> = List.of(u8(97), u8(98), u8(99))
    digest = Crypto.sha256(message)?
    let expected: List<u8> = List.of(u8(186),u8(120),u8(22),u8(191),u8(143),u8(1),u8(207),u8(234),u8(65),u8(65),u8(64),u8(222),u8(93),u8(174),u8(34),u8(35),u8(176),u8(3),u8(97),u8(163),u8(150),u8(23),u8(122),u8(156),u8(180),u8(16),u8(255),u8(97),u8(242),u8(0),u8(21),u8(173))
    if digest.len() != expected.len() { return Ok(false) }
    for index in 0..32 {
        if digest[index] != expected[index] { return Ok(false) }
    }
    return Ok(true)
}
fn hmac_known() -> Result<bool, CryptoError> uses Pure {
    let key_bytes: List<u8> = List.of(u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11))
    key = Crypto.import_secret(key_bytes)?
    let message: List<u8> = List.of(u8(72),u8(105),u8(32),u8(84),u8(104),u8(101),u8(114),u8(101))
    authenticator = Crypto.hmac_sha256(key, message)?
    let expected: List<u8> = List.of(u8(176),u8(52),u8(76),u8(97),u8(216),u8(219),u8(56),u8(83),u8(92),u8(168),u8(175),u8(206),u8(175),u8(11),u8(241),u8(43),u8(136),u8(29),u8(194),u8(0),u8(201),u8(131),u8(61),u8(167),u8(38),u8(233),u8(55),u8(108),u8(46),u8(50),u8(207),u8(247))
    valid = Crypto.hmac_sha256_verify(key, message, expected)?
    wrong = Crypto.hmac_sha256_verify(key, message, List.of(u8(0)))?
    return Ok(authenticator.len() == 32 && valid && !wrong)
}
fn main() uses Pure {
    print(sha_known())
    print(hmac_known())
    let empty: List<u8> = List.new()
    print(Crypto.import_secret(empty))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "Result.Ok(true)",
            "Result.Ok(true)",
            "Result.Ok(<SecretBytes:redacted>)",
        ]
    );
}

#[test]
fn native_sha256_and_hmac_use_platform_providers_and_match_the_interpreter() {
    let source = r#"
fn sha_known() -> Result<bool, CryptoError> uses Pure {
    let message: List<u8> = List.of(u8(97), u8(98), u8(99))
    digest = Crypto.sha256(message)?
    let expected: List<u8> = List.of(u8(186),u8(120),u8(22),u8(191),u8(143),u8(1),u8(207),u8(234),u8(65),u8(65),u8(64),u8(222),u8(93),u8(174),u8(34),u8(35),u8(176),u8(3),u8(97),u8(163),u8(150),u8(23),u8(122),u8(156),u8(180),u8(16),u8(255),u8(97),u8(242),u8(0),u8(21),u8(173))
    if digest.len() != expected.len() { return Ok(false) }
    for index in 0..32 {
        if digest[index] != expected[index] { return Ok(false) }
    }
    return Ok(true)
}
fn hmac_known() -> Result<bool, CryptoError> uses Pure {
    let key_bytes: List<u8> = List.of(u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11))
    key = Crypto.import_secret(key_bytes)?
    let message: List<u8> = List.of(u8(72),u8(105),u8(32),u8(84),u8(104),u8(101),u8(114),u8(101))
    authenticator = Crypto.hmac_sha256(key, message)?
    let expected: List<u8> = List.of(u8(176),u8(52),u8(76),u8(97),u8(216),u8(219),u8(56),u8(83),u8(92),u8(168),u8(175),u8(206),u8(175),u8(11),u8(241),u8(43),u8(136),u8(29),u8(194),u8(0),u8(201),u8(131),u8(61),u8(167),u8(38),u8(233),u8(55),u8(108),u8(46),u8(50),u8(207),u8(247))
    valid = Crypto.hmac_sha256_verify(key, message, expected)?
    wrong = Crypto.hmac_sha256_verify(key, message, List.of(u8(0)))?
    return Ok(authenticator.len() == 32 && valid && !wrong)
}
fn main() uses Pure {
    print(sha_known())
    print(hmac_known())
    let empty: List<u8> = List.new()
    print(Crypto.import_secret(empty))
}
"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native(source);
    assert_eq!(actual, expected);
    assert!(generated.contains("BCryptOpenAlgorithmProvider"));
    assert!(generated.contains("BCRYPT_ALG_HANDLE_HMAC_FLAG"));
    assert!(generated.contains("AF_ALG"));
    assert!(generated.contains("hmac(sha256)"));
    let import_start = generated
        .find("static bool disp_crypto_import_secret")
        .expect("secret import helper");
    let import_body = &generated[import_start..import_start + 500];
    assert!(
        import_body.find("disp_crypto_zero").unwrap() < import_body.find("disp_dealloc").unwrap()
    );
    assert!(!generated.contains("rand()"));
}

#[test]
fn hkdf_is_typed_pure_bounded_and_borrows_all_key_material() {
    let source = r#"
fn derive(salt: List<u8>, input: SecretBytes, info: List<u8>) -> Result<uint, CryptoError> uses Pure {
    output = Crypto.hkdf_sha256(salt, input, info, 42)?
    return Ok(output.len() + input.len() + salt.len() + info.len())
}
fn main() uses Pure {}
"#;
    check_source(source).unwrap();
    assert!(
        effect_report_source(source)
            .unwrap()
            .render()
            .contains("derive uses Pure")
    );

    for (source, expected) in [
        (
            "fn bad(input: SecretBytes, bytes: List<u8>){ value=Crypto.hkdf_sha256(\"salt\",input,bytes,32) } fn main(){}",
            "salt must be List<u8>",
        ),
        (
            "fn bad(bytes: List<u8>){ value=Crypto.hkdf_sha256(bytes,bytes,bytes,32) } fn main(){}",
            "input key material must be SecretBytes",
        ),
        (
            "fn bad(input: SecretBytes, bytes: List<u8>){ value=Crypto.hkdf_sha256(bytes,input,bytes,\"32\") } fn main(){}",
            "output length must be an integer",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn interpreter_hkdf_sha256_matches_rfc5869_and_rejects_invalid_lengths() {
    let source = r#"
fn known_answer() -> Result<bool, CryptoError> uses Pure {
    let input_bytes: List<u8> = List.of(u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11))
    input = Crypto.import_secret(input_bytes)?
    let salt: List<u8> = List.of(u8(0),u8(1),u8(2),u8(3),u8(4),u8(5),u8(6),u8(7),u8(8),u8(9),u8(10),u8(11),u8(12))
    let info: List<u8> = List.of(u8(240),u8(241),u8(242),u8(243),u8(244),u8(245),u8(246),u8(247),u8(248),u8(249))
    output = Crypto.hkdf_sha256(salt, input, info, 42)?
    let expected_bytes: List<u8> = List.of(u8(60),u8(178),u8(95),u8(37),u8(250),u8(172),u8(213),u8(122),u8(144),u8(67),u8(79),u8(100),u8(208),u8(54),u8(47),u8(42),u8(45),u8(45),u8(10),u8(144),u8(207),u8(26),u8(90),u8(76),u8(93),u8(176),u8(45),u8(86),u8(236),u8(196),u8(197),u8(191),u8(52),u8(0),u8(114),u8(8),u8(213),u8(184),u8(135),u8(24),u8(88),u8(101))
    expected = Crypto.import_secret(expected_bytes)?
    return Ok(output.constant_time_equals(expected) && input.len() == 22 && salt.len() == 13 && info.len() == 10)
}
fn invalid(length: int) -> Result<SecretBytes, CryptoError> uses Pure {
    let input_bytes: List<u8> = List.new()
    input = Crypto.import_secret(input_bytes)?
    let salt: List<u8> = List.new()
    let info: List<u8> = List.new()
    return Crypto.hkdf_sha256(salt, input, info, length)
}
fn main() uses Pure {
    print(known_answer())
    print(invalid(0))
    print(invalid(8161))
    print(invalid(-1))
}
"#;
    assert_eq!(
        run_source(source).unwrap(),
        [
            "Result.Ok(true)",
            "Result.Err(HKDF-SHA-256 requested 0 bytes but the maximum is 8160)",
            "Result.Err(HKDF-SHA-256 requested 8161 bytes but the maximum is 8160)",
            "Result.Err(HKDF-SHA-256 output length must be a non-negative platform-sized integer)",
        ]
    );
}

#[test]
fn native_hkdf_sha256_matches_rfc5869_and_zeroizes_intermediates() {
    let source = r#"
fn known_answer() -> Result<bool, CryptoError> uses Pure {
    let input_bytes: List<u8> = List.of(u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11),u8(11))
    input = Crypto.import_secret(input_bytes)?
    let salt: List<u8> = List.of(u8(0),u8(1),u8(2),u8(3),u8(4),u8(5),u8(6),u8(7),u8(8),u8(9),u8(10),u8(11),u8(12))
    let info: List<u8> = List.of(u8(240),u8(241),u8(242),u8(243),u8(244),u8(245),u8(246),u8(247),u8(248),u8(249))
    output = Crypto.hkdf_sha256(salt, input, info, 42)?
    let expected_bytes: List<u8> = List.of(u8(60),u8(178),u8(95),u8(37),u8(250),u8(172),u8(213),u8(122),u8(144),u8(67),u8(79),u8(100),u8(208),u8(54),u8(47),u8(42),u8(45),u8(45),u8(10),u8(144),u8(207),u8(26),u8(90),u8(76),u8(93),u8(176),u8(45),u8(86),u8(236),u8(196),u8(197),u8(191),u8(52),u8(0),u8(114),u8(8),u8(213),u8(184),u8(135),u8(24),u8(88),u8(101))
    expected = Crypto.import_secret(expected_bytes)?
    return Ok(output.constant_time_equals(expected))
}

fn invalid(length: int) -> Result<SecretBytes, CryptoError> uses Pure {
    let input_bytes: List<u8> = List.new()
    input = Crypto.import_secret(input_bytes)?
    let salt: List<u8> = List.new()
    let info: List<u8> = List.new()
    return Crypto.hkdf_sha256(salt, input, info, length)
}
fn main() uses Pure {
    print(known_answer())
    print(invalid(0))
    print(invalid(8161))
    print(invalid(-1))
}
"#;
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let (actual, generated) = native(source);
    assert_eq!(actual, expected);
    assert!(generated.contains("disp_crypto_hkdf_sha256"));
    let hkdf_start = generated
        .find("static bool disp_crypto_hkdf_sha256")
        .expect("HKDF helper");
    let hkdf_body = &generated[hkdf_start..hkdf_start + 3000];
    assert!(hkdf_body.contains("disp_crypto_zero(prk"));
    assert!(hkdf_body.contains("disp_crypto_zero(block"));
    assert!(
        hkdf_body.find("disp_crypto_zero(message").unwrap()
            < hkdf_body.find("disp_dealloc(message)").unwrap()
    );
    assert!(generated.contains("BCryptOpenAlgorithmProvider"));
    assert!(generated.contains("hmac(sha256)"));
}

#[test]
fn interpreter_authenticated_encryption_is_opaque_and_fail_closed() {
    let source = r#"
fn encrypt_and_open(changed_context: bool, wrong_key: bool) -> Result<bool, CryptoError> {
    let key_bytes: List<u8> = List.of(u8(0),u8(1),u8(2),u8(3),u8(4),u8(5),u8(6),u8(7),u8(8),u8(9),u8(10),u8(11),u8(12),u8(13),u8(14),u8(15),u8(16),u8(17),u8(18),u8(19),u8(20),u8(21),u8(22),u8(23),u8(24),u8(25),u8(26),u8(27),u8(28),u8(29),u8(30),u8(31))
    key = Crypto.import_secret(key_bytes)?
    let other_bytes: List<u8> = List.of(u8(31),u8(30),u8(29),u8(28),u8(27),u8(26),u8(25),u8(24),u8(23),u8(22),u8(21),u8(20),u8(19),u8(18),u8(17),u8(16),u8(15),u8(14),u8(13),u8(12),u8(11),u8(10),u8(9),u8(8),u8(7),u8(6),u8(5),u8(4),u8(3),u8(2),u8(1),u8(0))
    other = Crypto.import_secret(other_bytes)?
    let plaintext_bytes: List<u8> = List.of(u8(68),u8(73),u8(83),u8(80))
    plaintext = Crypto.import_secret(plaintext_bytes)?
    let context: List<u8> = List.of(u8(1),u8(2),u8(3))
    envelope = Crypto.aes256_gcm_siv_seal(key, plaintext, context)?
    let changed: List<u8> = List.of(u8(1),u8(2),u8(4))
    if wrong_key {
        opened = Crypto.aes256_gcm_siv_open(other, envelope, context)?
        return Ok(opened.constant_time_equals(plaintext))
    }
    if changed_context {
        opened = Crypto.aes256_gcm_siv_open(key, envelope, changed)?
        return Ok(opened.constant_time_equals(plaintext))
    }
    opened = Crypto.aes256_gcm_siv_open(key, envelope, context)?
    return Ok(opened.constant_time_equals(plaintext))
}
fn main() {
    print(encrypt_and_open(false, false))
    print(encrypt_and_open(true, false))
    print(encrypt_and_open(false, true))
}
"#;
    let expected = [
        "Result.Ok(true)",
        "Result.Err(AES-256-GCM-SIV authentication failed)",
        "Result.Err(AES-256-GCM-SIV authentication failed)",
    ];
    assert_eq!(run_source(source).unwrap(), expected);
    let (actual, generated) = native(source);
    assert_eq!(actual, expected.join("\n") + "\n");
    assert!(generated.contains("disp_crypto_native_abi_version"));
    assert!(generated.contains("disp_crypto_aead_seal"));
    assert!(generated.contains("disp_crypto_aead_open"));
}

#[test]
fn authenticated_encryption_rejects_untyped_envelopes() {
    let error = check_source(
        "fn bad(key: SecretBytes, bytes: List<u8>)->Result<SecretBytes,CryptoError>{ return Crypto.aes256_gcm_siv_open(key,bytes,bytes) } fn main(){}",
    )
    .unwrap_err();
    assert!(error.message.contains("AeadEnvelope"));
}

#[test]
fn aead_envelope_format_is_versioned_canonical_and_native_differential() {
    let source = r#"
fn portable_record() -> Result<bool, CryptoError> {
    let key_bytes: List<u8> = List.of(u8(0),u8(1),u8(2),u8(3),u8(4),u8(5),u8(6),u8(7),u8(8),u8(9),u8(10),u8(11),u8(12),u8(13),u8(14),u8(15),u8(16),u8(17),u8(18),u8(19),u8(20),u8(21),u8(22),u8(23),u8(24),u8(25),u8(26),u8(27),u8(28),u8(29),u8(30),u8(31))
    key = Crypto.import_secret(key_bytes)?
    let plaintext_bytes: List<u8> = List.of(u8(68),u8(73),u8(83),u8(80))
    plaintext = Crypto.import_secret(plaintext_bytes)?
    let context: List<u8> = List.of(u8(1))
    envelope = Crypto.aes256_gcm_siv_seal(key, plaintext, context)?
    encoded = Crypto.encode_aead_envelope(envelope)?
    decoded = Crypto.decode_aead_envelope(encoded)?
    opened = Crypto.aes256_gcm_siv_open(key, decoded, context)?
    return Ok(opened.constant_time_equals(plaintext) && encoded.len() == 48)
}
fn malformed_version() -> Result<AeadEnvelope, CryptoError> {
    let encoded: List<u8> = List.of(u8(68),u8(73),u8(83),u8(80),u8(2),u8(1),u8(12),u8(16),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(16),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0))
    return Crypto.decode_aead_envelope(encoded)
}
fn main() {
    print(portable_record())
    print(malformed_version())
}
"#;
    let expected = [
        "Result.Ok(true)",
        "Result.Err(DISP AEAD envelope rejected malformed input)",
    ];
    assert_eq!(run_source(source).unwrap(), expected);
    let (actual, generated) = native(source);
    assert_eq!(actual, expected.join("\n") + "\n");
    assert!(generated.contains("disp_crypto_aead_encode"));
    assert!(generated.contains("disp_crypto_aead_decode"));
}

#[test]
fn aead_envelope_format_rejects_wrong_source_types() {
    let encode = check_source(
        "fn bad(bytes: List<u8>){ value=Crypto.encode_aead_envelope(bytes) } fn main(){}",
    )
    .unwrap_err();
    assert!(encode.message.contains("AeadEnvelope"));
    let decode =
        check_source("fn bad(){ value=Crypto.decode_aead_envelope(\"DISP\") } fn main(){}")
            .unwrap_err();
    assert!(decode.message.contains("List<u8>"));
}

#[test]
fn ed25519_signatures_are_opaque_strict_and_native_differential() {
    let source = r#"
fn verify_release(changed: bool) -> Result<bool, CryptoError> uses Random {
    key = Crypto.ed25519_generate()?
    public_key = Crypto.ed25519_public_key(key)?
    let message: List<u8> = List.of(u8(68),u8(73),u8(83),u8(80))
    signature = Crypto.ed25519_sign(key, message)?
    encoded_public_key = Crypto.encode_ed25519_public_key(public_key)?
    encoded_signature = Crypto.encode_ed25519_signature(signature)?
    portable_public_key = Crypto.decode_ed25519_public_key(encoded_public_key)?
    portable_signature = Crypto.decode_ed25519_signature(encoded_signature)?
    key_id = Crypto.ed25519_key_id(public_key)?
    portable_key_id = Crypto.ed25519_key_id(portable_public_key)?
    if changed {
        let changed_message: List<u8> = List.of(u8(68),u8(73),u8(83),u8(81))
        return Crypto.ed25519_verify_keyed(key_id, portable_public_key, changed_message, portable_signature)
    }
    valid = Crypto.ed25519_verify_keyed(key_id, portable_public_key, message, portable_signature)?
    valid_key_id_lengths = key_id.len() == 32 && portable_key_id.len() == 32
    key_id_secret = Crypto.import_secret(key_id)?
    portable_key_id_secret = Crypto.import_secret(portable_key_id)?
    return Ok(valid && valid_key_id_lengths && key_id_secret.constant_time_equals(portable_key_id_secret))
}
fn wrong_identity() -> Result<bool, CryptoError> uses Random {
    signer = Crypto.ed25519_generate()?
    approved = Crypto.ed25519_generate()?
    signer_public = Crypto.ed25519_public_key(signer)?
    approved_public = Crypto.ed25519_public_key(approved)?
    approved_id = Crypto.ed25519_key_id(approved_public)?
    let message: List<u8> = List.of(u8(1),u8(2),u8(3))
    signature = Crypto.ed25519_sign(signer, message)?
    return Crypto.ed25519_verify_keyed(approved_id, signer_public, message, signature)
}
fn malformed_signature() -> Result<bool, CryptoError> uses Pure {
    let public_key: List<u8> = List.new()
    let message: List<u8> = List.new()
    let signature: List<u8> = List.new()
    return Crypto.ed25519_verify(public_key, message, signature)
}
fn malformed_record() -> bool {
    let encoded_key: List<u8> = List.of(u8(68),u8(73),u8(83),u8(80),u8(1),u8(2),u8(1),u8(32),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0),u8(0))
    return match Crypto.decode_ed25519_signature(encoded_key) { Ok(value) => false, Err(error) => true }
}
fn main() uses Random {
    print(verify_release(false))
    print(verify_release(true))
    print(malformed_signature())
    print(malformed_record())
    print(wrong_identity())
}
"#;
    let expected = [
        "Result.Ok(true)",
        "Result.Ok(false)",
        "Result.Ok(false)",
        "true",
        "Result.Ok(false)",
    ];
    assert_eq!(run_source(source).unwrap(), expected);
    let (actual, generated) = native(source);
    assert_eq!(actual, expected.join("\n") + "\n");
    assert!(generated.contains("disp_crypto_native_ed25519_generate"));
    assert!(generated.contains("disp_crypto_ed25519_sign"));
    assert!(generated.contains("disp_crypto_ed25519_verify"));
    assert!(generated.contains("disp_crypto_ed25519_record"));
    assert!(generated.contains("disp_crypto_ed25519_key_id"));
    assert!(generated.contains("disp_crypto_ed25519_verify_keyed"));
}

#[test]
fn ed25519_typing_effects_and_key_redaction_fail_closed() {
    let pure = check_source(
        "fn key()->Result<Ed25519SigningKey,CryptoError> uses Pure { return Crypto.ed25519_generate() } fn main(){}",
    )
    .unwrap_err();
    assert!(pure.message.contains("Random"));

    let wrong_key = check_source(
        "fn bad(bytes: List<u8>){ signature=Crypto.ed25519_sign(bytes,bytes) } fn main(){}",
    )
    .unwrap_err();
    assert!(wrong_key.message.contains("Ed25519SigningKey"));

    let printable = check_source(
        "fn bad()->Result<uint,CryptoError> uses Random { key=Crypto.ed25519_generate()? print(key) return Ok(0) } fn main(){}",
    )
    .unwrap_err();
    assert!(printable.message.contains("cannot be formatted or printed"));

    let comparable = check_source(
        "fn bad()->Result<bool,CryptoError> uses Random { key=Crypto.ed25519_generate()? return Ok(key == key) } fn main(){}",
    )
    .unwrap_err();
    assert!(comparable.message.contains("cannot be compared"));

    let serializable = check_source(
        "fn bad(key: Ed25519SigningKey)->Result<Json,ConversionError> { return Json.from(key) } fn main(){}",
    )
    .unwrap_err();
    assert!(serializable.message.contains("JSON"));
}

#[test]
fn ed25519_lifecycle_enforces_activation_expiry_and_revocation_differentially() {
    let source = r#"
fn lifecycle(mode: uint) -> Result<bool, CryptoError> uses Random {
    key = Crypto.ed25519_generate()?
    public_key = Crypto.ed25519_public_key(key)?
    key_id = Crypto.ed25519_key_id(public_key)?
    let message: List<u8> = List.of(u8(1),u8(2),u8(3))
    signature = Crypto.ed25519_sign(key, message)?
    if mode == 1 { return Crypto.ed25519_verify_lifecycle(key_id, public_key, message, signature, uint(100), uint(200), false, uint(99)) }
    if mode == 2 { return Crypto.ed25519_verify_lifecycle(key_id, public_key, message, signature, uint(100), uint(200), false, uint(201)) }
    if mode == 3 { return Crypto.ed25519_verify_lifecycle(key_id, public_key, message, signature, uint(100), uint(200), true, uint(150)) }
    if mode == 4 { return Crypto.ed25519_verify_lifecycle(key_id, public_key, message, signature, uint(200), uint(100), false, uint(150)) }
    return Crypto.ed25519_verify_lifecycle(key_id, public_key, message, signature, uint(100), uint(200), false, uint(150))
}
fn main() uses Random {
    print(lifecycle(uint(0)))
    print(lifecycle(uint(1)))
    print(lifecycle(uint(2)))
    print(lifecycle(uint(3)))
    print(lifecycle(uint(4)))
}
"#;
    let expected = [
        "Result.Ok(true)",
        "Result.Ok(false)",
        "Result.Ok(false)",
        "Result.Ok(false)",
        "Result.Err(Ed25519 key lifecycle window rejected malformed input)",
    ];
    assert_eq!(run_source(source).unwrap(), expected);
    let (actual, generated) = native(source);
    assert_eq!(actual, expected.join("\n") + "\n");
    assert!(generated.contains("disp_crypto_ed25519_verify_lifecycle"));
}

#[test]
fn ed25519_lifecycle_policy_is_statically_typed() {
    let error = check_source(
        "fn bad(id:List<u8>,key:List<u8>,message:List<u8>,signature:List<u8>){ value=Crypto.ed25519_verify_lifecycle(id,key,message,signature,true,uint(2),false,uint(1)) } fn main(){}",
    )
    .unwrap_err();
    assert!(error.message.contains("valid-from"));
}

#[test]
fn argon2id_password_hashing_is_fixed_policy_and_native_differential() {
    let source = r#"
fn check_password(wrong: bool) -> Result<bool, CryptoError> uses Random {
    let password_bytes: List<u8> = List.of(u8(99),u8(111),u8(114),u8(114),u8(101),u8(99),u8(116))
    password = Crypto.import_secret(password_bytes)?
    let wrong_bytes: List<u8> = List.of(u8(119),u8(114),u8(111),u8(110),u8(103))
    other = Crypto.import_secret(wrong_bytes)?
    encoded = Crypto.argon2id_hash_password(password)?
    if wrong {
        return Crypto.argon2id_verify_password(other, encoded)
    }
    return Crypto.argon2id_verify_password(password, encoded)
}
fn main() uses Random {
    print(check_password(false))
    print(check_password(true))
}
"#;
    let expected = ["Result.Ok(true)", "Result.Ok(false)"];
    assert_eq!(run_source(source).unwrap(), expected);
    let (actual, generated) = native(source);
    assert_eq!(actual, expected.join("\n") + "\n");
    assert!(generated.contains("disp_crypto_native_argon2id_hash"));
    assert!(generated.contains("disp_crypto_argon2id_verify"));
    assert!(generated.contains("native cryptography ABI version mismatch"));
}

#[test]
fn argon2id_typing_effects_and_hostile_hashes_fail_closed() {
    let pure = check_source(
        "fn hash(password: SecretBytes)->Result<String,CryptoError> uses Pure { return Crypto.argon2id_hash_password(password) } fn main(){}",
    )
    .unwrap_err();
    assert!(pure.message.contains("Random"));

    let wrong = check_source(
        "fn bad(bytes: List<u8>){ hash=Crypto.argon2id_hash_password(bytes) } fn main(){}",
    )
    .unwrap_err();
    assert!(wrong.message.contains("SecretBytes"));

    let source = r#"
fn hostile() -> Result<bool, CryptoError> {
    let password_bytes: List<u8> = List.of(u8(112))
    password = Crypto.import_secret(password_bytes)?
    return Crypto.argon2id_verify_password(password, "$argon2id$v=19$m=1048576,t=2,p=1$c2FsdHNhbHRzYWx0c2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
}
fn main() { print(hostile()) }
"#;
    let output = run_source(source).unwrap();
    assert_eq!(output.len(), 1);
    assert!(output[0].starts_with("Result.Err("));
}
