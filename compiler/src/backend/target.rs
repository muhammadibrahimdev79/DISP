use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    Windows,
    Linux,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub architecture: Architecture,
    pub operating_system: OperatingSystem,
    pub endian: Endian,
    pub pointer_width: u16,
    pub pointer_alignment: u64,
}

impl Target {
    pub fn host() -> Result<Self, Diagnostic> {
        let operating_system = if cfg!(target_os = "windows") {
            Some(OperatingSystem::Windows)
        } else if cfg!(target_os = "linux") {
            Some(OperatingSystem::Linux)
        } else {
            None
        };
        if cfg!(target_arch = "x86_64")
            && let Some(operating_system) = operating_system
        {
            Ok(Self {
                architecture: Architecture::X86_64,
                operating_system,
                endian: Endian::Little,
                pointer_width: 64,
                pointer_alignment: 8,
            })
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Backend,
                "native backend currently supports only Windows and Linux x86-64",
                Span::point(1, 1),
            ))
        }
    }
}
