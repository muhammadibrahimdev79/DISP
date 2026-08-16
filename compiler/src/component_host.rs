//! Resource-contained, out-of-process foreign component transport.
//!
//! Components receive and return exactly one `disp.component.v1` frame. They
//! never share the compiler or runtime address space. This boundary currently
//! provides process-tree and resource containment; filesystem and network
//! isolation require a stronger platform profile.

use crate::{
    limits,
    process_sandbox::{SandboxProfile, SandboxedCommand},
};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, io,
    path::Path,
    process::ExitStatus,
};

const MAGIC: &[u8; 8] = b"DISPCMP1";
const HEADER_BYTES: usize = MAGIC.len() + size_of::<u64>();

#[derive(Debug)]
pub enum ComponentError {
    RequestTooLarge { bytes: usize, limit: usize },
    Launch(io::Error),
    Failed { status: ExitStatus, stderr: String },
    Protocol(String),
}

impl fmt::Display for ComponentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestTooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "component request is {bytes} bytes; the limit is {limit}"
                )
            }
            Self::Launch(error) => write!(formatter, "component launch failed: {error}"),
            Self::Failed { status, stderr } if stderr.is_empty() => {
                write!(formatter, "component exited with status {status}")
            }
            Self::Failed { status, stderr } => {
                write!(formatter, "component exited with status {status}: {stderr}")
            }
            Self::Protocol(message) => write!(formatter, "invalid component response: {message}"),
        }
    }
}

impl Error for ComponentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Launch(error) => Some(error),
            _ => None,
        }
    }
}

pub struct ComponentCommand {
    program: OsString,
    arguments: Vec<OsString>,
}

impl ComponentCommand {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            arguments: Vec::new(),
        }
    }

    pub fn arg(&mut self, argument: impl AsRef<OsStr>) -> &mut Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.arguments
            .extend(arguments.into_iter().map(|value| value.as_ref().to_owned()));
        self
    }

    pub fn invoke(&self, request: &[u8]) -> Result<Vec<u8>, ComponentError> {
        if request.len() > limits::COMPONENT_MESSAGE_BYTES {
            return Err(ComponentError::RequestTooLarge {
                bytes: request.len(),
                limit: limits::COMPONENT_MESSAGE_BYTES,
            });
        }
        let input = encode_frame(request);
        let mut command = SandboxedCommand::new(&self.program);
        command
            .args(&self.arguments)
            .env_clear()
            .env("DISP_COMPONENT_PROTOCOL", "disp.component.v1");
        let output = command
            .output_with_input(SandboxProfile::Component, &input)
            .map_err(ComponentError::Launch)?;
        if !output.status.success() {
            return Err(ComponentError::Failed {
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        decode_frame(&output.stdout)
    }
}

pub(crate) fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER_BYTES + payload.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub(crate) fn decode_frame(frame: &[u8]) -> Result<Vec<u8>, ComponentError> {
    if frame.len() < HEADER_BYTES {
        return Err(ComponentError::Protocol(
            "response header is truncated".into(),
        ));
    }
    if &frame[..MAGIC.len()] != MAGIC {
        return Err(ComponentError::Protocol(
            "response magic is not DISPCMP1".into(),
        ));
    }
    let declared = u64::from_be_bytes(
        frame[MAGIC.len()..HEADER_BYTES]
            .try_into()
            .expect("component frame length has eight bytes"),
    );
    let length = usize::try_from(declared)
        .map_err(|_| ComponentError::Protocol("response length exceeds this platform".into()))?;
    if length > limits::COMPONENT_MESSAGE_BYTES {
        return Err(ComponentError::Protocol(format!(
            "response declares {length} bytes; the limit is {}",
            limits::COMPONENT_MESSAGE_BYTES
        )));
    }
    let expected = HEADER_BYTES
        .checked_add(length)
        .ok_or_else(|| ComponentError::Protocol("response length overflows".into()))?;
    if frame.len() < expected {
        return Err(ComponentError::Protocol(format!(
            "response body is truncated: expected {length} bytes, received {}",
            frame.len() - HEADER_BYTES
        )));
    }
    if frame.len() != expected {
        return Err(ComponentError::Protocol(format!(
            "response has {} trailing bytes",
            frame.len() - expected
        )));
    }
    Ok(frame[HEADER_BYTES..].to_vec())
}

/// Fuzzing-only entry point for the otherwise internal component decoder.
#[cfg(feature = "fuzzing")]
pub fn fuzz_decode_frame(frame: &[u8]) {
    let _ = decode_frame(frame);
}

pub fn invoke(program: impl AsRef<Path>, request: &[u8]) -> Result<Vec<u8>, ComponentError> {
    ComponentCommand::new(program.as_ref().as_os_str()).invoke(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_exact_binary_messages() {
        let payload = b"DISP\0component\xff";
        assert_eq!(decode_frame(&encode_frame(payload)).unwrap(), payload);
    }

    #[test]
    fn malformed_frames_fail_closed() {
        let mut bad_magic = encode_frame(b"safe");
        bad_magic[0] = b'X';
        assert!(
            decode_frame(&bad_magic)
                .unwrap_err()
                .to_string()
                .contains("magic")
        );

        let mut truncated = encode_frame(b"safe");
        truncated.pop();
        assert!(
            decode_frame(&truncated)
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );

        let mut trailing = encode_frame(b"safe");
        trailing.push(0);
        assert!(
            decode_frame(&trailing)
                .unwrap_err()
                .to_string()
                .contains("trailing")
        );

        let mut oversized = Vec::from(MAGIC);
        oversized.extend_from_slice(&((limits::COMPONENT_MESSAGE_BYTES as u64) + 1).to_be_bytes());
        assert!(
            decode_frame(&oversized)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }
}
