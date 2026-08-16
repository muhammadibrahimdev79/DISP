use disp::crypto::{
    AES256_GCM_SIV_TAG_BYTES, ARGON2ID_ITERATIONS, ARGON2ID_MEMORY_KIB, ARGON2ID_PARALLELISM,
    CryptoError, Ed25519SigningKey, MAX_HKDF_SHA256_OUTPUT, MAX_PASSWORD_BYTES, MAX_RANDOM_BYTES,
    SecretBytes, aes256_gcm_siv_open, aes256_gcm_siv_seal, argon2id_hash_password,
    argon2id_verify_password, ed25519_verify_strict, hkdf_sha256, hmac_sha256, hmac_sha256_verify,
    sha256,
};

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}

#[test]
fn sha256_and_hmac_match_published_known_answers() {
    assert_eq!(
        sha256(b"abc").to_vec(),
        decode_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );

    let key = SecretBytes::from_vec(vec![0x0b; 20]).unwrap();
    let expected = decode_hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(hmac_sha256(&key, b"Hi There").unwrap(), expected.as_slice());
    assert!(hmac_sha256_verify(&key, b"Hi There", &expected).unwrap());
    assert!(!hmac_sha256_verify(&key, b"Hi There!", &expected).unwrap());
}

#[test]
fn hkdf_sha256_matches_rfc5869_case_one_and_rejects_invalid_lengths() {
    let input = SecretBytes::from_vec(vec![0x0b; 22]).unwrap();
    let salt = decode_hex("000102030405060708090a0b0c");
    let info = decode_hex("f0f1f2f3f4f5f6f7f8f9");
    let output = hkdf_sha256(Some(&salt), &input, &info, 42).unwrap();
    assert_eq!(
        output.expose_secret(),
        decode_hex(
            "3cb25f25faacd57a90434f64d0362f2a\
             2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865"
        )
        .as_slice()
    );

    for length in [0, MAX_HKDF_SHA256_OUTPUT + 1] {
        assert!(matches!(
            hkdf_sha256(None, &input, b"", length),
            Err(CryptoError::InvalidLength { .. })
        ));
    }
}

#[test]
fn secrets_are_redacted_nonstandard_comparable_and_randomness_is_bounded() {
    let first = SecretBytes::from_vec(vec![1, 2, 3]).unwrap();
    let same = SecretBytes::from_vec(vec![1, 2, 3]).unwrap();
    let different = SecretBytes::from_vec(vec![1, 2, 4]).unwrap();
    assert!(first.constant_time_eq(&same));
    assert!(!first.constant_time_eq(&different));
    let debug = format!("{first:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("1, 2, 3"));

    let random = SecretBytes::random(32).unwrap();
    assert_eq!(random.len(), 32);
    for length in [0, MAX_RANDOM_BYTES + 1] {
        assert!(matches!(
            SecretBytes::random(length),
            Err(CryptoError::InvalidLength { .. })
        ));
    }

    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/crypto.rs"),
    )
    .unwrap();
    assert!(!source.contains("impl Clone for SecretBytes"));
    assert!(source.contains("Zeroizing<Vec<u8>>"));
}

#[test]
fn authenticated_encryption_round_trips_and_rejects_every_tamper_class() {
    let key = SecretBytes::random(32).unwrap();
    let wrong_key = SecretBytes::random(32).unwrap();
    let plaintext = SecretBytes::from_vec(b"private DISP payload".to_vec()).unwrap();
    let envelope = aes256_gcm_siv_seal(&key, &plaintext, b"record:v1").unwrap();
    assert_eq!(
        envelope.ciphertext().len(),
        plaintext.len() + AES256_GCM_SIV_TAG_BYTES
    );
    assert_eq!(
        aes256_gcm_siv_open(&key, &envelope, b"record:v1")
            .unwrap()
            .expose_secret(),
        plaintext.expose_secret()
    );

    assert!(matches!(
        aes256_gcm_siv_open(&wrong_key, &envelope, b"record:v1"),
        Err(CryptoError::AuthenticationFailed(_))
    ));
    assert!(matches!(
        aes256_gcm_siv_open(&key, &envelope, b"record:v2"),
        Err(CryptoError::AuthenticationFailed(_))
    ));
    let (nonce, mut ciphertext) = envelope.into_parts();
    ciphertext[0] ^= 1;
    let tampered = disp::crypto::AeadEnvelope::from_parts(nonce, ciphertext).unwrap();
    assert!(matches!(
        aes256_gcm_siv_open(&key, &tampered, b"record:v1"),
        Err(CryptoError::AuthenticationFailed(_))
    ));

    let short_key = SecretBytes::from_vec(vec![0; 31]).unwrap();
    assert!(matches!(
        aes256_gcm_siv_seal(&short_key, &plaintext, b""),
        Err(CryptoError::InvalidKey(_))
    ));
}

#[test]
fn authenticated_envelopes_have_one_canonical_versioned_encoding() {
    let key = SecretBytes::random(32).unwrap();
    let plaintext = SecretBytes::from_vec(b"portable encrypted record".to_vec()).unwrap();
    let envelope = aes256_gcm_siv_seal(&key, &plaintext, b"schema:v1").unwrap();
    let encoded = envelope.encode();
    assert_eq!(&encoded[..4], b"DISP");
    assert_eq!(&encoded[4..8], &[1, 1, 12, 16]);
    let decoded = disp::crypto::AeadEnvelope::decode(&encoded).unwrap();
    assert_eq!(decoded, envelope);
    assert_eq!(
        aes256_gcm_siv_open(&key, &decoded, b"schema:v1")
            .unwrap()
            .expose_secret(),
        plaintext.expose_secret()
    );

    for index in [0usize, 4, 5, 6, 7, 15] {
        let mut malformed = encoded.clone();
        malformed[index] ^= 1;
        assert!(disp::crypto::AeadEnvelope::decode(&malformed).is_err());
    }
    assert!(disp::crypto::AeadEnvelope::decode(&encoded[..encoded.len() - 1]).is_err());
    let mut trailing = encoded;
    trailing.push(0);
    assert!(disp::crypto::AeadEnvelope::decode(&trailing).is_err());
}

#[test]
fn ed25519_generation_signing_and_strict_verification_are_fail_closed() {
    let signing_key = Ed25519SigningKey::generate().unwrap();
    let public_key = signing_key.public_key();
    let signature = signing_key.sign(b"release manifest");
    assert!(ed25519_verify_strict(
        &public_key,
        b"release manifest",
        &signature
    ));
    assert!(!ed25519_verify_strict(
        &public_key,
        b"changed manifest",
        &signature
    ));
    assert!(!ed25519_verify_strict(
        &public_key[..31],
        b"release manifest",
        &signature
    ));
    assert!(!ed25519_verify_strict(
        &public_key,
        b"release manifest",
        &signature[..63]
    ));

    let debug = format!("{signing_key:?}");
    assert!(debug.contains("<redacted>"));
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/crypto.rs"),
    )
    .unwrap();
    assert!(!source.contains("impl Clone for Ed25519SigningKey"));
}

#[test]
fn ed25519_public_records_are_versioned_typed_and_canonical() {
    let key = Ed25519SigningKey::generate().unwrap();
    let public_key = key.public_key();
    let signature = key.sign(b"portable signature");
    let encoded_key = disp::crypto::encode_ed25519_public_key(&public_key).unwrap();
    let encoded_signature = disp::crypto::encode_ed25519_signature(&signature).unwrap();
    assert_eq!(&encoded_key[..8], b"DISP\x01\x02\x01\x20");
    assert_eq!(&encoded_signature[..8], b"DISP\x01\x03\x01\x40");
    assert_eq!(
        disp::crypto::decode_ed25519_public_key(&encoded_key).unwrap(),
        public_key
    );
    assert_eq!(
        disp::crypto::decode_ed25519_signature(&encoded_signature).unwrap(),
        signature
    );

    let mut wrong_kind = encoded_key.clone();
    wrong_kind[5] = 3;
    assert!(disp::crypto::decode_ed25519_public_key(&wrong_kind).is_err());
    let mut trailing = encoded_signature;
    trailing.push(0);
    assert!(disp::crypto::decode_ed25519_signature(&trailing).is_err());
    assert!(disp::crypto::encode_ed25519_public_key(&public_key[..31]).is_err());

    let key_id = disp::crypto::ed25519_key_id(&public_key).unwrap();
    assert!(
        disp::crypto::ed25519_verify_keyed(
            &key_id,
            &public_key,
            b"portable signature",
            &signature,
        )
        .unwrap()
    );
    let other_id =
        disp::crypto::ed25519_key_id(&Ed25519SigningKey::generate().unwrap().public_key()).unwrap();
    assert!(
        !disp::crypto::ed25519_verify_keyed(
            &other_id,
            &public_key,
            b"portable signature",
            &signature,
        )
        .unwrap()
    );
}

#[test]
fn argon2id_password_hashes_use_fixed_bounded_policy_and_verify_safely() {
    let password = SecretBytes::from_vec(b"correct horse battery staple".to_vec()).unwrap();
    let wrong = SecretBytes::from_vec(b"wrong password".to_vec()).unwrap();
    let encoded = argon2id_hash_password(&password).unwrap();
    assert!(encoded.starts_with(&format!(
        "$argon2id$v=19$m={ARGON2ID_MEMORY_KIB},t={ARGON2ID_ITERATIONS},p={ARGON2ID_PARALLELISM}$"
    )));
    assert!(argon2id_verify_password(&password, &encoded).unwrap());
    assert!(!argon2id_verify_password(&wrong, &encoded).unwrap());

    let hostile = encoded.replace("m=19456", "m=1048576");
    assert!(matches!(
        argon2id_verify_password(&password, &hostile),
        Err(CryptoError::InvalidEncoding(_))
    ));
    assert!(matches!(
        argon2id_verify_password(&password, "not-a-phc-string"),
        Err(CryptoError::InvalidEncoding(_))
    ));
    let oversized = SecretBytes::from_vec(vec![b'x'; MAX_PASSWORD_BYTES + 1]).unwrap();
    assert!(matches!(
        argon2id_hash_password(&oversized),
        Err(CryptoError::InvalidLength { .. })
    ));
}
