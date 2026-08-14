pub mod ast;
pub mod backend;
pub mod cfg;
mod data_store;
pub mod diagnostics;
pub mod hir;
pub mod interpreter;
pub mod lexer;
pub mod mir;
pub mod ownership;
pub mod package;
pub mod parser;
pub mod project;
pub mod resolver;
pub mod type_checker;

use ast::Program;
use diagnostics::{Diagnostic, DiagnosticKind, Span};
use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;
use resolver::Resolver;
use std::path::Path;
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
    if let Some(import) = program.imports.first() {
        return Err(Diagnostic::new(
            DiagnosticKind::Resolve,
            "module imports require a source path or project directory",
            import.span,
        )
        .with_help("use `check_path`, `lower_path`, or the `disp` command for multi-file code"));
    }
    validate_program(program)
}

fn validate_program(program: Program) -> Result<Program, Diagnostic> {
    Resolver::new().resolve(&program)?;
    TypeChecker::new().check(&program)?;
    ownership::check(&program)?;
    Ok(program)
}

pub fn check_path(path: &Path) -> Result<Program, Diagnostic> {
    let project = project::load(path)?;
    let sources = project.sources;
    let program = validate_program(project.program).map_err(|error| sources.remap(error))?;
    let hir = hir::lower(&program).map_err(|error| sources.remap(error))?;
    let mir = mir::lower(&hir).map_err(|error| sources.remap(error))?;
    for function in &mir.functions {
        let _ = cfg::ControlFlowGraph::new(function);
    }
    Ok(program)
}

pub fn run_path(path: &Path) -> Result<Vec<String>, Diagnostic> {
    run_path_with_args(path, &[])
}

pub fn run_path_with_args(path: &Path, arguments: &[String]) -> Result<Vec<String>, Diagnostic> {
    let project = project::load(path)?;
    let sources = project.sources;
    let program = validate_program(project.program).map_err(|error| sources.remap(error))?;
    let hir = hir::lower(&program).map_err(|error| sources.remap(error))?;
    let mir = mir::lower(&hir).map_err(|error| sources.remap(error))?;
    for function in &mir.functions {
        let _ = cfg::ControlFlowGraph::new(function);
    }
    Interpreter::new()
        .run_with_args(&program, arguments)
        .map_err(|error| sources.remap(error))
}

pub fn lower_path(path: &Path) -> Result<(hir::Program, mir::Program), Diagnostic> {
    let project = project::load(path)?;
    let sources = project.sources;
    let ast = validate_program(project.program).map_err(|error| sources.remap(error))?;
    let hir = hir::lower(&ast).map_err(|error| sources.remap(error))?;
    let mir = mir::lower(&hir).map_err(|error| sources.remap(error))?;
    Ok((hir, mir))
}

pub fn run_source(source: &str) -> Result<Vec<String>, Diagnostic> {
    run_source_with_args(source, &[])
}

pub fn run_source_with_args(source: &str, arguments: &[String]) -> Result<Vec<String>, Diagnostic> {
    let program = check_source(source)?;
    Interpreter::new().run_with_args(&program, arguments)
}

pub fn lower_source(source: &str) -> Result<(hir::Program, mir::Program), Diagnostic> {
    let ast = validate_source(source)?;
    let hir = hir::lower(&ast)?;
    let mir = mir::lower(&hir)?;
    Ok((hir, mir))
}
