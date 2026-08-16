//! Out-of-process hardware/external signing-key provider boundary.
//!
//! DISP never receives a provider's private key. It sends an opaque handle and
//! bounded message through the resource-contained component transport, pins
//! the provider executable by content, pins the public key by its stable DISP
//! key identifier, and verifies every returned signature before releasing it.

use crate::{
    component_host::{self, ComponentCommand, ComponentError},
    crypto, limits,
};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    num::NonZeroU8,
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"DISPKS1\0";
const HEADER_BYTES: usize = 16;
const PUBLIC_KEY_OPERATION: u8 = 1;
const SIGN_OPERATION: u8 = 2;
const MAX_HANDLE_BYTES: usize = 1024;
const MAX_PROVIDER_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_KEYSTORE_MESSAGE_BYTES: usize =
    limits::COMPONENT_MESSAGE_BYTES - HEADER_BYTES - 2 - MAX_HANDLE_BYTES;

/// Provider-side implementation of an external Ed25519 key.
///
/// Only an opaque handle and the message reach these methods. Implementations
/// retain private material inside their own process or hardware device.
pub trait Ed25519KeyProvider {
    fn public_key(&mut self, handle: &[u8]) -> Result<[u8; 32], NonZeroU8>;

    fn sign(&mut self, handle: &[u8], message: &[u8]) -> Result<[u8; 64], NonZeroU8>;
}

#[derive(Debug)]
pub enum KeystoreError {
    InvalidConfiguration(String),
    ProviderChanged,
    ProviderRejected(u8),
    Protocol(String),
    Component(ComponentError),
    Io(io::Error),
    Cryptography(crypto::CryptoError),
}

impl fmt::Display for KeystoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => {
                write!(formatter, "invalid key-provider configuration: {message}")
            }
            Self::ProviderChanged => formatter
                .write_str("key-provider executable content changed after the handle was opened"),
            Self::ProviderRejected(status) => {
                write!(
                    formatter,
                    "key provider rejected the request with status {status}"
                )
            }
            Self::Protocol(message) => {
                write!(formatter, "invalid key-provider response: {message}")
            }
            Self::Component(error) => write!(formatter, "key-provider component failed: {error}"),
            Self::Io(error) => write!(formatter, "key-provider I/O failed: {error}"),
            Self::Cryptography(error) => write!(formatter, "key-provider result failed: {error}"),
        }
    }
}

impl Error for KeystoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Component(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Cryptography(error) => Some(error),
            _ => None,
        }
    }
}

/// A pinned, opaque reference to an Ed25519 key held by an external provider.
///
/// The handle may itself be sensitive, so it is non-cloning, debug-redacted,
/// non-serializable, and zeroized on drop.
pub struct HardwareEd25519Key {
    provider: PathBuf,
    provider_digest: [u8; 32],
    handle: Zeroizing<Vec<u8>>,
    expected_key_id: [u8; 32],
}

impl fmt::Debug for HardwareEd25519Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HardwareEd25519Key")
            .field("provider", &self.provider)
            .field("handle", &"<redacted>")
            .field("expected_key_id", &self.expected_key_id)
            .finish()
    }
}

impl HardwareEd25519Key {
    pub fn open(
        provider: impl AsRef<Path>,
        handle: Vec<u8>,
        expected_key_id: &[u8],
    ) -> Result<Self, KeystoreError> {
        if !(1..=MAX_HANDLE_BYTES).contains(&handle.len()) {
            return Err(KeystoreError::InvalidConfiguration(format!(
                "opaque handle length must be between 1 and {MAX_HANDLE_BYTES} bytes"
            )));
        }
        let expected_key_id = <[u8; 32]>::try_from(expected_key_id).map_err(|_| {
            KeystoreError::InvalidConfiguration(
                "expected Ed25519 key identifier must contain exactly 32 bytes".into(),
            )
        })?;
        let provider = fs::canonicalize(provider.as_ref()).map_err(KeystoreError::Io)?;
        if !provider.is_file() {
            return Err(KeystoreError::InvalidConfiguration(format!(
                "provider `{}` is not a regular file",
                provider.display()
            )));
        }
        let provider_digest = provider_digest(&provider)?;
        Ok(Self {
            provider,
            provider_digest,
            handle: Zeroizing::new(handle),
            expected_key_id,
        })
    }

    pub fn expected_key_id(&self) -> &[u8; 32] {
        &self.expected_key_id
    }

    pub fn public_key(&self) -> Result<[u8; 32], KeystoreError> {
        let response = self.invoke(PUBLIC_KEY_OPERATION, &[])?;
        validate_public_key(&self.expected_key_id, &response)
    }

    pub fn sign(&self, message: &[u8]) -> Result<[u8; 64], KeystoreError> {
        if message.len() > MAX_KEYSTORE_MESSAGE_BYTES {
            return Err(KeystoreError::InvalidConfiguration(format!(
                "signing message is {} bytes; provider limit is {MAX_KEYSTORE_MESSAGE_BYTES}",
                message.len()
            )));
        }
        let public_key = self.public_key()?;
        let response = self.invoke(SIGN_OPERATION, message)?;
        validate_signature(&self.expected_key_id, &public_key, message, &response)
    }

    fn invoke(&self, operation: u8, message: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        if provider_digest(&self.provider)? != self.provider_digest {
            return Err(KeystoreError::ProviderChanged);
        }
        let request = encode_request(operation, &self.handle, message)?;
        let response = ComponentCommand::new(&self.provider)
            .arg("--disp-keystore-provider-v1")
            .invoke(&request)
            .map_err(KeystoreError::Component)?;
        decode_response(operation, &response)
    }
}

/// Serves exactly one component-framed keystore request on the supplied streams.
///
/// Provider executables normally pass locked standard input/output here after
/// checking for the `--disp-keystore-provider-v1` process argument. The caller
/// must not write diagnostics to `output`; standard error remains available.
pub fn serve_ed25519_provider_once<P: Ed25519KeyProvider>(
    provider: &mut P,
    input: &mut impl Read,
    output: &mut impl Write,
) -> Result<(), KeystoreError> {
    if std::env::var_os("DISP_COMPONENT_PROTOCOL").as_deref()
        != Some(std::ffi::OsStr::new("disp.component.v1"))
    {
        return Err(KeystoreError::InvalidConfiguration(
            "provider requires DISP_COMPONENT_PROTOCOL=disp.component.v1".into(),
        ));
    }

    let maximum_frame_bytes = limits::COMPONENT_MESSAGE_BYTES + 16;
    let mut framed_request = Vec::new();
    input
        .take((maximum_frame_bytes + 1) as u64)
        .read_to_end(&mut framed_request)
        .map_err(KeystoreError::Io)?;
    if framed_request.len() > maximum_frame_bytes {
        return Err(KeystoreError::Protocol(
            "component request exceeds its framing limit".into(),
        ));
    }
    let request =
        component_host::decode_frame(&framed_request).map_err(KeystoreError::Component)?;
    let response = dispatch_provider_request(provider, &request)?;
    output
        .write_all(&component_host::encode_frame(&response))
        .map_err(KeystoreError::Io)?;
    output.flush().map_err(KeystoreError::Io)
}

fn dispatch_provider_request<P: Ed25519KeyProvider>(
    provider: &mut P,
    request: &[u8],
) -> Result<Vec<u8>, KeystoreError> {
    let (operation, handle, message) = decode_request(request)?;
    match operation {
        PUBLIC_KEY_OPERATION => match provider.public_key(handle) {
            Ok(public_key) => Ok(encode_response(operation, 0, &public_key)),
            Err(status) => Ok(encode_response(operation, status.get(), &[])),
        },
        SIGN_OPERATION => match provider.sign(handle, message) {
            Ok(signature) => Ok(encode_response(operation, 0, &signature)),
            Err(status) => Ok(encode_response(operation, status.get(), &[])),
        },
        _ => Err(KeystoreError::Protocol(
            "provider request has an unknown operation".into(),
        )),
    }
}

fn validate_public_key(
    expected_key_id: &[u8; 32],
    response: &[u8],
) -> Result<[u8; 32], KeystoreError> {
    let public_key = <[u8; 32]>::try_from(response)
        .map_err(|_| KeystoreError::Protocol("public-key response is not 32 bytes".into()))?;
    let actual = crypto::ed25519_key_id(&public_key).map_err(KeystoreError::Cryptography)?;
    if !bool::from(actual.ct_eq(expected_key_id)) {
        return Err(KeystoreError::Protocol(
            "provider public key does not match the pinned key identifier".into(),
        ));
    }
    Ok(public_key)
}

fn validate_signature(
    expected_key_id: &[u8; 32],
    public_key: &[u8; 32],
    message: &[u8],
    response: &[u8],
) -> Result<[u8; 64], KeystoreError> {
    let signature = <[u8; 64]>::try_from(response)
        .map_err(|_| KeystoreError::Protocol("signature response is not 64 bytes".into()))?;
    let valid = crypto::ed25519_verify_keyed(expected_key_id, public_key, message, &signature)
        .map_err(KeystoreError::Cryptography)?;
    if !valid {
        return Err(KeystoreError::Protocol(
            "provider returned a signature that failed pinned verification".into(),
        ));
    }
    Ok(signature)
}

fn provider_digest(path: &Path) -> Result<[u8; 32], KeystoreError> {
    let metadata = fs::metadata(path).map_err(KeystoreError::Io)?;
    if metadata.len() == 0 || metadata.len() > MAX_PROVIDER_BYTES {
        return Err(KeystoreError::InvalidConfiguration(format!(
            "provider size must be between 1 and {MAX_PROVIDER_BYTES} bytes"
        )));
    }
    let mut file = File::open(path).map_err(KeystoreError::Io)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(KeystoreError::Io)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| KeystoreError::InvalidConfiguration("provider size overflow".into()))?;
        if total > MAX_PROVIDER_BYTES {
            return Err(KeystoreError::InvalidConfiguration(
                "provider grew beyond its size limit while hashing".into(),
            ));
        }
        digest.update(&buffer[..count]);
    }
    if total != metadata.len() {
        return Err(KeystoreError::InvalidConfiguration(
            "provider changed while its identity was being measured".into(),
        ));
    }
    Ok(digest.finalize().into())
}

fn encode_request(operation: u8, handle: &[u8], message: &[u8]) -> Result<Vec<u8>, KeystoreError> {
    if !matches!(operation, PUBLIC_KEY_OPERATION | SIGN_OPERATION)
        || !(1..=MAX_HANDLE_BYTES).contains(&handle.len())
        || message.len() > MAX_KEYSTORE_MESSAGE_BYTES
        || (operation == PUBLIC_KEY_OPERATION && !message.is_empty())
    {
        return Err(KeystoreError::InvalidConfiguration(
            "invalid provider operation, handle, or message bounds".into(),
        ));
    }
    let payload_length = 2usize
        .checked_add(handle.len())
        .and_then(|length| length.checked_add(message.len()))
        .ok_or_else(|| KeystoreError::InvalidConfiguration("request length overflow".into()))?;
    let mut request = Vec::with_capacity(HEADER_BYTES + payload_length);
    request.extend_from_slice(MAGIC);
    request.extend_from_slice(&[operation, 0, 0, 0]);
    request.extend_from_slice(&(payload_length as u32).to_be_bytes());
    request.extend_from_slice(&(handle.len() as u16).to_be_bytes());
    request.extend_from_slice(handle);
    request.extend_from_slice(message);
    Ok(request)
}

fn decode_request(request: &[u8]) -> Result<(u8, &[u8], &[u8]), KeystoreError> {
    if request.len() < HEADER_BYTES + 2 {
        return Err(KeystoreError::Protocol("request is truncated".into()));
    }
    let operation = request[8];
    if &request[..8] != MAGIC
        || !matches!(operation, PUBLIC_KEY_OPERATION | SIGN_OPERATION)
        || request[9..12] != [0, 0, 0]
    {
        return Err(KeystoreError::Protocol(
            "request magic, operation, or reserved fields are invalid".into(),
        ));
    }
    let payload_length = u32::from_be_bytes(
        request[12..16]
            .try_into()
            .expect("provider payload length has four bytes"),
    ) as usize;
    let expected = HEADER_BYTES
        .checked_add(payload_length)
        .ok_or_else(|| KeystoreError::Protocol("request length overflow".into()))?;
    if request.len() != expected {
        return Err(KeystoreError::Protocol(
            "request length is truncated or has trailing bytes".into(),
        ));
    }
    let handle_length = u16::from_be_bytes(
        request[16..18]
            .try_into()
            .expect("provider handle length has two bytes"),
    ) as usize;
    if !(1..=MAX_HANDLE_BYTES).contains(&handle_length) || 2 + handle_length > payload_length {
        return Err(KeystoreError::Protocol(
            "request opaque-handle length is invalid".into(),
        ));
    }
    let handle_end = HEADER_BYTES + 2 + handle_length;
    let handle = &request[HEADER_BYTES + 2..handle_end];
    let message = &request[handle_end..];
    if message.len() > MAX_KEYSTORE_MESSAGE_BYTES
        || (operation == PUBLIC_KEY_OPERATION && !message.is_empty())
    {
        return Err(KeystoreError::Protocol(
            "request message is invalid for its operation".into(),
        ));
    }
    Ok((operation, handle, message))
}

fn encode_response(operation: u8, status: u8, payload: &[u8]) -> Vec<u8> {
    debug_assert!(matches!(operation, PUBLIC_KEY_OPERATION | SIGN_OPERATION));
    debug_assert!(
        (status == 0
            && payload.len()
                == if operation == PUBLIC_KEY_OPERATION {
                    32
                } else {
                    64
                })
            || (status != 0 && payload.is_empty())
    );
    let mut response = Vec::with_capacity(HEADER_BYTES + payload.len());
    response.extend_from_slice(MAGIC);
    response.extend_from_slice(&[operation, status, 0, 0]);
    response.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    response.extend_from_slice(payload);
    response
}

fn decode_response(operation: u8, response: &[u8]) -> Result<Vec<u8>, KeystoreError> {
    if response.len() < HEADER_BYTES {
        return Err(KeystoreError::Protocol(
            "response header is truncated".into(),
        ));
    }
    if &response[..8] != MAGIC || response[8] != operation || response[10] != 0 || response[11] != 0
    {
        return Err(KeystoreError::Protocol(
            "response magic, operation, or reserved fields are invalid".into(),
        ));
    }
    let status = response[9];
    let payload_length = u32::from_be_bytes(
        response[12..16]
            .try_into()
            .expect("provider payload length has four bytes"),
    ) as usize;
    let expected = HEADER_BYTES
        .checked_add(payload_length)
        .ok_or_else(|| KeystoreError::Protocol("response length overflow".into()))?;
    if response.len() != expected {
        return Err(KeystoreError::Protocol(
            "response length is truncated or has trailing bytes".into(),
        ));
    }
    if status != 0 {
        if payload_length != 0 {
            return Err(KeystoreError::Protocol(
                "rejected response must not contain a payload".into(),
            ));
        }
        return Err(KeystoreError::ProviderRejected(status));
    }
    let required = if operation == PUBLIC_KEY_OPERATION {
        32
    } else {
        64
    };
    if payload_length != required {
        return Err(KeystoreError::Protocol(format!(
            "successful operation {operation} returned {payload_length} bytes; expected {required}"
        )));
    }
    Ok(response[HEADER_BYTES..].to_vec())
}

/// Fuzzing-only entry point for the internal keystore request/response codecs.
#[cfg(feature = "fuzzing")]
pub fn fuzz_decode_frames(frame: &[u8]) {
    let _ = decode_request(frame);
    let _ = decode_response(PUBLIC_KEY_OPERATION, frame);
    let _ = decode_response(SIGN_OPERATION, frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Ed25519SigningKey;

    struct TestProvider {
        key: Ed25519SigningKey,
    }

    impl Ed25519KeyProvider for TestProvider {
        fn public_key(&mut self, handle: &[u8]) -> Result<[u8; 32], NonZeroU8> {
            if handle == b"device-slot-7" {
                Ok(self.key.public_key())
            } else {
                Err(NonZeroU8::new(4).unwrap())
            }
        }

        fn sign(&mut self, handle: &[u8], message: &[u8]) -> Result<[u8; 64], NonZeroU8> {
            if handle == b"device-slot-7" {
                Ok(self.key.sign(message))
            } else {
                Err(NonZeroU8::new(4).unwrap())
            }
        }
    }

    fn response(operation: u8, status: u8, payload: &[u8]) -> Vec<u8> {
        let mut response = Vec::from(MAGIC);
        response.extend_from_slice(&[operation, status, 0, 0]);
        response.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        response.extend_from_slice(payload);
        response
    }

    #[test]
    fn requests_are_exact_bounded_and_contain_only_handle_and_message() {
        let request = encode_request(SIGN_OPERATION, b"opaque-handle", b"message").unwrap();
        assert_eq!(&request[..8], MAGIC);
        assert_eq!(request[8], SIGN_OPERATION);
        assert_eq!(&request[16..18], &(13u16).to_be_bytes());
        assert_eq!(&request[18..31], b"opaque-handle");
        assert_eq!(&request[31..], b"message");
        assert!(!request.windows(6).any(|window| window == b"secret"));
        assert!(encode_request(PUBLIC_KEY_OPERATION, b"h", b"unexpected").is_err());
        assert!(encode_request(SIGN_OPERATION, b"", b"message").is_err());
    }

    #[test]
    fn responses_are_operation_bound_exact_and_fail_closed() {
        assert_eq!(
            decode_response(
                PUBLIC_KEY_OPERATION,
                &response(PUBLIC_KEY_OPERATION, 0, &[7; 32])
            )
            .unwrap(),
            vec![7; 32]
        );
        assert!(
            decode_response(SIGN_OPERATION, &response(PUBLIC_KEY_OPERATION, 0, &[0; 32])).is_err()
        );
        assert!(decode_response(SIGN_OPERATION, &response(SIGN_OPERATION, 0, &[0; 63])).is_err());
        assert!(matches!(
            decode_response(SIGN_OPERATION, &response(SIGN_OPERATION, 9, &[])),
            Err(KeystoreError::ProviderRejected(9))
        ));
        let mut trailing = response(SIGN_OPERATION, 0, &[0; 64]);
        trailing.push(0);
        assert!(decode_response(SIGN_OPERATION, &trailing).is_err());
    }

    #[test]
    fn handles_are_nonclone_redacted_and_zeroizing_by_construction() {
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/crypto_keystore.rs"),
        )
        .unwrap();
        let before_declaration = source
            .split_once("pub struct HardwareEd25519Key")
            .unwrap()
            .0;
        let declaration_attributes =
            &before_declaration[before_declaration.len().saturating_sub(160)..];
        assert!(!declaration_attributes.contains("Clone"));
        assert!(source.contains("handle: Zeroizing<Vec<u8>>"));
        assert!(source.contains(".field(\"handle\", &\"<redacted>\")"));
    }

    #[test]
    fn provider_results_are_identity_pinned_and_verified() {
        let signing_key = Ed25519SigningKey::generate().unwrap();
        let public_key = signing_key.public_key();
        let key_id = crypto::ed25519_key_id(&public_key).unwrap();
        let message = b"hardware-backed release signature";
        let signature = signing_key.sign(message);

        assert_eq!(
            validate_public_key(&key_id, &public_key).unwrap(),
            public_key
        );
        assert_eq!(
            validate_signature(&key_id, &public_key, message, &signature).unwrap(),
            signature
        );

        let other_key = Ed25519SigningKey::generate().unwrap();
        assert!(validate_public_key(&key_id, &other_key.public_key()).is_err());
        assert!(validate_signature(&key_id, &public_key, b"changed", &signature).is_err());
        assert!(validate_signature(&key_id, &public_key, message, &[0; 64]).is_err());
    }

    #[test]
    fn provider_sdk_dispatches_only_opaque_handles_and_messages() {
        let mut provider = TestProvider {
            key: Ed25519SigningKey::generate().unwrap(),
        };
        let public_request = encode_request(PUBLIC_KEY_OPERATION, b"device-slot-7", &[]).unwrap();
        let public_response = dispatch_provider_request(&mut provider, &public_request).unwrap();
        let public_key = decode_response(PUBLIC_KEY_OPERATION, &public_response).unwrap();
        assert_eq!(public_key, provider.key.public_key());

        let message = b"release artifact digest";
        let sign_request = encode_request(SIGN_OPERATION, b"device-slot-7", message).unwrap();
        let sign_response = dispatch_provider_request(&mut provider, &sign_request).unwrap();
        let signature = decode_response(SIGN_OPERATION, &sign_response).unwrap();
        assert!(crypto::ed25519_verify_strict(
            &public_key,
            message,
            &signature
        ));

        let missing = encode_request(SIGN_OPERATION, b"missing", message).unwrap();
        assert!(matches!(
            decode_response(
                SIGN_OPERATION,
                &dispatch_provider_request(&mut provider, &missing).unwrap()
            ),
            Err(KeystoreError::ProviderRejected(4))
        ));

        let mut malformed = sign_request;
        malformed[10] = 1;
        assert!(dispatch_provider_request(&mut provider, &malformed).is_err());
    }
}
