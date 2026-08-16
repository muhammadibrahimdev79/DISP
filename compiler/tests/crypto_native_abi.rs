use disp_crypto_native::{
    ABI_VERSION, AUTHENTICATION_FAILED, ED25519_PUBLIC_KEY_BYTES, ED25519_SECRET_KEY_BYTES,
    ED25519_SIGNATURE_BYTES, INVALID_ARGUMENT, INVALID_KEY, KEY_BYTES, MAX_PASSWORD_HASH_BYTES,
    NONCE_BYTES, OK, TAG_BYTES, disp_crypto_native_abi_version,
    disp_crypto_native_aes256_gcm_siv_open, disp_crypto_native_aes256_gcm_siv_seal,
    disp_crypto_native_argon2id_hash, disp_crypto_native_argon2id_verify,
    disp_crypto_native_ed25519_generate, disp_crypto_native_ed25519_key_id,
    disp_crypto_native_ed25519_public_key, disp_crypto_native_ed25519_sign,
    disp_crypto_native_ed25519_verify,
};

#[test]
fn native_crypto_abi_is_explicit_and_versioned() {
    assert_eq!(disp_crypto_native_abi_version(), ABI_VERSION);
    assert_eq!(ABI_VERSION, 1);
    assert_eq!(KEY_BYTES, 32);
    assert_eq!(NONCE_BYTES, 12);
    assert_eq!(TAG_BYTES, 16);
    assert_eq!(ED25519_SECRET_KEY_BYTES, 32);
    assert_eq!(ED25519_PUBLIC_KEY_BYTES, 32);
    assert_eq!(ED25519_SIGNATURE_BYTES, 64);
}

#[test]
fn native_crypto_abi_generates_signs_and_strictly_verifies_ed25519() {
    let mut secret = [0u8; ED25519_SECRET_KEY_BYTES];
    // SAFETY: the output buffer is valid and exactly sized.
    assert_eq!(
        unsafe { disp_crypto_native_ed25519_generate(secret.as_mut_ptr(), secret.len()) },
        OK
    );
    assert_ne!(secret, [0; ED25519_SECRET_KEY_BYTES]);

    let mut public = [0u8; ED25519_PUBLIC_KEY_BYTES];
    // SAFETY: input and output buffers are valid, exact, and non-aliasing.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_public_key(
                secret.as_ptr(),
                secret.len(),
                public.as_mut_ptr(),
                public.len(),
            )
        },
        OK
    );
    let message = b"DISP release manifest";
    let mut signature = [0u8; ED25519_SIGNATURE_BYTES];
    // SAFETY: input and output buffers are valid, exact, and non-aliasing.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_sign(
                secret.as_ptr(),
                secret.len(),
                message.as_ptr(),
                message.len(),
                signature.as_mut_ptr(),
                signature.len(),
            )
        },
        OK
    );
    let mut valid = 0u8;
    // SAFETY: all inputs are valid and `valid` is writable.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_verify(
                public.as_ptr(),
                public.len(),
                message.as_ptr(),
                message.len(),
                signature.as_ptr(),
                signature.len(),
                &mut valid,
            )
        },
        OK
    );
    assert_eq!(valid, 1);

    let mut key_id = [0u8; 32];
    let mut same_key_id = [0u8; 32];
    // SAFETY: exact valid public-key and output buffers are supplied.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_key_id(
                public.as_ptr(),
                public.len(),
                key_id.as_mut_ptr(),
                key_id.len(),
            )
        },
        OK
    );
    // SAFETY: exact valid public-key and output buffers are supplied.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_key_id(
                public.as_ptr(),
                public.len(),
                same_key_id.as_mut_ptr(),
                same_key_id.len(),
            )
        },
        OK
    );
    assert_eq!(key_id, same_key_id);
    assert_ne!(key_id, [0; 32]);

    valid = 1;
    // SAFETY: all inputs are valid; the changed message is an authentication failure.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_verify(
                public.as_ptr(),
                public.len(),
                b"changed".as_ptr(),
                b"changed".len(),
                signature.as_ptr(),
                signature.len(),
                &mut valid,
            )
        },
        OK
    );
    assert_eq!(valid, 0);
}

#[test]
fn native_ed25519_abi_rejects_bad_capacities_without_writing() {
    let mut secret = [0xA5u8; ED25519_SECRET_KEY_BYTES];
    // SAFETY: the deliberately wrong capacity must be rejected before output.
    assert_eq!(
        unsafe { disp_crypto_native_ed25519_generate(secret.as_mut_ptr(), secret.len() - 1) },
        INVALID_ARGUMENT
    );
    assert_eq!(secret, [0xA5; ED25519_SECRET_KEY_BYTES]);

    let message = b"message";
    let mut signature = [0xA5u8; ED25519_SIGNATURE_BYTES];
    // SAFETY: valid buffers are provided with a deliberately short key length.
    assert_eq!(
        unsafe {
            disp_crypto_native_ed25519_sign(
                secret.as_ptr(),
                secret.len() - 1,
                message.as_ptr(),
                message.len(),
                signature.as_mut_ptr(),
                signature.len(),
            )
        },
        INVALID_ARGUMENT
    );
    assert_eq!(signature, [0xA5; ED25519_SIGNATURE_BYTES]);
}

#[test]
fn native_argon2id_abi_enforces_fixed_costs_and_verifies_passwords() {
    let password = b"correct horse battery staple";
    let mut encoded = [0u8; MAX_PASSWORD_HASH_BYTES];
    let mut encoded_length = 0usize;
    // SAFETY: all buffers are valid, bounded, and non-aliasing.
    assert_eq!(
        unsafe {
            disp_crypto_native_argon2id_hash(
                password.as_ptr(),
                password.len(),
                encoded.as_mut_ptr(),
                encoded.len(),
                &mut encoded_length,
            )
        },
        OK
    );
    let encoded = &encoded[..encoded_length];
    assert!(
        std::str::from_utf8(encoded)
            .unwrap()
            .starts_with("$argon2id$v=19$m=19456,t=2,p=1$")
    );

    let mut valid = 0u8;
    // SAFETY: all inputs are valid and `valid` is writable.
    assert_eq!(
        unsafe {
            disp_crypto_native_argon2id_verify(
                password.as_ptr(),
                password.len(),
                encoded.as_ptr(),
                encoded.len(),
                &mut valid,
            )
        },
        OK
    );
    assert_eq!(valid, 1);

    valid = 1;
    // SAFETY: all inputs are valid and the wrong password is a normal negative result.
    assert_eq!(
        unsafe {
            disp_crypto_native_argon2id_verify(
                b"wrong".as_ptr(),
                b"wrong".len(),
                encoded.as_ptr(),
                encoded.len(),
                &mut valid,
            )
        },
        OK
    );
    assert_eq!(valid, 0);

    let hostile = std::str::from_utf8(encoded)
        .unwrap()
        .replace("m=19456", "m=1048576");
    valid = 0xA5;
    // SAFETY: the encoded input is valid memory but deliberately violates policy.
    assert_eq!(
        unsafe {
            disp_crypto_native_argon2id_verify(
                password.as_ptr(),
                password.len(),
                hostile.as_ptr(),
                hostile.len(),
                &mut valid,
            )
        },
        INVALID_ARGUMENT
    );
    assert_eq!(valid, 0xA5);
}

#[test]
fn native_crypto_abi_seals_opens_and_authenticates_before_output() {
    let key = [0x42u8; KEY_BYTES];
    let plaintext = b"opaque DISP secret";
    let associated_data = b"record:v1";
    let mut nonce = [0u8; NONCE_BYTES];
    let mut ciphertext = vec![0u8; plaintext.len() + TAG_BYTES];
    let mut ciphertext_length = 0usize;
    // SAFETY: every buffer is valid, non-aliasing, and sized as declared.
    let sealed = unsafe {
        disp_crypto_native_aes256_gcm_siv_seal(
            key.as_ptr(),
            key.len(),
            plaintext.as_ptr(),
            plaintext.len(),
            associated_data.as_ptr(),
            associated_data.len(),
            nonce.as_mut_ptr(),
            ciphertext.as_mut_ptr(),
            ciphertext.len(),
            &mut ciphertext_length,
        )
    };
    assert_eq!(sealed, OK);
    assert_eq!(ciphertext_length, ciphertext.len());
    assert_ne!(&ciphertext[..plaintext.len()], plaintext);

    let mut opened = vec![0xA5u8; plaintext.len()];
    let mut opened_length = 0usize;
    // SAFETY: every buffer is valid, non-aliasing, and sized as declared.
    let status = unsafe {
        disp_crypto_native_aes256_gcm_siv_open(
            key.as_ptr(),
            key.len(),
            nonce.as_ptr(),
            nonce.len(),
            ciphertext.as_ptr(),
            ciphertext_length,
            associated_data.as_ptr(),
            associated_data.len(),
            opened.as_mut_ptr(),
            opened.len(),
            &mut opened_length,
        )
    };
    assert_eq!(status, OK);
    assert_eq!(opened_length, plaintext.len());
    assert_eq!(opened, plaintext);

    ciphertext[0] ^= 1;
    opened.fill(0xA5);
    opened_length = usize::MAX;
    // SAFETY: every buffer is valid, non-aliasing, and sized as declared.
    let tampered = unsafe {
        disp_crypto_native_aes256_gcm_siv_open(
            key.as_ptr(),
            key.len(),
            nonce.as_ptr(),
            nonce.len(),
            ciphertext.as_ptr(),
            ciphertext.len(),
            associated_data.as_ptr(),
            associated_data.len(),
            opened.as_mut_ptr(),
            opened.len(),
            &mut opened_length,
        )
    };
    assert_eq!(tampered, AUTHENTICATION_FAILED);
    assert_eq!(opened, vec![0xA5; plaintext.len()]);
    assert_eq!(opened_length, usize::MAX);
}

#[test]
fn native_crypto_abi_rejects_bad_keys_pointers_and_capacities_without_writing() {
    let key = [7u8; KEY_BYTES];
    let plaintext = b"value";
    let mut nonce = [0xA5u8; NONCE_BYTES];
    let mut ciphertext = [0xA5u8; 8];
    let mut length = usize::MAX;
    // SAFETY: valid buffers are provided; the deliberately short key and
    // output capacity are part of the checked ABI input.
    let short_key = unsafe {
        disp_crypto_native_aes256_gcm_siv_seal(
            key.as_ptr(),
            KEY_BYTES - 1,
            plaintext.as_ptr(),
            plaintext.len(),
            std::ptr::null(),
            0,
            nonce.as_mut_ptr(),
            ciphertext.as_mut_ptr(),
            ciphertext.len(),
            &mut length,
        )
    };
    assert_eq!(short_key, INVALID_KEY);
    assert_eq!(nonce, [0xA5; NONCE_BYTES]);
    assert_eq!(ciphertext, [0xA5; 8]);
    assert_eq!(length, usize::MAX);

    // SAFETY: the null pointer with a nonzero length is intentionally invalid
    // and must be rejected before it is dereferenced.
    let null_plaintext = unsafe {
        disp_crypto_native_aes256_gcm_siv_seal(
            key.as_ptr(),
            key.len(),
            std::ptr::null(),
            1,
            std::ptr::null(),
            0,
            nonce.as_mut_ptr(),
            ciphertext.as_mut_ptr(),
            ciphertext.len(),
            &mut length,
        )
    };
    assert_eq!(null_plaintext, INVALID_ARGUMENT);
}
