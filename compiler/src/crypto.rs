//! Vetted cryptographic foundations for the DISP runtime.
//!
//! This module deliberately composes established primitives. It does not
//! contain handwritten cryptographic algorithms.

use aes_gcm_siv::{
    Aes256GcmSiv, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

pub const MAX_SECRET_BYTES: usize = 1024 * 1024;
pub const MAX_RANDOM_BYTES: usize = 1024 * 1024;
pub const MAX_CRYPTO_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_HKDF_SHA256_OUTPUT: usize = 255 * 32;
pub const MAX_HKDF_CONTEXT_BYTES: usize = MAX_SECRET_BYTES;
pub const MAX_AEAD_PLAINTEXT_BYTES: usize = MAX_SECRET_BYTES;
pub const MAX_AEAD_ASSOCIATED_DATA_BYTES: usize = MAX_SECRET_BYTES;
pub const AES256_GCM_SIV_NONCE_BYTES: usize = 12;
pub const AES256_GCM_SIV_TAG_BYTES: usize = 16;
pub const AEAD_ENVELOPE_HEADER_BYTES: usize = 16;
pub const AEAD_ENVELOPE_FORMAT_VERSION: u8 = 1;
pub const AEAD_ENVELOPE_ALGORITHM_AES256_GCM_SIV: u8 = 1;
pub const ED25519_PUBLIC_KEY_BYTES: usize = 32;
pub const ED25519_SIGNATURE_BYTES: usize = 64;
pub const MAX_PASSWORD_BYTES: usize = 1024;
pub const MAX_PASSWORD_HASH_BYTES: usize = 1024;
pub const ARGON2ID_MEMORY_KIB: u32 = 19 * 1024;
pub const ARGON2ID_ITERATIONS: u32 = 2;
pub const ARGON2ID_PARALLELISM: u32 = 1;
pub const ARGON2ID_OUTPUT_BYTES: usize = 32;
pub const ED25519_RECORD_HEADER_BYTES: usize = 8;
pub const ED25519_RECORD_FORMAT_VERSION: u8 = 1;
pub const ED25519_RECORD_ALGORITHM: u8 = 1;
pub const ED25519_PUBLIC_KEY_RECORD_KIND: u8 = 2;
pub const ED25519_SIGNATURE_RECORD_KIND: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    InvalidLength {
        operation: &'static str,
        requested: usize,
        maximum: usize,
    },
    InvalidKey(&'static str),
    InvalidEncoding(&'static str),
    AuthenticationFailed(&'static str),
    OperationFailed {
        operation: &'static str,
        cause: String,
    },
    EntropyUnavailable(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength {
                operation,
                requested,
                maximum,
            } => write!(
                formatter,
                "{operation} requested {requested} bytes but the maximum is {maximum}"
            ),
            Self::InvalidKey(operation) => write!(formatter, "{operation} rejected the key"),
            Self::InvalidEncoding(operation) => {
                write!(formatter, "{operation} rejected malformed input")
            }
            Self::AuthenticationFailed(operation) => {
                write!(formatter, "{operation} authentication failed")
            }
            Self::OperationFailed { operation, cause } => {
                write!(formatter, "{operation} failed: {cause}")
            }
            Self::EntropyUnavailable(cause) => {
                write!(
                    formatter,
                    "secure operating-system entropy is unavailable: {cause}"
                )
            }
        }
    }
}

impl Error for CryptoError {}

/// Owned secret material. It is intentionally non-`Clone`, redacts `Debug`,
/// has no ordinary equality implementation, and zeroizes its allocation on drop.
pub struct SecretBytes {
    bytes: Zeroizing<Vec<u8>>,
}

impl SecretBytes {
    pub fn from_vec(bytes: Vec<u8>) -> Result<Self, CryptoError> {
        let bytes = Zeroizing::new(bytes);
        validate_length("SecretBytes", bytes.len(), MAX_SECRET_BYTES, true)?;
        Ok(Self { bytes })
    }

    pub fn random(length: usize) -> Result<Self, CryptoError> {
        validate_length("SecretBytes.random", length, MAX_RANDOM_BYTES, false)?;
        let mut bytes = Zeroizing::new(vec![0u8; length]);
        getrandom::fill(bytes.as_mut_slice())
            .map_err(|error| CryptoError::EntropyUnavailable(error.to_string()))?;
        Ok(Self { bytes })
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Explicitly borrows secret material. Callers must not persist or log it.
    pub fn expose_secret(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn constant_time_eq(&self, other: &Self) -> bool {
        bool::from(self.bytes.as_slice().ct_eq(other.bytes.as_slice()))
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretBytes")
            .field("length", &self.bytes.len())
            .field("contents", &"<redacted>")
            .finish()
    }
}

/// An authenticated ciphertext with its public nonce. Plaintext is never
/// exposed through formatting, and callers cannot select encryption nonces.
#[derive(Clone, PartialEq, Eq)]
pub struct AeadEnvelope {
    nonce: [u8; AES256_GCM_SIV_NONCE_BYTES],
    ciphertext: Vec<u8>,
}

impl AeadEnvelope {
    pub fn from_parts(
        nonce: [u8; AES256_GCM_SIV_NONCE_BYTES],
        ciphertext: Vec<u8>,
    ) -> Result<Self, CryptoError> {
        validate_length(
            "AES-256-GCM-SIV ciphertext",
            ciphertext.len(),
            MAX_AEAD_PLAINTEXT_BYTES + AES256_GCM_SIV_TAG_BYTES,
            false,
        )?;
        if ciphertext.len() < AES256_GCM_SIV_TAG_BYTES {
            return Err(CryptoError::InvalidEncoding("AES-256-GCM-SIV"));
        }
        Ok(Self { nonce, ciphertext })
    }

    pub fn nonce(&self) -> &[u8; AES256_GCM_SIV_NONCE_BYTES] {
        &self.nonce
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub fn into_parts(self) -> ([u8; AES256_GCM_SIV_NONCE_BYTES], Vec<u8>) {
        (self.nonce, self.ciphertext)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(
            AEAD_ENVELOPE_HEADER_BYTES + self.nonce.len() + self.ciphertext.len(),
        );
        encoded.extend_from_slice(b"DISP");
        encoded.extend_from_slice(&[
            AEAD_ENVELOPE_FORMAT_VERSION,
            AEAD_ENVELOPE_ALGORITHM_AES256_GCM_SIV,
            AES256_GCM_SIV_NONCE_BYTES as u8,
            AES256_GCM_SIV_TAG_BYTES as u8,
        ]);
        encoded.extend_from_slice(&(self.ciphertext.len() as u64).to_be_bytes());
        encoded.extend_from_slice(&self.nonce);
        encoded.extend_from_slice(&self.ciphertext);
        encoded
    }

    pub fn decode(encoded: &[u8]) -> Result<Self, CryptoError> {
        let maximum = AEAD_ENVELOPE_HEADER_BYTES
            + AES256_GCM_SIV_NONCE_BYTES
            + MAX_AEAD_PLAINTEXT_BYTES
            + AES256_GCM_SIV_TAG_BYTES;
        if encoded.len()
            < AEAD_ENVELOPE_HEADER_BYTES + AES256_GCM_SIV_NONCE_BYTES + AES256_GCM_SIV_TAG_BYTES
            || encoded.len() > maximum
            || encoded[..4] != *b"DISP"
            || encoded[4] != AEAD_ENVELOPE_FORMAT_VERSION
            || encoded[5] != AEAD_ENVELOPE_ALGORITHM_AES256_GCM_SIV
            || encoded[6] != AES256_GCM_SIV_NONCE_BYTES as u8
            || encoded[7] != AES256_GCM_SIV_TAG_BYTES as u8
        {
            return Err(CryptoError::InvalidEncoding("DISP AEAD envelope"));
        }
        let ciphertext_length = u64::from_be_bytes(
            encoded[8..16]
                .try_into()
                .map_err(|_| CryptoError::InvalidEncoding("DISP AEAD envelope"))?,
        );
        let ciphertext_length = usize::try_from(ciphertext_length)
            .map_err(|_| CryptoError::InvalidEncoding("DISP AEAD envelope"))?;
        let expected = AEAD_ENVELOPE_HEADER_BYTES
            .checked_add(AES256_GCM_SIV_NONCE_BYTES)
            .and_then(|length| length.checked_add(ciphertext_length))
            .ok_or(CryptoError::InvalidEncoding("DISP AEAD envelope"))?;
        if expected != encoded.len() {
            return Err(CryptoError::InvalidEncoding("DISP AEAD envelope"));
        }
        let nonce_start = AEAD_ENVELOPE_HEADER_BYTES;
        let nonce_end = nonce_start + AES256_GCM_SIV_NONCE_BYTES;
        let nonce = encoded[nonce_start..nonce_end]
            .try_into()
            .map_err(|_| CryptoError::InvalidEncoding("DISP AEAD envelope"))?;
        Self::from_parts(nonce, encoded[nonce_end..].to_vec())
    }
}

impl fmt::Debug for AeadEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AeadEnvelope")
            .field("nonce", &self.nonce)
            .field("ciphertext_length", &self.ciphertext.len())
            .finish()
    }
}

/// An Ed25519 signing key whose secret seed is zeroized by the underlying
/// implementation and cannot be exported through the DISP bootstrap API.
pub struct Ed25519SigningKey {
    key: SigningKey,
}

impl Ed25519SigningKey {
    pub fn generate() -> Result<Self, CryptoError> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut_slice())
            .map_err(|error| CryptoError::EntropyUnavailable(error.to_string()))?;
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
        })
    }

    pub fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_BYTES] {
        self.key.verifying_key().to_bytes()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; ED25519_SIGNATURE_BYTES] {
        self.key.sign(message).to_bytes()
    }
}

impl fmt::Debug for Ed25519SigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ed25519SigningKey")
            .field("public_key", &self.public_key())
            .field("secret", &"<redacted>")
            .finish()
    }
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn hmac_sha256(key: &SecretBytes, message: &[u8]) -> Result<[u8; 32], CryptoError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.expose_secret())
        .map_err(|_| CryptoError::InvalidKey("HMAC-SHA-256"))?;
    mac.update(message);
    Ok(mac.finalize().into_bytes().into())
}

pub fn hmac_sha256_verify(
    key: &SecretBytes,
    message: &[u8],
    expected: &[u8],
) -> Result<bool, CryptoError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.expose_secret())
        .map_err(|_| CryptoError::InvalidKey("HMAC-SHA-256"))?;
    mac.update(message);
    Ok(mac.verify_slice(expected).is_ok())
}

pub fn hkdf_sha256(
    salt: Option<&[u8]>,
    input_key_material: &SecretBytes,
    info: &[u8],
    output_length: usize,
) -> Result<SecretBytes, CryptoError> {
    validate_length("HKDF-SHA-256", output_length, MAX_HKDF_SHA256_OUTPUT, false)?;
    let hkdf = Hkdf::<Sha256>::new(salt, input_key_material.expose_secret());
    let mut output = Zeroizing::new(vec![0u8; output_length]);
    hkdf.expand(info, output.as_mut_slice())
        .map_err(|_| CryptoError::InvalidLength {
            operation: "HKDF-SHA-256",
            requested: output_length,
            maximum: MAX_HKDF_SHA256_OUTPUT,
        })?;
    Ok(SecretBytes { bytes: output })
}

pub fn aes256_gcm_siv_seal(
    key: &SecretBytes,
    plaintext: &SecretBytes,
    associated_data: &[u8],
) -> Result<AeadEnvelope, CryptoError> {
    validate_aead_inputs(key, plaintext.len(), associated_data.len())?;
    let mut nonce = [0u8; AES256_GCM_SIV_NONCE_BYTES];
    getrandom::fill(&mut nonce)
        .map_err(|error| CryptoError::EntropyUnavailable(error.to_string()))?;
    seal_with_nonce(key, plaintext.expose_secret(), associated_data, nonce)
}

pub fn aes256_gcm_siv_open(
    key: &SecretBytes,
    envelope: &AeadEnvelope,
    associated_data: &[u8],
) -> Result<SecretBytes, CryptoError> {
    validate_length(
        "AES-256-GCM-SIV associated data",
        associated_data.len(),
        MAX_AEAD_ASSOCIATED_DATA_BYTES,
        true,
    )?;
    if key.len() != 32 {
        return Err(CryptoError::InvalidKey("AES-256-GCM-SIV"));
    }
    let cipher = Aes256GcmSiv::new_from_slice(key.expose_secret())
        .map_err(|_| CryptoError::InvalidKey("AES-256-GCM-SIV"))?;
    let nonce = Nonce::try_from(envelope.nonce.as_slice())
        .map_err(|_| CryptoError::InvalidEncoding("AES-256-GCM-SIV nonce"))?;
    let plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &envelope.ciphertext,
                aad: associated_data,
            },
        )
        .map_err(|_| CryptoError::AuthenticationFailed("AES-256-GCM-SIV"))?;
    SecretBytes::from_vec(plaintext)
}

pub fn ed25519_verify_strict(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(public_key_bytes) = <&[u8; ED25519_PUBLIC_KEY_BYTES]>::try_from(public_key) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(public_key_bytes) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    verifying_key.verify_strict(message, &signature).is_ok()
}

pub fn ed25519_key_id(public_key: &[u8]) -> Result<[u8; 32], CryptoError> {
    let bytes = <&[u8; ED25519_PUBLIC_KEY_BYTES]>::try_from(public_key)
        .map_err(|_| CryptoError::InvalidEncoding("Ed25519 public key"))?;
    let key = VerifyingKey::from_bytes(bytes)
        .map_err(|_| CryptoError::InvalidEncoding("Ed25519 public key"))?;
    if key.is_weak() {
        return Err(CryptoError::InvalidKey("Ed25519 public key"));
    }
    let mut digest = Sha256::new();
    digest.update(b"DISP Ed25519 key identifier v1\0");
    digest.update(public_key);
    Ok(digest.finalize().into())
}

pub fn ed25519_verify_keyed(
    expected_key_id: &[u8],
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<bool, CryptoError> {
    let expected = <&[u8; 32]>::try_from(expected_key_id)
        .map_err(|_| CryptoError::InvalidEncoding("Ed25519 key identifier"))?;
    let actual = ed25519_key_id(public_key)?;
    if !bool::from(actual.ct_eq(expected)) {
        return Ok(false);
    }
    Ok(ed25519_verify_strict(public_key, message, signature))
}

#[allow(clippy::too_many_arguments)]
pub fn ed25519_verify_lifecycle(
    expected_key_id: &[u8],
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
    valid_from: u64,
    valid_until: u64,
    revoked: bool,
    evaluation_time: u64,
) -> Result<bool, CryptoError> {
    if valid_from > valid_until {
        return Err(CryptoError::InvalidEncoding("Ed25519 key lifecycle window"));
    }
    if revoked || evaluation_time < valid_from || evaluation_time > valid_until {
        return Ok(false);
    }
    ed25519_verify_keyed(expected_key_id, public_key, message, signature)
}

pub fn encode_ed25519_public_key(public_key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    encode_ed25519_record(
        public_key,
        ED25519_PUBLIC_KEY_BYTES,
        ED25519_PUBLIC_KEY_RECORD_KIND,
        "DISP Ed25519 public key",
    )
}

pub fn decode_ed25519_public_key(encoded: &[u8]) -> Result<Vec<u8>, CryptoError> {
    decode_ed25519_record(
        encoded,
        ED25519_PUBLIC_KEY_BYTES,
        ED25519_PUBLIC_KEY_RECORD_KIND,
        "DISP Ed25519 public key",
    )
}

pub fn encode_ed25519_signature(signature: &[u8]) -> Result<Vec<u8>, CryptoError> {
    encode_ed25519_record(
        signature,
        ED25519_SIGNATURE_BYTES,
        ED25519_SIGNATURE_RECORD_KIND,
        "DISP Ed25519 signature",
    )
}

pub fn decode_ed25519_signature(encoded: &[u8]) -> Result<Vec<u8>, CryptoError> {
    decode_ed25519_record(
        encoded,
        ED25519_SIGNATURE_BYTES,
        ED25519_SIGNATURE_RECORD_KIND,
        "DISP Ed25519 signature",
    )
}

fn encode_ed25519_record(
    payload: &[u8],
    expected_length: usize,
    kind: u8,
    operation: &'static str,
) -> Result<Vec<u8>, CryptoError> {
    if payload.len() != expected_length {
        return Err(CryptoError::InvalidEncoding(operation));
    }
    let mut encoded = Vec::with_capacity(ED25519_RECORD_HEADER_BYTES + payload.len());
    encoded.extend_from_slice(b"DISP");
    encoded.extend_from_slice(&[
        ED25519_RECORD_FORMAT_VERSION,
        kind,
        ED25519_RECORD_ALGORITHM,
        expected_length as u8,
    ]);
    encoded.extend_from_slice(payload);
    Ok(encoded)
}

fn decode_ed25519_record(
    encoded: &[u8],
    expected_length: usize,
    kind: u8,
    operation: &'static str,
) -> Result<Vec<u8>, CryptoError> {
    if encoded.len() != ED25519_RECORD_HEADER_BYTES + expected_length
        || encoded[..4] != *b"DISP"
        || encoded[4] != ED25519_RECORD_FORMAT_VERSION
        || encoded[5] != kind
        || encoded[6] != ED25519_RECORD_ALGORITHM
        || usize::from(encoded[7]) != expected_length
    {
        return Err(CryptoError::InvalidEncoding(operation));
    }
    Ok(encoded[ED25519_RECORD_HEADER_BYTES..].to_vec())
}

pub fn argon2id_hash_password(password: &SecretBytes) -> Result<String, CryptoError> {
    validate_length(
        "Argon2id password",
        password.len(),
        MAX_PASSWORD_BYTES,
        false,
    )?;
    let mut salt_bytes = Zeroizing::new([0u8; 16]);
    getrandom::fill(salt_bytes.as_mut_slice())
        .map_err(|error| CryptoError::EntropyUnavailable(error.to_string()))?;
    let salt = SaltString::encode_b64(salt_bytes.as_slice()).map_err(|error| {
        CryptoError::OperationFailed {
            operation: "Argon2id salt encoding",
            cause: error.to_string(),
        }
    })?;
    let hash = argon2id_policy()?
        .hash_password(password.expose_secret(), &salt)
        .map_err(|error| CryptoError::OperationFailed {
            operation: "Argon2id password hashing",
            cause: error.to_string(),
        })?
        .to_string();
    validate_length(
        "Argon2id password hash",
        hash.len(),
        MAX_PASSWORD_HASH_BYTES,
        false,
    )?;
    Ok(hash)
}

pub fn argon2id_verify_password(
    password: &SecretBytes,
    encoded_hash: &str,
) -> Result<bool, CryptoError> {
    validate_length(
        "Argon2id password",
        password.len(),
        MAX_PASSWORD_BYTES,
        false,
    )?;
    validate_length(
        "Argon2id password hash",
        encoded_hash.len(),
        MAX_PASSWORD_HASH_BYTES,
        false,
    )?;
    let parsed = PasswordHash::new(encoded_hash)
        .map_err(|_| CryptoError::InvalidEncoding("Argon2id password hash"))?;
    if !argon2id_hash_uses_policy(&parsed) {
        return Err(CryptoError::InvalidEncoding(
            "Argon2id password hash policy",
        ));
    }
    match argon2id_policy()?.verify_password(password.expose_secret(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(error) => Err(CryptoError::OperationFailed {
            operation: "Argon2id password verification",
            cause: error.to_string(),
        }),
    }
}

fn validate_aead_inputs(
    key: &SecretBytes,
    plaintext_length: usize,
    associated_data_length: usize,
) -> Result<(), CryptoError> {
    if key.len() != 32 {
        return Err(CryptoError::InvalidKey("AES-256-GCM-SIV"));
    }
    validate_length(
        "AES-256-GCM-SIV plaintext",
        plaintext_length,
        MAX_AEAD_PLAINTEXT_BYTES,
        true,
    )?;
    validate_length(
        "AES-256-GCM-SIV associated data",
        associated_data_length,
        MAX_AEAD_ASSOCIATED_DATA_BYTES,
        true,
    )
}

fn seal_with_nonce(
    key: &SecretBytes,
    plaintext: &[u8],
    associated_data: &[u8],
    nonce: [u8; AES256_GCM_SIV_NONCE_BYTES],
) -> Result<AeadEnvelope, CryptoError> {
    validate_aead_inputs(key, plaintext.len(), associated_data.len())?;
    let cipher = Aes256GcmSiv::new_from_slice(key.expose_secret())
        .map_err(|_| CryptoError::InvalidKey("AES-256-GCM-SIV"))?;
    let nonce_value = Nonce::try_from(nonce.as_slice())
        .map_err(|_| CryptoError::InvalidEncoding("AES-256-GCM-SIV nonce"))?;
    let ciphertext = cipher
        .encrypt(
            &nonce_value,
            Payload {
                msg: plaintext,
                aad: associated_data,
            },
        )
        .map_err(|_| CryptoError::OperationFailed {
            operation: "AES-256-GCM-SIV encryption",
            cause: "cipher rejected bounded input".to_owned(),
        })?;
    AeadEnvelope::from_parts(nonce, ciphertext)
}

fn argon2id_policy() -> Result<Argon2<'static>, CryptoError> {
    let params = Params::new(
        ARGON2ID_MEMORY_KIB,
        ARGON2ID_ITERATIONS,
        ARGON2ID_PARALLELISM,
        Some(ARGON2ID_OUTPUT_BYTES),
    )
    .map_err(|error| CryptoError::OperationFailed {
        operation: "Argon2id policy",
        cause: error.to_string(),
    })?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
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

fn validate_length(
    operation: &'static str,
    requested: usize,
    maximum: usize,
    allow_empty: bool,
) -> Result<(), CryptoError> {
    if requested > maximum || (!allow_empty && requested == 0) {
        return Err(CryptoError::InvalidLength {
            operation,
            requested,
            maximum,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn aes256_gcm_siv_matches_rfc8452_empty_plaintext_vector() {
        let key = SecretBytes::from_vec(decode_hex(
            "0100000000000000000000000000000000000000000000000000000000000000",
        ))
        .unwrap();
        let nonce: [u8; AES256_GCM_SIV_NONCE_BYTES] =
            decode_hex("030000000000000000000000").try_into().unwrap();
        let envelope = seal_with_nonce(&key, &[], &[], nonce).unwrap();
        assert_eq!(
            envelope.ciphertext(),
            decode_hex("07f5f4169bbf55a8400cd47ea6fd400f")
        );
        assert!(
            aes256_gcm_siv_open(&key, &envelope, &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ed25519_matches_rfc8032_empty_message_vector() {
        let seed: [u8; 32] =
            decode_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
                .try_into()
                .unwrap();
        let key = Ed25519SigningKey {
            key: SigningKey::from_bytes(&seed),
        };
        assert_eq!(
            key.public_key().to_vec(),
            decode_hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        );
        let signature = key.sign(&[]);
        assert_eq!(
            signature.to_vec(),
            decode_hex(
                "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
                 5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
            )
        );
        assert!(ed25519_verify_strict(&key.public_key(), &[], &signature));
    }
}
