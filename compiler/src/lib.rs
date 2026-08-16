pub mod ast;
pub mod backend;
pub mod cfg;
pub mod component_host;
pub mod const_eval;
pub mod crypto;
pub mod crypto_keystore;
mod data_store;
pub mod diagnostics;
pub mod effects;
pub mod expansion;
pub mod formatter;
pub mod freestanding;
pub mod freestanding32;
pub mod freestanding64;
pub mod freestanding_aarch64;
pub mod hir;
pub mod interpreter;
pub mod lexer;
pub mod limits;
pub mod mir;
pub mod ownership;
pub mod package;
pub mod parser;
#[doc(hidden)]
pub mod process_sandbox;
pub mod project;
pub mod resolver;
mod sqlite_compat;
pub mod type_checker;

use ast::Program;
use diagnostics::{Diagnostic, DiagnosticKind, Span};
use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;
use resolver::Resolver;
use std::{path::Path, thread};
use type_checker::TypeChecker;

pub use limits::MAX_SOURCE_BYTES;
use limits::TYPE_CHECKER_STACK_BYTES;

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

fn validate_program(mut program: Program) -> Result<Program, Diagnostic> {
    expansion::expand(&mut program)?;
    validate_expanded_program(program)
}

fn validate_expanded_program(mut program: Program) -> Result<Program, Diagnostic> {
    Resolver::new().resolve(&program)?;
    check_types(&program)?;
    ownership::check(&program)?;
    effects::analyze(&program)?;
    let constants = const_eval::evaluate(&program)?;
    const_eval::fold(&mut program, &constants);
    Ok(program)
}

fn check_types(program: &Program) -> Result<(), Diagnostic> {
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .name("disp-type-checker".into())
            .stack_size(TYPE_CHECKER_STACK_BYTES)
            .spawn_scoped(scope, || TypeChecker::new().check(program))
            .map_err(|error| {
                Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("could not start type checker: {error}"),
                    Span::point(1, 1),
                )
            })?;
        match worker.join() {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

pub fn expansion_report_source(source: &str) -> Result<expansion::Report, Diagnostic> {
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
    let mut program = Parser::new(tokens).parse()?;
    if let Some(import) = program.imports.first() {
        return Err(Diagnostic::new(
            DiagnosticKind::Resolve,
            "module imports require a source path or project directory",
            import.span,
        ));
    }
    let report = expansion::expand(&mut program)?;
    validate_expanded_program(program)?;
    Ok(report)
}

pub fn expansion_report_path(path: &Path) -> Result<expansion::Report, Diagnostic> {
    let project = project::load(path)?;
    let sources = project.sources;
    let mut program = project.program;
    let report = expansion::expand(&mut program).map_err(|error| sources.remap(error))?;
    validate_expanded_program(program).map_err(|error| sources.remap(error))?;
    Ok(report)
}

pub fn constant_report_source(source: &str) -> Result<const_eval::Report, Diagnostic> {
    let program = validate_source(source)?;
    const_eval::evaluate(&program)
}

pub fn constant_report_path(path: &Path) -> Result<const_eval::Report, Diagnostic> {
    let project = project::load(path)?;
    let sources = project.sources;
    let program = validate_program(project.program).map_err(|error| sources.remap(error))?;
    const_eval::evaluate(&program).map_err(|error| sources.remap(error))
}

pub fn effect_report_source(source: &str) -> Result<effects::Report, Diagnostic> {
    let program = validate_source(source)?;
    effects::analyze(&program)
}

pub fn effect_report_path(path: &Path) -> Result<effects::Report, Diagnostic> {
    let project = project::load(path)?;
    let sources = project.sources;
    let program = validate_program(project.program).map_err(|error| sources.remap(error))?;
    effects::analyze(&program).map_err(|error| sources.remap(error))
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
    reject_interpreter_device_io(&mir).map_err(|error| sources.remap(error))?;
    Interpreter::from_environment()?
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
    let hir = hir::lower(&program)?;
    let mir = mir::lower(&hir)?;
    reject_interpreter_device_io(&mir)?;
    Interpreter::new().run_with_args(&program, arguments)
}

fn reject_interpreter_device_io(program: &mir::Program) -> Result<(), Diagnostic> {
    if let Some(span) = program.functions.iter().find_map(|function| {
        function
            .blocks
            .iter()
            .find_map(|block| match &block.terminator {
                mir::Terminator::Call {
                    target: hir::CallTarget::Intrinsic(name),
                    span,
                    ..
                } if name.starts_with("Port.") || name.starts_with("Mmio.") => Some(*span),
                _ => None,
            })
    }) {
        Err(Diagnostic::new(
            DiagnosticKind::Runtime,
            "direct hardware I/O cannot execute in the hosted interpreter",
            span,
        )
        .with_help(
            "compile authorized port I/O with a freestanding x86 target, or authenticated MMIO with `--freestanding-aarch64`",
        ))
    } else {
        Ok(())
    }
}

pub fn lower_source(source: &str) -> Result<(hir::Program, mir::Program), Diagnostic> {
    let ast = validate_source(source)?;
    let hir = hir::lower(&ast)?;
    let mir = mir::lower(&hir)?;
    Ok((hir, mir))
}
