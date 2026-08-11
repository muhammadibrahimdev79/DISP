pub mod ast;
pub mod backend;
pub mod cfg;
pub mod diagnostics;
pub mod hir;
pub mod interpreter;
pub mod lexer;
pub mod mir;
pub mod ownership;
pub mod parser;
pub mod resolver;
pub mod type_checker;

use ast::Program;
use diagnostics::{Diagnostic, DiagnosticKind, Span};
use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;
use resolver::Resolver;
use type_checker::TypeChecker;

pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;

pub fn check_source(source: &str) -> Result<Program, Diagnostic> {
    let program = validate_source(source)?;
    let hir = hir::lower(&program)?;
    let mir = mir::lower(&hir)?;
    for function in &mir.functions {
        let _ = cfg::ControlFlowGraph::new(function);
    }
    Ok(program)
}

fn validate_source(source: &str) -> Result<Program, Diagnostic> {
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
    let tokens = Lexer::new(source).tokenize()?;
    let program = Parser::new(tokens).parse()?;
    Resolver::new().resolve(&program)?;
    TypeChecker::new().check(&program)?;
    ownership::check(&program)?;
    Ok(program)
}

pub fn run_source(source: &str) -> Result<Vec<String>, Diagnostic> {
    let program = check_source(source)?;
    Interpreter::new().run(&program)
}

pub fn lower_source(source: &str) -> Result<(hir::Program, mir::Program), Diagnostic> {
    let ast = validate_source(source)?;
    let hir = hir::lower(&ast)?;
    let mir = mir::lower(&hir)?;
    Ok((hir, mir))
}
