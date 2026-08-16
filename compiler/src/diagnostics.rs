use std::{fmt, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    pub const fn point(line: usize, column: usize) -> Self {
        let position = Position { line, column };
        Self::new(position, position)
    }

    pub fn through(self, other: Self) -> Self {
        Self::new(self.start, other.end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// User-facing path used in diagnostics.
    pub path: PathBuf,
    /// Canonical file identity used by compiler services such as build caching.
    pub identity_path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMap {
    pub files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn remap(&self, mut diagnostic: Diagnostic) -> Diagnostic {
        if diagnostic.file.is_some() {
            return diagnostic;
        }
        let Some(source) = self.files.iter().find(|source| {
            diagnostic.span.start.line >= source.start_line
                && diagnostic.span.start.line <= source.end_line
        }) else {
            return diagnostic;
        };
        let offset = source.start_line - 1;
        diagnostic.span = Span::new(
            Position {
                line: diagnostic.span.start.line.saturating_sub(offset),
                column: diagnostic.span.start.column,
            },
            Position {
                line: diagnostic.span.end.line.saturating_sub(offset),
                column: diagnostic.span.end.column,
            },
        );
        diagnostic.file = Some(source.path.display().to_string());
        diagnostic
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Lex,
    Parse,
    Resolve,
    Type,
    Runtime,
    Internal,
    Backend,
}

impl DiagnosticKind {
    /// Stable category code used by machine-readable diagnostics.
    ///
    /// Candidate 1 stabilizes stage-level codes first. More specific leaf codes may be added
    /// later without changing these category identities.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Lex => "DISP-LEX-0001",
            Self::Parse => "DISP-PARSE-0001",
            Self::Resolve => "DISP-RESOLVE-0001",
            Self::Type => "DISP-TYPE-0001",
            Self::Runtime => "DISP-RUNTIME-0001",
            Self::Internal => "DISP-INTERNAL-0001",
            Self::Backend => "DISP-BACKEND-0001",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lex => "lexer",
            Self::Parse => "parser",
            Self::Resolve => "resolver",
            Self::Type => "type",
            Self::Runtime => "runtime",
            Self::Internal => "internal compiler",
            Self::Backend => "native backend",
        }
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub span: Span,
    pub help: Option<String>,
    pub file: Option<String>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, message: impl Into<String>, span: Span) -> Self {
        Self {
            kind,
            message: message.into(),
            span,
            help: None,
            file: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn render(&self, file: &str) -> String {
        let file = self.file.as_deref().unwrap_or(file);
        let mut rendered = format!(
            "{file}:{}:{}: {} error: {}",
            self.span.start.line, self.span.start.column, self.kind, self.message
        );
        if let Some(help) = &self.help {
            rendered.push_str("\nhelp: ");
            rendered.push_str(help);
        }
        rendered
    }

    /// Renders one deterministic JSON object following `disp.diagnostic.v1`.
    pub fn render_json(&self, file: &str) -> String {
        let file = self.file.as_deref().unwrap_or(file);
        let help = self.help.as_ref().map_or_else(
            || "null".to_owned(),
            |help| format!("\"{}\"", escape_json(help)),
        );
        format!(
            "{{\"schema\":\"disp.diagnostic.v1\",\"code\":\"{}\",\"severity\":\"error\",\"stage\":\"{}\",\"message\":\"{}\",\"file\":\"{}\",\"span\":{{\"start\":{{\"line\":{},\"column\":{}}},\"end\":{{\"line\":{},\"column\":{}}}}},\"help\":{help}}}",
            self.kind.code(),
            self.kind.label(),
            escape_json(&self.message),
            escape_json(file),
            self.span.start.line,
            self.span.start.column,
            self.span.end.line,
            self.span.end.column,
        )
    }
}

pub fn render_driver_json(code: &str, message: &str) -> String {
    format!(
        "{{\"schema\":\"disp.diagnostic.v1\",\"code\":\"{}\",\"severity\":\"error\",\"stage\":\"driver\",\"message\":\"{}\",\"file\":null,\"span\":null,\"help\":null}}",
        escape_json(code),
        escape_json(message)
    )
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use fmt::Write;
                write!(escaped, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => escaped.push(character),
        }
    }
    escaped
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} error at {}:{}: {}",
            self.kind, self.span.start.line, self.span.start.column, self.message
        )
    }
}

impl std::error::Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, DiagnosticKind, Position, Span, render_driver_json};

    #[test]
    fn json_diagnostics_are_deterministic_complete_and_escaped() {
        let diagnostic = Diagnostic::new(
            DiagnosticKind::Type,
            "unknown \"value\"\nnext",
            Span::new(
                Position { line: 2, column: 3 },
                Position { line: 2, column: 8 },
            ),
        )
        .with_help("use \\safe");
        assert_eq!(
            diagnostic.render_json("C:\\work\\main.disp"),
            "{\"schema\":\"disp.diagnostic.v1\",\"code\":\"DISP-TYPE-0001\",\"severity\":\"error\",\"stage\":\"type\",\"message\":\"unknown \\\"value\\\"\\nnext\",\"file\":\"C:\\\\work\\\\main.disp\",\"span\":{\"start\":{\"line\":2,\"column\":3},\"end\":{\"line\":2,\"column\":8}},\"help\":\"use \\\\safe\"}"
        );
    }

    #[test]
    fn driver_json_has_explicit_null_location() {
        assert_eq!(
            render_driver_json("DISP-DRIVER-0001", "bad argument\nnext"),
            "{\"schema\":\"disp.diagnostic.v1\",\"code\":\"DISP-DRIVER-0001\",\"severity\":\"error\",\"stage\":\"driver\",\"message\":\"bad argument\\nnext\",\"file\":null,\"span\":null,\"help\":null}"
        );
    }
}
