use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    Windows,
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
        if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
            Ok(Self {
                architecture: Architecture::X86_64,
                operating_system: OperatingSystem::Windows,
                endian: Endian::Little,
                pointer_width: 64,
                pointer_alignment: 8,
            })
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Backend,
                "native backend currently supports only Windows x86-64",
                Span::point(1, 1),
            ))
        }
    }
}
