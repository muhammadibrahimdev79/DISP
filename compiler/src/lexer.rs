use crate::diagnostics::{Diagnostic, DiagnosticKind, Position, Span};
use unicode_ident::{is_xid_continue, is_xid_start};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Let,
    Var,
    Const,
    Fn,
    Return,
    If,
    Else,
    Match,
    For,
    In,
    While,
    Loop,
    Break,
    Continue,
    Struct,
    Enum,
    Trait,
    Impl,
    Type,
    Module,
    Use,
    As,
    Pub,
    Async,
    Await,
    Spawn,
    Parallel,
    Move,
    Mut,
    Unsafe,
    Extern,
    Export,
    Data,
    Transaction,
    Page,
    Component,
    Style,
    State,
    Route,
    Comptime,
    True,
    False,
    Identifier(String),
    Integer(u128),
    Float(f64),
    String(String),
    Character(char),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Equal,
    EqualEqual,
    Bang,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    AndAnd,
    Or,
    OrOr,
    Caret,
    Tilde,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    ShiftLeft,
    ShiftRight,
    Arrow,
    FatArrow,
    Question,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Semicolon,
    Range,
    RangeInclusive,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexer {
    source: Vec<char>,
    current: usize,
    position: Position,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Self::with_start_line(source, 1)
    }

    pub fn with_start_line(source: &str, start_line: usize) -> Self {
        Self {
            source: source.chars().collect(),
            current: 0,
            position: Position {
                line: start_line,
                column: 1,
            },
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, Diagnostic> {
        let mut tokens = Vec::new();
        while !self.is_at_end() {
            self.skip_whitespace_and_comments()?;
            if self.is_at_end() {
                break;
            }

            let start = self.position;
            let character = self.advance();
            let kind = match character {
                '(' => TokenKind::LeftParen,
                ')' => TokenKind::RightParen,
                '{' => TokenKind::LeftBrace,
                '}' => TokenKind::RightBrace,
                '[' => TokenKind::LeftBracket,
                ']' => TokenKind::RightBracket,
                ',' => TokenKind::Comma,
                ':' => TokenKind::Colon,
                ';' => TokenKind::Semicolon,
                '+' => self.compound('=', TokenKind::PlusEqual, TokenKind::Plus),
                '-' if self.match_char('>') => TokenKind::Arrow,
                '-' => self.compound('=', TokenKind::MinusEqual, TokenKind::Minus),
                '*' => self.compound('=', TokenKind::StarEqual, TokenKind::Star),
                '/' => self.compound('=', TokenKind::SlashEqual, TokenKind::Slash),
                '%' => TokenKind::Percent,
                '=' if self.match_char('>') => TokenKind::FatArrow,
                '=' => self.compound('=', TokenKind::EqualEqual, TokenKind::Equal),
                '!' => self.compound('=', TokenKind::BangEqual, TokenKind::Bang),
                '<' if self.match_char('=') => TokenKind::LessEqual,
                '<' if self.match_char('<') => TokenKind::ShiftLeft,
                '<' => TokenKind::Less,
                '>' if self.match_char('=') => TokenKind::GreaterEqual,
                '>' if self.match_char('>') => TokenKind::ShiftRight,
                '>' => TokenKind::Greater,
                '&' => self.compound('&', TokenKind::AndAnd, TokenKind::And),
                '|' => self.compound('|', TokenKind::OrOr, TokenKind::Or),
                '^' => TokenKind::Caret,
                '~' => TokenKind::Tilde,
                '?' => TokenKind::Question,
                '.' if self.match_char('.') => {
                    if self.match_char('=') {
                        TokenKind::RangeInclusive
                    } else {
                        TokenKind::Range
                    }
                }
                '.' => TokenKind::Dot,
                '"' => TokenKind::String(self.read_string(start)?),
                '\'' => TokenKind::Character(self.read_character(start)?),
                c if c.is_ascii_digit() => self.read_number(c, start)?,
                c if is_identifier_start(c) => self.read_identifier(c, start)?,
                c => {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Lex,
                        format!("unexpected character `{c}`"),
                        Span::new(start, self.position),
                    ));
                }
            };
            tokens.push(Token {
                kind,
                span: Span::new(start, self.position),
            });
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.position, self.position),
        });
        Ok(tokens)
    }

    fn compound(&mut self, next: char, combined: TokenKind, single: TokenKind) -> TokenKind {
        if self.match_char(next) {
            combined
        } else {
            single
        }
    }

    fn read_identifier(&mut self, first: char, start: Position) -> Result<TokenKind, Diagnostic> {
        let mut value = String::from(first);
        while self.peek().is_some_and(is_identifier_continue) {
            value.push(self.advance());
        }

        let normalized: String = value.nfc().collect();
        if normalized != value {
            return Err(Diagnostic::new(
                DiagnosticKind::Lex,
                format!("identifier `{value}` is not in Unicode NFC form"),
                Span::new(start, self.position),
            )
            .with_help(format!("write the identifier as `{normalized}`")));
        }

        Ok(match value.as_str() {
            "let" => TokenKind::Let,
            "var" => TokenKind::Var,
            "const" => TokenKind::Const,
            "fn" => TokenKind::Fn,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "match" => TokenKind::Match,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "while" => TokenKind::While,
            "loop" => TokenKind::Loop,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "struct" => TokenKind::Struct,
            "enum" => TokenKind::Enum,
            "trait" => TokenKind::Trait,
            "impl" => TokenKind::Impl,
            "type" => TokenKind::Type,
            "module" => TokenKind::Module,
            "use" => TokenKind::Use,
            "as" => TokenKind::As,
            "pub" => TokenKind::Pub,
            "async" => TokenKind::Async,
            "await" => TokenKind::Await,
            "spawn" => TokenKind::Spawn,
            "parallel" => TokenKind::Parallel,
            "move" => TokenKind::Move,
            "mut" => TokenKind::Mut,
            "unsafe" => TokenKind::Unsafe,
            "extern" => TokenKind::Extern,
            "export" => TokenKind::Export,
            "data" => TokenKind::Data,
            "transaction" => TokenKind::Transaction,
            "page" => TokenKind::Page,
            "component" => TokenKind::Component,
            "style" => TokenKind::Style,
            "state" => TokenKind::State,
            "route" => TokenKind::Route,
            "comptime" => TokenKind::Comptime,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Identifier(value),
        })
    }

    fn read_number(&mut self, first: char, start: Position) -> Result<TokenKind, Diagnostic> {
        let mut value = String::from(first);
        self.read_digit_run(&mut value, start)?;
        let mut is_float = false;

        if self.peek() == Some('.') && self.peek_next().is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            value.push(self.advance());
            let first_fraction = self.advance();
            value.push(first_fraction);
            self.read_digit_run(&mut value, start)?;
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            value.push(self.advance());
            if matches!(self.peek(), Some('+' | '-')) {
                value.push(self.advance());
            }
            if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
                return Err(self.invalid_number(&value, start, "exponent requires digits"));
            }
            value.push(self.advance());
            self.read_digit_run(&mut value, start)?;
        }

        let clean = value.replace('_', "");
        if is_float {
            clean.parse::<f64>().map(TokenKind::Float).map_err(|_| {
                self.invalid_number(&value, start, "floating-point value is out of range")
            })
        } else {
            clean.parse::<u128>().map(TokenKind::Integer).map_err(|_| {
                self.invalid_number(&value, start, "integer value is outside the `int` range")
            })
        }
    }

    fn read_digit_run(&mut self, value: &mut String, start: Position) -> Result<(), Diagnostic> {
        let mut previous_was_separator = false;
        while let Some(character) = self.peek() {
            if character.is_ascii_digit() {
                previous_was_separator = false;
                value.push(self.advance());
            } else if character == '_' {
                if previous_was_separator || !self.peek_next().is_some_and(|c| c.is_ascii_digit()) {
                    value.push(self.advance());
                    return Err(self.invalid_number(
                        value,
                        start,
                        "numeric separators must appear between digits",
                    ));
                }
                previous_was_separator = true;
                value.push(self.advance());
            } else {
                break;
            }
        }
        Ok(())
    }

    fn invalid_number(&self, value: &str, start: Position, reason: &str) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Lex,
            format!("invalid numeric literal `{value}`: {reason}"),
            Span::new(start, self.position),
        )
    }

    fn read_string(&mut self, start: Position) -> Result<String, Diagnostic> {
        let mut value = String::new();
        while !self.is_at_end() {
            match self.advance() {
                '"' => return Ok(value),
                '\\' => value.push(self.read_escape(start)?),
                '\n' => {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Lex,
                        "unterminated string literal",
                        Span::new(start, self.position),
                    ));
                }
                character => value.push(character),
            }
        }
        Err(Diagnostic::new(
            DiagnosticKind::Lex,
            "unterminated string literal at end of file",
            Span::new(start, self.position),
        ))
    }

    fn read_character(&mut self, start: Position) -> Result<char, Diagnostic> {
        if self.is_at_end() || self.peek() == Some('\n') {
            return Err(Diagnostic::new(
                DiagnosticKind::Lex,
                "unterminated character literal",
                Span::new(start, self.position),
            ));
        }
        let value = if self.peek() == Some('\\') {
            self.advance();
            self.read_escape(start)?
        } else {
            self.advance()
        };
        if !self.match_char('\'') {
            return Err(Diagnostic::new(
                DiagnosticKind::Lex,
                "character literal must contain exactly one Unicode scalar value",
                Span::new(start, self.position),
            ));
        }
        Ok(value)
    }

    fn read_escape(&mut self, start: Position) -> Result<char, Diagnostic> {
        if self.is_at_end() {
            return Err(Diagnostic::new(
                DiagnosticKind::Lex,
                "incomplete escape sequence",
                Span::new(start, self.position),
            ));
        }
        let escaped = self.advance();
        match escaped {
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            '0' => Ok('\0'),
            '\\' => Ok('\\'),
            '"' => Ok('"'),
            '\'' => Ok('\''),
            _ => Err(Diagnostic::new(
                DiagnosticKind::Lex,
                format!("unknown escape sequence `\\{escaped}`"),
                Span::new(start, self.position),
            )),
        }
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), Diagnostic> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.advance();
                }
                Some('/') if self.peek_next() == Some('/') => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.advance();
                    }
                }
                Some('/') if self.peek_next() == Some('*') => {
                    let start = self.position;
                    self.advance();
                    self.advance();
                    let mut depth = 1usize;
                    while depth > 0 && !self.is_at_end() {
                        if self.peek() == Some('/') && self.peek_next() == Some('*') {
                            self.advance();
                            self.advance();
                            depth += 1;
                        } else if self.peek() == Some('*') && self.peek_next() == Some('/') {
                            self.advance();
                            self.advance();
                            depth -= 1;
                        } else {
                            self.advance();
                        }
                    }
                    if depth != 0 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Lex,
                            "unterminated block comment",
                            Span::new(start, self.position),
                        ));
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn advance(&mut self) -> char {
        let character = self.source[self.current];
        self.current += 1;
        if character == '\n' {
            self.position.line += 1;
            self.position.column = 1;
        } else {
            self.position.column += 1;
        }
        character
    }

    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() != Some(expected) {
            return false;
        }
        self.advance();
        true
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.current).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.source.get(self.current + 1).copied()
    }

    fn is_at_end(&self) -> bool {
        self.current >= self.source.len()
    }
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || is_xid_start(character)
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || is_xid_continue(character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Result<Vec<TokenKind>, Diagnostic> {
        Ok(Lexer::new(source)
            .tokenize()?
            .into_iter()
            .map(|token| token.kind)
            .collect())
    }

    #[test]
    fn lexes_keywords_identifiers_and_unicode() {
        let tokens = kinds("fn main let export nombre 東京").expect("lexer should succeed");
        assert_eq!(tokens[0], TokenKind::Fn);
        assert_eq!(tokens[1], TokenKind::Identifier("main".into()));
        assert_eq!(tokens[2], TokenKind::Let);
        assert_eq!(tokens[3], TokenKind::Export);
        assert_eq!(tokens[4], TokenKind::Identifier("nombre".into()));
        assert_eq!(tokens[5], TokenKind::Identifier("東京".into()));
    }

    #[test]
    fn lexes_numbers_ranges_and_exponents() {
        let tokens = kinds("10 1_000 3.125 2e3 0..10").expect("lexer should succeed");
        assert_eq!(tokens[0], TokenKind::Integer(10));
        assert_eq!(tokens[1], TokenKind::Integer(1000));
        assert_eq!(tokens[2], TokenKind::Float(3.125));
        assert_eq!(tokens[3], TokenKind::Float(2000.0));
        assert_eq!(tokens[4], TokenKind::Integer(0));
        assert_eq!(tokens[5], TokenKind::Range);
    }

    #[test]
    fn rejects_malformed_numeric_separators() {
        for source in ["1_", "1__0", "1.0_"] {
            assert!(kinds(source).is_err(), "`{source}` should be rejected");
        }
    }

    #[test]
    fn lexes_string_character_and_nested_comments() {
        let tokens = kinds("/* outer /* inner */ done */ \"Hello\\nDISP\" '\\t'")
            .expect("lexer should succeed");
        assert_eq!(tokens[0], TokenKind::String("Hello\nDISP".into()));
        assert_eq!(tokens[1], TokenKind::Character('\t'));
    }

    #[test]
    fn tracks_end_exclusive_spans() {
        let tokens = Lexer::new("let x\n  = 1")
            .tokenize()
            .expect("lexer should succeed");
        assert_eq!(
            tokens[0].span,
            Span::new(
                Position { line: 1, column: 1 },
                Position { line: 1, column: 4 }
            )
        );
        assert_eq!(tokens[2].span.start, Position { line: 2, column: 3 });
    }

    #[test]
    fn rejects_unterminated_input_without_panicking() {
        assert!(kinds("\"unterminated").is_err());
        assert!(kinds("/* unterminated").is_err());
        assert!(kinds("'ab'").is_err());
    }
}
