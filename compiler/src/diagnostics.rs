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
    pub path: PathBuf,
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

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Lex => "lexer",
            Self::Parse => "parser",
            Self::Resolve => "resolver",
            Self::Type => "type",
            Self::Runtime => "runtime",
            Self::Internal => "internal compiler",
            Self::Backend => "native backend",
        };
        formatter.write_str(label)
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
