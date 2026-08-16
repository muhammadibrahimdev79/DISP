//! Stable native cryptographic boundary for generated DISP programs.
//!
//! Algorithms come from vetted RustCrypto crates. The C ABI contains no
//! allocator ownership transfer: callers provide every output buffer.

use aes_gcm_siv::{
    Aes256GcmSiv, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{panic::AssertUnwindSafe, slice};
use zeroize::Zeroizing;

pub const ABI_VERSION: u32 = 1;
pub const KEY_BYTES: usize = 32;
pub const NONCE_BYTES: usize = 12;
pub const TAG_BYTES: usize = 16;
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const ED25519_SECRET_KEY_BYTES: usize = 32;
pub const ED25519_PUBLIC_KEY_BYTES: usize = 32;
pub const ED25519_SIGNATURE_BYTES: usize = 64;
pub const ED25519_KEY_ID_BYTES: usize = 32;
pub const MAX_SIGNATURE_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PASSWORD_BYTES: usize = 1024;
pub const MAX_PASSWORD_HASH_BYTES: usize = 1024;
pub const ARGON2ID_MEMORY_KIB: u32 = 19 * 1024;
pub const ARGON2ID_ITERATIONS: u32 = 2;
pub const ARGON2ID_PARALLELISM: u32 = 1;
pub const ARGON2ID_OUTPUT_BYTES: usize = 32;

pub const OK: i32 = 0;
pub const INVALID_ARGUMENT: i32 = 1;
pub const INVALID_KEY: i32 = 2;
pub const ENTROPY_UNAVAILABLE: i32 = 3;
pub const AUTHENTICATION_FAILED: i32 = 4;
pub const OPERATION_FAILED: i32 = 5;
pub const PANIC_CONTAINED: i32 = 6;

#[unsafe(no_mangle)]
pub extern "C" fn disp_crypto_native_abi_version() -> u32 {
    ABI_VERSION
}

unsafe fn input<'a>(pointer: *const u8, length: usize) -> Result<&'a [u8], i32> {
    if length > MAX_INPUT_BYTES || (length != 0 && pointer.is_null()) {
        return Err(INVALID_ARGUMENT);
    }
    if length == 0 {
        Ok(&[])
    } else {
        // SAFETY: the caller contract requires `length` readable bytes and we
        // rejected null for nonempty input. The slice does not escape the call.
        Ok(unsafe { slice::from_raw_parts(pointer, length) })
    }
}

unsafe fn output<'a>(pointer: *mut u8, length: usize) -> Result<&'a mut [u8], i32> {
    if length != 0 && pointer.is_null() {
        return Err(INVALID_ARGUMENT);
    }
    if length == 0 {
        Ok(&mut [])
    } else {
        // SAFETY: the caller contract requires `length` writable bytes and we
        // rejected null for nonempty output. The slice does not escape.
        Ok(unsafe { slice::from_raw_parts_mut(pointer, length) })
    }
}

/// Seals with AES-256-GCM-SIV and an internally generated 96-bit nonce.
///
/// # Safety
/// Every non-null input/output pointer must remain valid for its declared
/// length for the duration of the call. Input and output ranges must not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_aes256_gcm_siv_seal(
    key: *const u8,
    key_length: usize,
    plaintext: *const u8,
    plaintext_length: usize,
    associated_data: *const u8,
    associated_data_length: usize,
    nonce_output: *mut u8,
    ciphertext_output: *mut u8,
    ciphertext_capacity: usize,
    ciphertext_length: *mut usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if ciphertext_length.is_null() || key_length != KEY_BYTES {
            return if key_length != KEY_BYTES {
                INVALID_KEY
            } else {
                INVALID_ARGUMENT
            };
        }
        let Ok(key) = input(key, key_length) else {
            return INVALID_KEY;
        };
        let Ok(plaintext) = input(plaintext, plaintext_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(associated_data) = input(associated_data, associated_data_length) else {
            return INVALID_ARGUMENT;
        };
        let Some(required) = plaintext_length.checked_add(TAG_BYTES) else {
            return INVALID_ARGUMENT;
        };
        if ciphertext_capacity < required {
            return INVALID_ARGUMENT;
        }
        let Ok(nonce_output) = output(nonce_output, NONCE_BYTES) else {
            return INVALID_ARGUMENT;
        };
        let Ok(ciphertext_output) = output(ciphertext_output, ciphertext_capacity) else {
            return INVALID_ARGUMENT;
        };
        let mut nonce = [0u8; NONCE_BYTES];
        if getrandom::fill(&mut nonce).is_err() {
            return ENTROPY_UNAVAILABLE;
        }
        let Ok(cipher) = Aes256GcmSiv::new_from_slice(key) else {
            return INVALID_KEY;
        };
        let Ok(nonce_value) = Nonce::try_from(nonce.as_slice()) else {
            return OPERATION_FAILED;
        };
        let Ok(ciphertext) = cipher.encrypt(
            &nonce_value,
            Payload {
                msg: plaintext,
                aad: associated_data,
            },
        ) else {
            return OPERATION_FAILED;
        };
        if ciphertext.len() != required {
            return OPERATION_FAILED;
        }
        nonce_output.copy_from_slice(&nonce);
        ciphertext_output[..required].copy_from_slice(&ciphertext);
        ciphertext_length.write(required);
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Opens AES-256-GCM-SIV only after successful authentication.
///
/// # Safety
/// Every non-null input/output pointer must remain valid for its declared
/// length for the duration of the call. Input and output ranges must not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_aes256_gcm_siv_open(
    key: *const u8,
    key_length: usize,
    nonce: *const u8,
    nonce_length: usize,
    ciphertext: *const u8,
    ciphertext_length: usize,
    associated_data: *const u8,
    associated_data_length: usize,
    plaintext_output: *mut u8,
    plaintext_capacity: usize,
    plaintext_length: *mut usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if plaintext_length.is_null() || key_length != KEY_BYTES {
            return if key_length != KEY_BYTES {
                INVALID_KEY
            } else {
                INVALID_ARGUMENT
            };
        }
        if nonce_length != NONCE_BYTES || ciphertext_length < TAG_BYTES {
            return INVALID_ARGUMENT;
        }
        let Ok(key) = input(key, key_length) else {
            return INVALID_KEY;
        };
        let Ok(nonce) = input(nonce, nonce_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(ciphertext) = input(ciphertext, ciphertext_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(associated_data) = input(associated_data, associated_data_length) else {
            return INVALID_ARGUMENT;
        };
        let required = ciphertext_length - TAG_BYTES;
        if plaintext_capacity < required {
            return INVALID_ARGUMENT;
        }
        let Ok(plaintext_output) = output(plaintext_output, plaintext_capacity) else {
            return INVALID_ARGUMENT;
        };
        let Ok(cipher) = Aes256GcmSiv::new_from_slice(key) else {
            return INVALID_KEY;
        };
        let Ok(nonce_value) = Nonce::try_from(nonce) else {
            return INVALID_ARGUMENT;
        };
        let Ok(plaintext) = cipher.decrypt(
            &nonce_value,
            Payload {
                msg: ciphertext,
                aad: associated_data,
            },
        ) else {
            return AUTHENTICATION_FAILED;
        };
        let plaintext = Zeroizing::new(plaintext);
        if plaintext.len() != required {
            return OPERATION_FAILED;
        }
        plaintext_output[..required].copy_from_slice(plaintext.as_slice());
        plaintext_length.write(required);
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Generates an Ed25519 signing key directly into caller-owned secret storage.
///
/// # Safety
/// `secret_key_output` must designate exactly 32 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_ed25519_generate(
    secret_key_output: *mut u8,
    secret_key_capacity: usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if secret_key_capacity != ED25519_SECRET_KEY_BYTES {
            return INVALID_ARGUMENT;
        }
        let Ok(output) = output(secret_key_output, secret_key_capacity) else {
            return INVALID_ARGUMENT;
        };
        let mut seed = Zeroizing::new([0u8; ED25519_SECRET_KEY_BYTES]);
        if getrandom::fill(seed.as_mut_slice()).is_err() {
            return ENTROPY_UNAVAILABLE;
        }
        output.copy_from_slice(seed.as_slice());
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Derives the public verification key without exporting secret material.
///
/// # Safety
/// Input and output pointers must be valid for their declared lengths and must not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_ed25519_public_key(
    secret_key: *const u8,
    secret_key_length: usize,
    public_key_output: *mut u8,
    public_key_capacity: usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if secret_key_length != ED25519_SECRET_KEY_BYTES
            || public_key_capacity != ED25519_PUBLIC_KEY_BYTES
        {
            return INVALID_ARGUMENT;
        }
        let Ok(secret_key) = input(secret_key, secret_key_length) else {
            return INVALID_KEY;
        };
        let Ok(output) = output(public_key_output, public_key_capacity) else {
            return INVALID_ARGUMENT;
        };
        let Ok(seed) = <&[u8; ED25519_SECRET_KEY_BYTES]>::try_from(secret_key) else {
            return INVALID_KEY;
        };
        output.copy_from_slice(&SigningKey::from_bytes(seed).verifying_key().to_bytes());
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Signs a bounded message into a caller-owned 64-byte signature buffer.
///
/// # Safety
/// Input and output pointers must be valid for their declared lengths and must not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_ed25519_sign(
    secret_key: *const u8,
    secret_key_length: usize,
    message: *const u8,
    message_length: usize,
    signature_output: *mut u8,
    signature_capacity: usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if secret_key_length != ED25519_SECRET_KEY_BYTES
            || signature_capacity != ED25519_SIGNATURE_BYTES
            || message_length > MAX_SIGNATURE_MESSAGE_BYTES
        {
            return INVALID_ARGUMENT;
        }
        let Ok(secret_key) = input(secret_key, secret_key_length) else {
            return INVALID_KEY;
        };
        let Ok(message) = input(message, message_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(output) = output(signature_output, signature_capacity) else {
            return INVALID_ARGUMENT;
        };
        let Ok(seed) = <&[u8; ED25519_SECRET_KEY_BYTES]>::try_from(secret_key) else {
            return INVALID_KEY;
        };
        output.copy_from_slice(&SigningKey::from_bytes(seed).sign(message).to_bytes());
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Strictly verifies an Ed25519 signature. Invalid encodings and signatures are
/// reported as `valid = 0`, while API misuse returns a nonzero status.
///
/// # Safety
/// Input pointers must be valid for their lengths and `valid_output` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_ed25519_verify(
    public_key: *const u8,
    public_key_length: usize,
    message: *const u8,
    message_length: usize,
    signature: *const u8,
    signature_length: usize,
    valid_output: *mut u8,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if valid_output.is_null() || message_length > MAX_SIGNATURE_MESSAGE_BYTES {
            return INVALID_ARGUMENT;
        }
        let Ok(public_key) = input(public_key, public_key_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(message) = input(message, message_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(signature) = input(signature, signature_length) else {
            return INVALID_ARGUMENT;
        };
        let valid = <&[u8; ED25519_PUBLIC_KEY_BYTES]>::try_from(public_key)
            .ok()
            .and_then(|bytes| VerifyingKey::from_bytes(bytes).ok())
            .zip(Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify_strict(message, &signature).is_ok());
        valid_output.write(u8::from(valid));
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Computes a domain-separated identifier for a valid, non-weak Ed25519
/// public key.
///
/// # Safety
/// Input and output buffers must be valid, exact, and non-aliasing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_ed25519_key_id(
    public_key: *const u8,
    public_key_length: usize,
    key_id_output: *mut u8,
    key_id_capacity: usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if public_key_length != ED25519_PUBLIC_KEY_BYTES || key_id_capacity != ED25519_KEY_ID_BYTES
        {
            return INVALID_ARGUMENT;
        }
        let Ok(public_key) = input(public_key, public_key_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(output) = output(key_id_output, key_id_capacity) else {
            return INVALID_ARGUMENT;
        };
        let Ok(bytes) = <&[u8; ED25519_PUBLIC_KEY_BYTES]>::try_from(public_key) else {
            return INVALID_ARGUMENT;
        };
        let Ok(key) = VerifyingKey::from_bytes(bytes) else {
            return INVALID_ARGUMENT;
        };
        if key.is_weak() {
            return INVALID_KEY;
        }
        let mut digest = Sha256::new();
        digest.update(b"DISP Ed25519 key identifier v1\0");
        digest.update(public_key);
        output.copy_from_slice(&digest.finalize());
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

fn argon2id_policy() -> Result<Argon2<'static>, i32> {
    Params::new(
        ARGON2ID_MEMORY_KIB,
        ARGON2ID_ITERATIONS,
        ARGON2ID_PARALLELISM,
        Some(ARGON2ID_OUTPUT_BYTES),
    )
    .map(|params| Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    .map_err(|_| OPERATION_FAILED)
}

fn argon2id_hash_uses_policy(hash: &PasswordHash<'_>) -> bool {
    hash.algorithm.as_str() == "argon2id"
        && hash.version == Some(19)
        && hash.params.as_str() == "m=19456,t=2,p=1"
        && hash.params.get_decimal("m") == Some(ARGON2ID_MEMORY_KIB)
        && hash.params.get_decimal("t") == Some(ARGON2ID_ITERATIONS)
        && hash.params.get_decimal("p") == Some(ARGON2ID_PARALLELISM)
        && hash.salt.as_ref().is_some_and(|salt| salt.len() == 22)
        && hash
            .hash
            .as_ref()
            .is_some_and(|output| output.as_bytes().len() == ARGON2ID_OUTPUT_BYTES)
}

/// Hashes a bounded password with the fixed DISP Argon2id policy and a fresh
/// 128-bit salt, writing the PHC string into caller-owned storage.
///
/// # Safety
/// Input/output pointers must be valid for their declared lengths and must not alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_argon2id_hash(
    password: *const u8,
    password_length: usize,
    encoded_output: *mut u8,
    encoded_capacity: usize,
    encoded_length: *mut usize,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if password_length == 0
            || password_length > MAX_PASSWORD_BYTES
            || encoded_capacity < MAX_PASSWORD_HASH_BYTES
            || encoded_length.is_null()
        {
            return INVALID_ARGUMENT;
        }
        let Ok(password) = input(password, password_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(output) = output(encoded_output, encoded_capacity) else {
            return INVALID_ARGUMENT;
        };
        let mut salt_bytes = Zeroizing::new([0u8; 16]);
        if getrandom::fill(salt_bytes.as_mut_slice()).is_err() {
            return ENTROPY_UNAVAILABLE;
        }
        let Ok(salt) = SaltString::encode_b64(salt_bytes.as_slice()) else {
            return OPERATION_FAILED;
        };
        let Ok(policy) = argon2id_policy() else {
            return OPERATION_FAILED;
        };
        let Ok(encoded) = policy.hash_password(password, &salt) else {
            return OPERATION_FAILED;
        };
        let encoded = encoded.to_string();
        if encoded.is_empty() || encoded.len() > MAX_PASSWORD_HASH_BYTES {
            return OPERATION_FAILED;
        }
        output[..encoded.len()].copy_from_slice(encoded.as_bytes());
        encoded_length.write(encoded.len());
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}

/// Verifies only canonical hashes using DISP's fixed policy, rejecting hostile
/// resource parameters before invoking Argon2.
///
/// # Safety
/// Inputs must be readable for their declared lengths and `valid_output` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn disp_crypto_native_argon2id_verify(
    password: *const u8,
    password_length: usize,
    encoded_hash: *const u8,
    encoded_hash_length: usize,
    valid_output: *mut u8,
) -> i32 {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        if password_length == 0
            || password_length > MAX_PASSWORD_BYTES
            || encoded_hash_length == 0
            || encoded_hash_length > MAX_PASSWORD_HASH_BYTES
            || valid_output.is_null()
        {
            return INVALID_ARGUMENT;
        }
        let Ok(password) = input(password, password_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(encoded_hash) = input(encoded_hash, encoded_hash_length) else {
            return INVALID_ARGUMENT;
        };
        let Ok(encoded_hash) = std::str::from_utf8(encoded_hash) else {
            return INVALID_ARGUMENT;
        };
        let Ok(parsed) = PasswordHash::new(encoded_hash) else {
            return INVALID_ARGUMENT;
        };
        if !argon2id_hash_uses_policy(&parsed) {
            return INVALID_ARGUMENT;
        }
        let Ok(policy) = argon2id_policy() else {
            return OPERATION_FAILED;
        };
        match policy.verify_password(password, &parsed) {
            Ok(()) => valid_output.write(1),
            Err(argon2::password_hash::Error::Password) => valid_output.write(0),
            Err(_) => return OPERATION_FAILED,
        }
        OK
    }))
    .unwrap_or(PANIC_CONTAINED)
}
