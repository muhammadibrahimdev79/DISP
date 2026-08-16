use crate::{
    MAX_SOURCE_BYTES,
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    lexer::Lexer,
    parser::Parser,
};

/// Formats one DISP source file without changing token contents or line boundaries.
///
/// The formatter deliberately starts conservatively: it validates the complete syntax,
/// normalizes line endings and indentation, removes trailing whitespace, and bounds blank
/// lines. Keeping tokens and statement line boundaries intact makes this first formatter
/// safe for every currently implemented surface feature, including comments and `unsafe`.
pub fn format_source(source: &str) -> Result<String, Diagnostic> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(Diagnostic::new(
            DiagnosticKind::Lex,
            format!(
                "source is {} bytes; the current safety limit is {MAX_SOURCE_BYTES} bytes",
                source.len()
            ),
            Span::point(1, 1),
        ));
    }

    parse(source)?;
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = String::with_capacity(normalized.len().saturating_add(1));
    let mut state = ScanState::default();
    let mut depth = 0usize;
    let mut previous_blank = false;

    for raw_line in normalized.lines() {
        let content = raw_line.trim();
        if content.is_empty() {
            if !output.is_empty() && !previous_blank {
                output.push('\n');
            }
            previous_blank = true;
            continue;
        }

        let leading_closes = leading_closing_braces(content, state);
        let line_depth = depth.saturating_sub(leading_closes);
        output.push_str(&"    ".repeat(line_depth));
        output.push_str(content);
        output.push('\n');
        previous_blank = false;

        let (opens, closes) = scan_braces(content, &mut state);
        depth = depth.saturating_add(opens).saturating_sub(closes);
    }

    while output.ends_with("\n\n") {
        output.pop();
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }

    // Whitespace is not allowed to turn valid syntax into a different or invalid program.
    parse(&output)?;
    Ok(output)
}

fn parse(source: &str) -> Result<(), Diagnostic> {
    let tokens = Lexer::new(source).tokenize()?;
    Parser::new(tokens).parse().map(|_| ())
}

#[derive(Clone, Copy, Default)]
struct ScanState {
    block_comment_depth: usize,
}

fn leading_closing_braces(line: &str, state: ScanState) -> usize {
    if state.block_comment_depth != 0 {
        return 0;
    }
    line.chars()
        .take_while(|character| *character == '}')
        .count()
}

fn scan_braces(line: &str, state: &mut ScanState) -> (usize, usize) {
    let characters: Vec<char> = line.chars().collect();
    let mut index = 0usize;
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut string = false;
    let mut character = false;
    let mut escaped = false;

    while index < characters.len() {
        let current = characters[index];
        let next = characters.get(index + 1).copied();

        if state.block_comment_depth != 0 {
            if current == '/' && next == Some('*') {
                state.block_comment_depth += 1;
                index += 2;
                continue;
            }
            if current == '*' && next == Some('/') {
                state.block_comment_depth -= 1;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }

        if string || character {
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if string && current == '"' {
                string = false;
            } else if character && current == '\'' {
                character = false;
            }
            index += 1;
            continue;
        }

        if current == '/' && next == Some('/') {
            break;
        }
        if current == '/' && next == Some('*') {
            state.block_comment_depth = 1;
            index += 2;
            continue;
        }
        match current {
            '"' => string = true,
            '\'' => character = true,
            '{' => opens += 1,
            '}' => closes += 1,
            _ => {}
        }
        index += 1;
    }

    (opens, closes)
}

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn formatting_is_valid_and_idempotent() {
        let source = "fn main() {  \r\n\tlet text = \"{ not a block }\"   \r\n\tif true {\r\n\t\tprint(text)\r\n\t}\r\n}\r\n";
        let expected = "fn main() {\n    let text = \"{ not a block }\"\n    if true {\n        print(text)\n    }\n}\n";
        let formatted = format_source(source).unwrap();
        assert_eq!(formatted, expected);
        assert_eq!(format_source(&formatted).unwrap(), formatted);
    }

    #[test]
    fn comments_do_not_change_indentation_depth() {
        let source = "fn main() {\n/* { nested /* } */ comment } */\n// }\nprint(\"safe\")\n}\n";
        let expected =
            "fn main() {\n    /* { nested /* } */ comment } */\n    // }\n    print(\"safe\")\n}\n";
        assert_eq!(format_source(source).unwrap(), expected);
    }

    #[test]
    fn malformed_source_is_not_rewritten() {
        assert!(format_source("fn main( {").is_err());
    }
}
