//! Direct, runtime-free machine image generation for DISP's freestanding profile.

use crate::{
    ast::{
        AssignmentOperator, BinaryOperator, Block, Expr, Expression, Function, Program, Statement,
        UnaryOperator,
    },
    diagnostics::{Diagnostic, DiagnosticKind, Span},
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) const BOOT_SECTOR_BYTES: usize = 512;
pub(crate) const BOOT_PAYLOAD_BYTES: usize = 510;
const BOOT_ORIGIN: u16 = 0x7c00;
pub(crate) const STAGE_ORIGIN: u16 = 0x7e00;
pub(crate) const MAX_STAGE_SECTORS: usize = 64;
const LOCAL_MEMORY_ORIGIN: u16 = 0x6000;
const MAX_LOCALS: usize = 128;
const MAX_LOCAL_BYTES: usize = 4096;
const MAX_FUNCTIONS: usize = 256;
const STACK_FLOOR: u16 = LOCAL_MEMORY_ORIGIN + MAX_LOCAL_BYTES as u16;
const STACK_EXPRESSION_RESERVE: usize = 1024;

/// Builds a deterministic, legacy-x86 boot sector directly from validated DISP syntax.
///
/// The profile accepts exact-width allocation-free computation and guarded scalar functions.
/// The resulting image has no dependency on an OS, allocator, libc, linker, C compiler,
/// assembler, or Rust runtime.
pub fn build_x86_bios(program: &Program, source_path: &Path) -> Result<PathBuf, Diagnostic> {
    if !source_path.is_file()
        || source_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("disp")
    {
        return Err(error_at(
            source_path,
            Span::point(1, 1),
            "the x86 BIOS freestanding target currently requires one `.disp` source file",
        ));
    }

    let image = compile_x86_bios(program).map_err(|error| {
        if error.file.is_some() {
            error
        } else {
            error.with_file(source_path.display().to_string())
        }
    })?;
    let stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| {
            error_at(
                source_path,
                Span::point(1, 1),
                "the freestanding source filename must be valid UTF-8",
            )
        })?;
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let build = parent.join("build");
    fs::create_dir_all(&build).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not create freestanding build directory: {cause}"),
        )
    })?;
    let destination = build.join(format!("{stem}-x86-bios.img"));
    transactional_write(&destination, &image).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not write freestanding image safely: {cause}"),
        )
    })?;
    Ok(destination)
}

/// Compiles a validated DISP AST to a deterministic BIOS disk image.
pub fn compile_x86_bios(program: &Program) -> Result<Vec<u8>, Diagnostic> {
    reject_hosted_declarations(program)?;
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| {
            profile_error(
                Span::point(1, 1),
                "the freestanding profile requires a `fn main()` entry function",
            )
        })?;
    validate_function_signatures(program)?;
    validate_call_graph(program)?;

    let direct = Compiler::new(BOOT_ORIGIN, program)?.compile(program)?;
    if direct.len() <= BOOT_PAYLOAD_BYTES {
        let mut image = vec![0u8; BOOT_SECTOR_BYTES];
        image[..direct.len()].copy_from_slice(&direct);
        image[BOOT_PAYLOAD_BYTES..].copy_from_slice(&[0x55, 0xaa]);
        return Ok(image);
    }

    let stage = Compiler::new(STAGE_ORIGIN, program)?.compile(program)?;
    let sectors = stage.len().div_ceil(BOOT_SECTOR_BYTES);
    if sectors > MAX_STAGE_SECTORS {
        return Err(profile_error(
            main.body.span,
            format!(
                "freestanding stage needs {sectors} sectors but the safe real-mode limit is {MAX_STAGE_SECTORS}",
            ),
        )
        .with_help("reduce the program; protected-mode freestanding images arrive in a later profile"));
    }
    let mut image = boot_loader(sectors, main.body.span)?;
    image.resize((sectors + 1) * BOOT_SECTOR_BYTES, 0);
    image[BOOT_SECTOR_BYTES..BOOT_SECTOR_BYTES + stage.len()].copy_from_slice(&stage);
    Ok(image)
}

pub(crate) fn boot_loader(sectors: usize, span: Span) -> Result<Vec<u8>, Diagnostic> {
    let mut assembler = Assembler::default();
    let boot_drive = assembler.label();
    let packet = assembler.label();
    let failure = assembler.label();
    let halt = assembler.label();
    assembler.emit(&[
        0xfa, 0xea, 0x06, 0x7c, 0x00, 0x00, // cli; normalize CS
        0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, // zero segments
        0xbc, 0x00, 0x7c, 0xfb, // stack; sti
        0x88, 0x16, // mov [boot_drive], dl
    ]);
    assembler.absolute(boot_drive);
    assembler.emit(&[0xbe]); // mov si, disk-address packet
    assembler.absolute(packet);
    assembler.emit(&[0xb4, 0x42, 0x8a, 0x16]); // extended read; restore drive
    assembler.absolute(boot_drive);
    assembler.emit(&[0xcd, 0x13]);
    assembler.conditional_jump(0x82, failure); // jc failure
    assembler.emit(&[0xea, 0x00, 0x7e, 0x00, 0x00]); // jump to loaded stage
    assembler.bind(failure);
    assembler.emit(&[
        0xb0, b'!', 0xe6, 0xe9, 0xb4, 0x0e, 0xbb, 0x07, 0x00, 0xcd, 0x10,
    ]);
    assembler.jump(halt);
    assembler.bind(halt);
    assembler.emit(&[0xfa, 0xf4, 0xeb, 0xfd]);
    assembler.bind(boot_drive);
    assembler.emit(&[0]);
    while assembler.bytes.len() % 4 != 0 {
        assembler.emit(&[0]);
    }
    assembler.bind(packet);
    assembler.emit(&[0x10, 0x00]);
    assembler.immediate_u16(sectors as u16);
    assembler.immediate_u16(STAGE_ORIGIN);
    assembler.immediate_u16(0);
    assembler.emit(&1u64.to_le_bytes());
    let loader = assembler.finish(BOOT_ORIGIN, span)?;
    if loader.len() > BOOT_PAYLOAD_BYTES {
        return Err(Diagnostic::new(
            DiagnosticKind::Internal,
            "freestanding disk loader exceeded one boot sector",
            span,
        ));
    }
    let mut sector = vec![0u8; BOOT_SECTOR_BYTES];
    sector[..loader.len()].copy_from_slice(&loader);
    sector[BOOT_PAYLOAD_BYTES..].copy_from_slice(&[0x55, 0xaa]);
    Ok(sector)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueKind {
    U8,
    U16,
    U32,
    I32,
    Bool,
}

impl ValueKind {
    fn from_annotation(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "i32" => Some(Self::I32),
            "bool" => Some(Self::Bool),
            _ => None,
        }
    }

    const fn bytes(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 | Self::Bool => 2,
            Self::U32 | Self::I32 => 4,
        }
    }

    const fn numeric(self) -> bool {
        !matches!(self, Self::Bool)
    }

    const fn wide(self) -> bool {
        matches!(self, Self::U32 | Self::I32)
    }

    const fn byte_sized(self) -> bool {
        matches!(self, Self::U8)
    }

    const fn stack_bytes(self) -> usize {
        if self.wide() { 4 } else { 2 }
    }

    const fn signed(self) -> bool {
        matches!(self, Self::I32)
    }
}

fn validate_function_signatures(program: &Program) -> Result<(), Diagnostic> {
    if program.functions.len() > MAX_FUNCTIONS {
        return Err(profile_error(
            program
                .functions
                .get(MAX_FUNCTIONS)
                .map_or(Span::point(1, 1), |function| function.name_span),
            format!("freestanding programs support at most {MAX_FUNCTIONS} functions"),
        ));
    }
    for function in &program.functions {
        if function.name == "print" {
            return Err(profile_error(
                function.name_span,
                "`print` is reserved by the freestanding output ABI",
            ));
        }
        if function.asynchronous
            || !function.generics.is_empty()
            || function.capabilities.is_some()
            || function.external.is_some()
        {
            return Err(profile_error(
                function.span,
                "freestanding functions cannot be `async`, generic, capability-bearing, or external",
            ));
        }
        if function.name == "main"
            && (!function.parameters.is_empty() || function.return_type.is_some())
        {
            return Err(profile_error(
                function.span,
                "freestanding `main` must be plain `fn main()` with no parameters or return type",
            ));
        }
        for parameter in &function.parameters {
            if ValueKind::from_annotation(&parameter.ty.name).is_none() {
                return Err(profile_error(
                    parameter.ty.span,
                    "freestanding parameters support only `u8`, `u16`, `u32`, `i32`, and `bool`",
                ));
            }
        }
        if let Some(return_type) = &function.return_type
            && return_type.name != "Unit"
            && ValueKind::from_annotation(&return_type.name).is_none()
        {
            return Err(profile_error(
                return_type.span,
                "freestanding returns support only `u8`, `u16`, `u32`, `i32`, `bool`, and `Unit`",
            ));
        }
    }
    Ok(())
}

fn validate_call_graph(program: &Program) -> Result<(), Diagnostic> {
    let names = program
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect::<HashSet<_>>();
    for function in &program.functions {
        let mut calls = Vec::new();
        collect_block_calls(&function.body, &names, &mut calls);
        for (target, span) in calls {
            if target == "main" {
                return Err(profile_error(
                    span,
                    "freestanding `main` is an entry point and cannot be called",
                ));
            }
        }
    }
    Ok(())
}

fn collect_block_calls(block: &Block, names: &HashSet<&str>, output: &mut Vec<(String, Span)>) {
    for statement in &block.statements {
        match &statement.node {
            Statement::Binding {
                value: Some(value), ..
            } => collect_expression_calls(value, names, output),
            Statement::Assignment { value, .. } => collect_expression_calls(value, names, output),
            Statement::Expression(value) => collect_expression_calls(value, names, output),
            Statement::Return(Some(value)) => collect_expression_calls(value, names, output),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                collect_expression_calls(condition, names, output);
                collect_block_calls(then_branch, names, output);
                if let Some(else_branch) = else_branch {
                    collect_block_calls(else_branch, names, output);
                }
            }
            Statement::While { condition, body } => {
                collect_expression_calls(condition, names, output);
                collect_block_calls(body, names, output);
            }
            Statement::Loop(body) => collect_block_calls(body, names, output),
            _ => {}
        }
    }
}

fn collect_expression_calls(
    expression: &Expr,
    names: &HashSet<&str>,
    output: &mut Vec<(String, Span)>,
) {
    match &expression.node {
        Expression::Call { callee, arguments } => {
            if let Expression::Identifier(name) = &callee.node
                && names.contains(name.as_str())
            {
                output.push((name.clone(), expression.span));
            }
            collect_expression_calls(callee, names, output);
            for argument in arguments {
                collect_expression_calls(argument, names, output);
            }
        }
        Expression::Unary { operand, .. } => collect_expression_calls(operand, names, output),
        Expression::Binary { left, right, .. } => {
            collect_expression_calls(left, names, output);
            collect_expression_calls(right, names, output);
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
struct Local {
    address: u16,
    kind: ValueKind,
}

#[derive(Clone)]
struct FunctionInfo {
    label: Label,
    parameters: Vec<(String, Local)>,
    frame_slots: Vec<Local>,
    return_kind: Option<ValueKind>,
}

#[derive(Clone, Copy)]
struct LoopContext {
    continue_target: Label,
    break_target: Label,
}

#[derive(Clone, Copy)]
struct Label(usize);

struct Fixup {
    immediate: usize,
    label: Label,
    relative: bool,
}

#[derive(Default)]
struct Assembler {
    bytes: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
}

impl Assembler {
    fn label(&mut self) -> Label {
        let label = Label(self.labels.len());
        self.labels.push(None);
        label
    }

    fn bind(&mut self, label: Label) {
        assert!(self.labels[label.0].replace(self.bytes.len()).is_none());
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn immediate_u16(&mut self, value: u16) {
        self.emit(&value.to_le_bytes());
    }

    fn absolute(&mut self, label: Label) {
        let immediate = self.bytes.len();
        self.emit(&[0, 0]);
        self.fixups.push(Fixup {
            immediate,
            label,
            relative: false,
        });
    }

    fn relative(&mut self, label: Label) {
        let immediate = self.bytes.len();
        self.emit(&[0, 0]);
        self.fixups.push(Fixup {
            immediate,
            label,
            relative: true,
        });
    }

    fn call(&mut self, label: Label) {
        self.emit(&[0xe8]);
        self.relative(label);
    }

    fn jump(&mut self, label: Label) {
        self.emit(&[0xe9]);
        self.relative(label);
    }

    fn conditional_jump(&mut self, condition: u8, label: Label) {
        self.emit(&[0x0f, condition]);
        self.relative(label);
    }

    fn finish(mut self, origin: u16, span: Span) -> Result<Vec<u8>, Diagnostic> {
        for fixup in self.fixups {
            let target = self.labels[fixup.label.0].ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Internal,
                    "unbound freestanding machine-code label",
                    span,
                )
            })?;
            let value = if fixup.relative {
                let displacement = target as isize - (fixup.immediate + 2) as isize;
                i16::try_from(displacement).map_err(|_| {
                    profile_error(span, "freestanding branch exceeds the 16-bit target range")
                })? as u16
            } else {
                origin
                    .checked_add(u16::try_from(target).map_err(|_| {
                        profile_error(span, "freestanding image exceeds the 16-bit address space")
                    })?)
                    .ok_or_else(|| {
                        profile_error(span, "freestanding image exceeds the 16-bit address space")
                    })?
            };
            self.bytes[fixup.immediate..fixup.immediate + 2].copy_from_slice(&value.to_le_bytes());
        }
        Ok(self.bytes)
    }
}

struct Compiler {
    assembler: Assembler,
    scopes: Vec<HashMap<String, Local>>,
    data: Vec<(Label, Vec<u8>)>,
    next_local: usize,
    local_bytes: usize,
    functions: HashMap<String, FunctionInfo>,
    preallocated_locals: HashMap<(String, Span), Local>,
    current_function: String,
    current_return: Option<ValueKind>,
    current_is_main: bool,
    loops: Vec<LoopContext>,
    emit_character: Label,
    print_string: Label,
    print_unsigned: Label,
    print_signed: Label,
    newline: Label,
    arithmetic_failure: Label,
    stack_failure: Label,
    halt: Label,
    origin: u16,
}

impl Compiler {
    fn new(origin: u16, program: &Program) -> Result<Self, Diagnostic> {
        let mut assembler = Assembler::default();
        let emit_character = assembler.label();
        let print_string = assembler.label();
        let print_unsigned = assembler.label();
        let print_signed = assembler.label();
        let newline = assembler.label();
        let arithmetic_failure = assembler.label();
        let stack_failure = assembler.label();
        let halt = assembler.label();
        let mut compiler = Self {
            assembler,
            scopes: Vec::new(),
            data: Vec::new(),
            next_local: 0,
            local_bytes: 0,
            functions: HashMap::new(),
            preallocated_locals: HashMap::new(),
            current_function: String::new(),
            current_return: None,
            current_is_main: true,
            loops: Vec::new(),
            emit_character,
            print_string,
            print_unsigned,
            print_signed,
            newline,
            arithmetic_failure,
            stack_failure,
            halt,
            origin,
        };
        for function in &program.functions {
            let label = compiler.assembler.label();
            let mut parameters = Vec::new();
            let mut frame_slots = Vec::new();
            for parameter in &function.parameters {
                let kind = ValueKind::from_annotation(&parameter.ty.name).ok_or_else(|| {
                    profile_error(parameter.ty.span, "unsupported freestanding parameter type")
                })?;
                let local = compiler.allocate_local(kind, parameter.ty.span)?;
                parameters.push((parameter.name.clone(), local));
                frame_slots.push(local);
            }
            compiler.preallocate_block_locals(&function.name, &function.body, &mut frame_slots)?;
            let return_kind = function
                .return_type
                .as_ref()
                .and_then(|return_type| ValueKind::from_annotation(&return_type.name));
            compiler.functions.insert(
                function.name.clone(),
                FunctionInfo {
                    label,
                    parameters,
                    frame_slots,
                    return_kind,
                },
            );
        }
        Ok(compiler)
    }

    fn compile(mut self, program: &Program) -> Result<Vec<u8>, Diagnostic> {
        self.emit_boot_prelude();
        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("freestanding main was validated");
        self.compile_function(main, true)?;
        self.assembler.jump(self.halt);
        for function in &program.functions {
            if function.name != "main" {
                self.compile_function(function, false)?;
            }
        }
        self.emit_routines();
        for (label, bytes) in std::mem::take(&mut self.data) {
            self.assembler.bind(label);
            self.assembler.emit(&bytes);
        }
        self.assembler.finish(self.origin, main.body.span)
    }

    fn compile_function(&mut self, function: &Function, main: bool) -> Result<(), Diagnostic> {
        let info = self
            .functions
            .get(&function.name)
            .expect("freestanding function was registered")
            .clone();
        self.assembler.bind(info.label);
        self.current_return = info.return_kind;
        self.current_is_main = main;
        self.current_function.clone_from(&function.name);
        self.loops.clear();
        self.scopes.push(info.parameters.into_iter().collect());
        self.compile_block(&function.body)?;
        self.scopes.pop();
        if !main {
            if self.current_return.is_some() {
                self.assembler.jump(self.arithmetic_failure);
            } else {
                self.assembler.emit(&[0xc3]);
            }
        }
        Ok(())
    }

    fn emit_boot_prelude(&mut self) {
        let normalized_entry = self.origin + 6;
        self.assembler.emit(&[
            0xfa, // cli
            0xea, // far jmp 0x0000:origin+6
        ]);
        self.assembler.immediate_u16(normalized_entry);
        self.assembler.emit(&[
            0x00, 0x00, 0x31, 0xc0, // xor ax, ax
            0x8e, 0xd8, // mov ds, ax
            0x8e, 0xc0, // mov es, ax
            0x8e, 0xd0, // mov ss, ax
            0xbc, 0x00, 0x7c, // mov sp, 0x7c00
            0xfb, // sti
        ]);
    }

    fn compile_block(&mut self, block: &Block) -> Result<(), Diagnostic> {
        self.scopes.push(HashMap::new());
        for statement in &block.statements {
            self.compile_statement(&statement.node, statement.span)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn compile_statement(&mut self, statement: &Statement, span: Span) -> Result<(), Diagnostic> {
        match statement {
            Statement::Binding {
                kind: _,
                name,
                annotation,
                value,
                ..
            } => {
                let annotation = annotation.as_ref().ok_or_else(|| {
                    profile_error(
                        span,
                        "freestanding locals require an explicit `u8`, `u16`, `u32`, `i32`, or `bool` annotation",
                    )
                })?;
                let local_kind = ValueKind::from_annotation(&annotation.name).ok_or_else(|| {
                    profile_error(
                        annotation.span,
                        "freestanding locals support only `u8`, `u16`, `u32`, `i32`, and `bool`",
                    )
                })?;
                let value = value.as_ref().ok_or_else(|| {
                    profile_error(
                        span,
                        "freestanding locals must be initialized when declared",
                    )
                })?;
                let value_kind = self.compile_expression(value, Some(local_kind))?;
                self.require_kind(value_kind, local_kind, value.span)?;
                let local = self
                    .preallocated_locals
                    .get(&(self.current_function.clone(), span))
                    .copied()
                    .expect("freestanding local was preallocated");
                debug_assert_eq!(local.kind, local_kind);
                self.store(local);
                self.scopes
                    .last_mut()
                    .expect("freestanding block scope exists")
                    .insert(name.clone(), local);
                Ok(())
            }
            Statement::Assignment {
                name,
                operator,
                value,
                ..
            } => {
                let local = self.lookup(name, span)?;
                let kind = self.compile_expression(value, Some(local.kind))?;
                self.require_kind(kind, local.kind, value.span)?;
                if *operator != AssignmentOperator::Assign {
                    self.move_accumulator_to_secondary(local.kind);
                    self.load(local);
                    self.emit_assignment_operator(*operator, local.kind);
                }
                self.store(local);
                Ok(())
            }
            Statement::Expression(expression) => self.compile_call_statement(expression),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let kind = self.compile_expression(condition, Some(ValueKind::Bool))?;
                self.require_kind(kind, ValueKind::Bool, condition.span)?;
                self.assembler.emit(&[0x85, 0xc0]); // test ax, ax
                let alternate = self.assembler.label();
                let end = self.assembler.label();
                self.assembler.conditional_jump(0x84, alternate); // je
                self.compile_block(then_branch)?;
                self.assembler.jump(end);
                self.assembler.bind(alternate);
                if let Some(else_branch) = else_branch {
                    self.compile_block(else_branch)?;
                }
                self.assembler.bind(end);
                Ok(())
            }
            Statement::While { condition, body } => {
                let start = self.assembler.label();
                let end = self.assembler.label();
                self.assembler.bind(start);
                let kind = self.compile_expression(condition, Some(ValueKind::Bool))?;
                self.require_kind(kind, ValueKind::Bool, condition.span)?;
                self.assembler.emit(&[0x85, 0xc0]); // test ax, ax
                self.assembler.conditional_jump(0x84, end); // je
                self.loops.push(LoopContext {
                    continue_target: start,
                    break_target: end,
                });
                self.compile_block(body)?;
                self.loops.pop();
                self.assembler.jump(start);
                self.assembler.bind(end);
                Ok(())
            }
            Statement::Loop(body) => {
                let start = self.assembler.label();
                let end = self.assembler.label();
                self.assembler.bind(start);
                self.loops.push(LoopContext {
                    continue_target: start,
                    break_target: end,
                });
                self.compile_block(body)?;
                self.loops.pop();
                self.assembler.jump(start);
                self.assembler.bind(end);
                Ok(())
            }
            Statement::Break => {
                let context = self.loops.last().copied().ok_or_else(|| {
                    profile_error(span, "freestanding `break` requires an enclosing loop")
                })?;
                self.assembler.jump(context.break_target);
                Ok(())
            }
            Statement::Continue => {
                let context = self.loops.last().copied().ok_or_else(|| {
                    profile_error(span, "freestanding `continue` requires an enclosing loop")
                })?;
                self.assembler.jump(context.continue_target);
                Ok(())
            }
            Statement::Return(value) => self.compile_return(value.as_ref(), span),
            _ => Err(profile_error(
                span,
                "this statement is not available in the allocation-free freestanding profile",
            )),
        }
    }

    fn compile_return(&mut self, value: Option<&Expr>, span: Span) -> Result<(), Diagnostic> {
        if self.current_is_main {
            if value.is_some() {
                return Err(profile_error(
                    span,
                    "freestanding `main` cannot return a value",
                ));
            }
            self.assembler.jump(self.halt);
            return Ok(());
        }
        match (self.current_return, value) {
            (None, None) => self.assembler.emit(&[0xc3]),
            (Some(expected), Some(value)) => {
                let actual = self.compile_expression(value, Some(expected))?;
                self.require_kind(actual, expected, value.span)?;
                self.assembler.emit(&[0xc3]);
            }
            (Some(_), None) => {
                return Err(profile_error(
                    span,
                    "freestanding scalar function must return a value",
                ));
            }
            (None, Some(_)) => {
                return Err(profile_error(
                    span,
                    "freestanding `Unit` function cannot return a value",
                ));
            }
        }
        Ok(())
    }

    fn compile_call_statement(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let Expression::Call { callee, arguments } = &expression.node else {
            return Err(profile_error(
                expression.span,
                "freestanding expression statements must be `print(value)` calls",
            ));
        };
        let Expression::Identifier(name) = &callee.node else {
            return Err(profile_error(
                expression.span,
                "freestanding calls require a direct function name",
            ));
        };
        if name != "print" {
            self.compile_user_call(name, arguments, None, expression.span)?;
            return Ok(());
        }
        if arguments.len() != 1 {
            return Err(profile_error(
                expression.span,
                "freestanding output requires one argument to `print`",
            ));
        }
        if let Expression::String(text) = &arguments[0].node {
            let label = self.add_string(text, arguments[0].span)?;
            self.assembler.emit(&[0xbe]); // mov si, string
            self.assembler.absolute(label);
            self.assembler.call(self.print_string);
        } else {
            let kind = self.compile_expression(&arguments[0], None)?;
            match kind {
                ValueKind::U8 | ValueKind::U16 => {
                    self.assembler.emit(&[0x66, 0x0f, 0xb7, 0xc0]); // movzx eax, ax
                    self.assembler.call(self.print_unsigned);
                }
                ValueKind::U32 => self.assembler.call(self.print_unsigned),
                ValueKind::I32 => self.assembler.call(self.print_signed),
                ValueKind::Bool => self.emit_print_bool(),
            }
        }
        self.assembler.call(self.newline);
        Ok(())
    }

    fn compile_expression(
        &mut self,
        expression: &Expr,
        expected: Option<ValueKind>,
    ) -> Result<ValueKind, Diagnostic> {
        match &expression.node {
            Expression::Integer(value) => {
                let kind = expected.unwrap_or(if *value <= u16::MAX.into() {
                    ValueKind::U16
                } else {
                    ValueKind::U32
                });
                if !kind.numeric() {
                    return Err(profile_error(
                        expression.span,
                        "integer literal cannot initialize a freestanding boolean",
                    ));
                }
                self.emit_integer_literal(*value, kind, expression.span)?;
                Ok(kind)
            }
            Expression::Bool(value) => {
                if let Some(expected) = expected {
                    self.require_kind(ValueKind::Bool, expected, expression.span)?;
                }
                self.assembler.emit(&[0xb8]);
                self.assembler.immediate_u16(u16::from(*value));
                Ok(ValueKind::Bool)
            }
            Expression::Identifier(name) => {
                let local = self.lookup(name, expression.span)?;
                if let Some(expected) = expected {
                    self.require_kind(local.kind, expected, expression.span)?;
                }
                self.load(local);
                Ok(local.kind)
            }
            Expression::Unary {
                operator: UnaryOperator::Not,
                operand,
            } => {
                let kind = self.compile_expression(operand, Some(ValueKind::Bool))?;
                self.require_kind(kind, ValueKind::Bool, operand.span)?;
                self.assembler
                    .emit(&[0x85, 0xc0, 0x0f, 0x94, 0xc0, 0x30, 0xe4]);
                Ok(ValueKind::Bool)
            }
            Expression::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => {
                if expected.is_some_and(|kind| kind != ValueKind::I32) {
                    return Err(profile_error(
                        expression.span,
                        "freestanding negation requires an `i32` value",
                    ));
                }
                if let Expression::Integer(value) = operand.node
                    && value == (i32::MAX as u128) + 1
                {
                    self.emit_u32_literal(0x8000_0000);
                    return Ok(ValueKind::I32);
                }
                let kind = self.compile_expression(operand, Some(ValueKind::I32))?;
                self.require_kind(kind, ValueKind::I32, operand.span)?;
                self.assembler.emit(&[0x66, 0xf7, 0xd8]); // neg eax
                self.assembler
                    .conditional_jump(0x80, self.arithmetic_failure); // jo
                Ok(ValueKind::I32)
            }
            Expression::Binary {
                left,
                operator,
                right,
            } if matches!(operator, BinaryOperator::And | BinaryOperator::Or) => {
                self.compile_short_circuit(left, *operator, right)
            }
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let operand_kind =
                    self.infer_binary_kind(left, right, expected, expression.span)?;
                let left_kind = self.compile_expression(left, Some(operand_kind))?;
                self.push_accumulator(operand_kind);
                let right_kind = self.compile_expression(right, Some(operand_kind))?;
                self.move_accumulator_to_secondary(operand_kind);
                self.pop_accumulator(operand_kind);
                self.emit_binary_operator(*operator, left_kind, right_kind, expression.span)
            }
            Expression::Call { callee, arguments } => {
                let Expression::Identifier(name) = &callee.node else {
                    return Err(profile_error(
                        expression.span,
                        "freestanding calls require a direct function name",
                    ));
                };
                if name == "print" {
                    return Err(profile_error(
                        expression.span,
                        "`print` returns `Unit` and cannot be used as a value",
                    ));
                }
                self.compile_user_call(name, arguments, expected, expression.span)?
                    .ok_or_else(|| {
                        profile_error(
                            expression.span,
                            format!("freestanding `Unit` function `{name}` has no value"),
                        )
                    })
            }
            _ => Err(profile_error(
                expression.span,
                "this expression is not available in the allocation-free freestanding profile",
            )),
        }
    }

    fn compile_user_call(
        &mut self,
        name: &str,
        arguments: &[Expr],
        expected: Option<ValueKind>,
        span: Span,
    ) -> Result<Option<ValueKind>, Diagnostic> {
        let info = self.functions.get(name).cloned().ok_or_else(|| {
            profile_error(span, format!("`{name}` is not a freestanding function"))
        })?;
        if name == "main" {
            return Err(profile_error(
                span,
                "freestanding `main` is an entry point and cannot be called",
            ));
        }
        if arguments.len() != info.parameters.len() {
            return Err(profile_error(
                span,
                format!(
                    "freestanding function `{name}` expects {} arguments but received {}",
                    info.parameters.len(),
                    arguments.len()
                ),
            ));
        }
        let frame_bytes = info
            .frame_slots
            .iter()
            .map(|local| local.kind.stack_bytes())
            .sum::<usize>();
        let argument_bytes = info
            .parameters
            .iter()
            .map(|(_, local)| local.kind.stack_bytes())
            .sum::<usize>();
        self.guard_stack(frame_bytes + argument_bytes + 2, span)?;
        for local in &info.frame_slots {
            self.load(*local);
            self.push_accumulator(local.kind);
        }
        for (argument, (_, parameter)) in arguments.iter().zip(&info.parameters) {
            let actual = self.compile_expression(argument, Some(parameter.kind))?;
            self.require_kind(actual, parameter.kind, argument.span)?;
            self.push_accumulator(parameter.kind);
        }
        for (_, parameter) in info.parameters.iter().rev() {
            self.pop_accumulator(parameter.kind);
            self.store(*parameter);
        }
        self.assembler.call(info.label);
        if let Some(kind) = info.return_kind {
            self.move_accumulator_to_tertiary(kind);
        }
        for local in info.frame_slots.iter().rev() {
            self.pop_accumulator(local.kind);
            self.store(*local);
        }
        if let Some(kind) = info.return_kind {
            self.move_tertiary_to_accumulator(kind);
        }
        if let (Some(expected), Some(actual)) = (expected, info.return_kind) {
            self.require_kind(actual, expected, span)?;
        }
        Ok(info.return_kind)
    }

    fn compile_short_circuit(
        &mut self,
        left: &Expr,
        operator: BinaryOperator,
        right: &Expr,
    ) -> Result<ValueKind, Diagnostic> {
        let left_kind = self.compile_expression(left, Some(ValueKind::Bool))?;
        self.require_kind(left_kind, ValueKind::Bool, left.span)?;
        self.assembler.emit(&[0x85, 0xc0]);
        let shortcut = self.assembler.label();
        let end = self.assembler.label();
        self.assembler.conditional_jump(
            if operator == BinaryOperator::And {
                0x84
            } else {
                0x85
            },
            shortcut,
        );
        let right_kind = self.compile_expression(right, Some(ValueKind::Bool))?;
        self.require_kind(right_kind, ValueKind::Bool, right.span)?;
        self.assembler.jump(end);
        self.assembler.bind(shortcut);
        self.assembler.emit(&[0xb8]);
        self.assembler
            .immediate_u16(u16::from(operator == BinaryOperator::Or));
        self.assembler.bind(end);
        Ok(ValueKind::Bool)
    }

    fn infer_binary_kind(
        &self,
        left: &Expr,
        right: &Expr,
        expected: Option<ValueKind>,
        span: Span,
    ) -> Result<ValueKind, Diagnostic> {
        let expected = expected.filter(|kind| kind.numeric());
        let left_hint = self.expression_hint(left)?;
        let right_hint = self.expression_hint(right)?;
        let mut selected = expected;
        for hint in [left_hint, right_hint].into_iter().flatten() {
            if hint == ValueKind::Bool {
                if selected.is_some_and(|selected| selected != ValueKind::Bool) {
                    return Err(profile_error(
                        span,
                        "freestanding binary operands have different types",
                    ));
                }
                selected = Some(ValueKind::Bool);
            } else if let Some(selected_kind) = selected {
                if selected_kind != hint {
                    return Err(profile_error(
                        span,
                        "freestanding arithmetic does not implicitly mix integer widths or signedness",
                    ));
                }
            } else {
                selected = Some(hint);
            }
        }
        Ok(selected.unwrap_or_else(|| {
            if Self::contains_large_literal(left) || Self::contains_large_literal(right) {
                ValueKind::U32
            } else {
                ValueKind::U16
            }
        }))
    }

    fn expression_hint(&self, expression: &Expr) -> Result<Option<ValueKind>, Diagnostic> {
        match &expression.node {
            Expression::Identifier(name) => Ok(Some(self.lookup(name, expression.span)?.kind)),
            Expression::Bool(_) => Ok(Some(ValueKind::Bool)),
            Expression::Integer(_) => Ok(None),
            Expression::Unary {
                operator: UnaryOperator::Negate,
                ..
            } => Ok(Some(ValueKind::I32)),
            Expression::Unary {
                operator: UnaryOperator::Not,
                ..
            } => Ok(Some(ValueKind::Bool)),
            Expression::Binary {
                operator:
                    BinaryOperator::Equal
                    | BinaryOperator::NotEqual
                    | BinaryOperator::Less
                    | BinaryOperator::LessEqual
                    | BinaryOperator::Greater
                    | BinaryOperator::GreaterEqual
                    | BinaryOperator::And
                    | BinaryOperator::Or,
                ..
            } => Ok(Some(ValueKind::Bool)),
            Expression::Binary { left, right, .. } => {
                let left = self.expression_hint(left)?;
                let right = self.expression_hint(right)?;
                match (left, right) {
                    (Some(left), Some(right)) if left != right => Err(profile_error(
                        expression.span,
                        "freestanding expression mixes integer widths or signedness",
                    )),
                    (Some(kind), _) | (_, Some(kind)) => Ok(Some(kind)),
                    _ => Ok(None),
                }
            }
            Expression::Call { callee, .. } => {
                if let Expression::Identifier(name) = &callee.node {
                    Ok(self
                        .functions
                        .get(name)
                        .and_then(|function| function.return_kind))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn contains_large_literal(expression: &Expr) -> bool {
        match &expression.node {
            Expression::Integer(value) => *value > u16::MAX.into(),
            Expression::Unary { operand, .. } => Self::contains_large_literal(operand),
            Expression::Binary { left, right, .. } => {
                Self::contains_large_literal(left) || Self::contains_large_literal(right)
            }
            Expression::Call { arguments, .. } => {
                arguments.iter().any(Self::contains_large_literal)
            }
            _ => false,
        }
    }

    fn emit_integer_literal(
        &mut self,
        value: u128,
        kind: ValueKind,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match kind {
            ValueKind::U8 => {
                let value = u8::try_from(value)
                    .map_err(|_| profile_error(span, "freestanding `u8` literal exceeds 255"))?;
                self.assembler.emit(&[0xb8]);
                self.assembler.immediate_u16(value.into());
            }
            ValueKind::U16 => {
                let value = u16::try_from(value)
                    .map_err(|_| profile_error(span, "freestanding `u16` literal exceeds 65535"))?;
                self.assembler.emit(&[0xb8]);
                self.assembler.immediate_u16(value);
            }
            ValueKind::U32 => {
                let value = u32::try_from(value).map_err(|_| {
                    profile_error(span, "freestanding `u32` literal exceeds 4294967295")
                })?;
                self.emit_u32_literal(value);
            }
            ValueKind::I32 => {
                let value = i32::try_from(value).map_err(|_| {
                    profile_error(
                        span,
                        "positive freestanding `i32` literal exceeds 2147483647",
                    )
                })?;
                self.emit_u32_literal(value as u32);
            }
            ValueKind::Bool => unreachable!(),
        }
        Ok(())
    }

    fn emit_u32_literal(&mut self, value: u32) {
        self.assembler.emit(&[0x66, 0xb8]);
        self.assembler.emit(&value.to_le_bytes());
    }

    fn emit_binary_operator(
        &mut self,
        operator: BinaryOperator,
        left: ValueKind,
        right: ValueKind,
        span: Span,
    ) -> Result<ValueKind, Diagnostic> {
        if left != right {
            return Err(profile_error(
                span,
                "freestanding binary operands must have exactly the same type",
            ));
        }
        match operator {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                if !left.numeric() {
                    return Err(profile_error(span, "boolean arithmetic is not available"));
                }
                self.emit_checked_arithmetic(operator, left);
                Ok(left)
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                self.emit_comparison(
                    if operator == BinaryOperator::Equal {
                        0x94
                    } else {
                        0x95
                    },
                    left,
                );
                Ok(ValueKind::Bool)
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if !left.numeric() {
                    return Err(profile_error(
                        span,
                        "ordered boolean comparison is not available",
                    ));
                }
                let condition = match (operator, left.signed()) {
                    (BinaryOperator::Less, false) => 0x92,
                    (BinaryOperator::LessEqual, false) => 0x96,
                    (BinaryOperator::Greater, false) => 0x97,
                    (BinaryOperator::GreaterEqual, false) => 0x93,
                    (BinaryOperator::Less, true) => 0x9c,
                    (BinaryOperator::LessEqual, true) => 0x9e,
                    (BinaryOperator::Greater, true) => 0x9f,
                    (BinaryOperator::GreaterEqual, true) => 0x9d,
                    _ => unreachable!(),
                };
                self.emit_comparison(condition, left);
                Ok(ValueKind::Bool)
            }
            BinaryOperator::And | BinaryOperator::Or => unreachable!(),
        }
    }

    fn emit_checked_arithmetic(&mut self, operator: BinaryOperator, kind: ValueKind) {
        if kind == ValueKind::U8 {
            self.emit_checked_u8(operator);
        } else if kind == ValueKind::U16 {
            self.emit_checked_u16(operator);
        } else if kind == ValueKind::I32 {
            self.emit_checked_i32(operator);
        } else {
            self.emit_checked_u32(operator);
        }
    }

    fn emit_checked_u8(&mut self, operator: BinaryOperator) {
        match operator {
            BinaryOperator::Add => {
                self.assembler.emit(&[0x01, 0xd8, 0x3d, 0xff, 0x00]); // add ax,bx; cmp ax,255
                self.assembler
                    .conditional_jump(0x87, self.arithmetic_failure); // ja
            }
            BinaryOperator::Subtract => {
                self.assembler.emit(&[0x29, 0xd8]); // sub ax,bx
                self.assembler
                    .conditional_jump(0x82, self.arithmetic_failure); // jb
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[0xf7, 0xe3, 0x3d, 0xff, 0x00]); // mul bx; cmp ax,255
                self.assembler
                    .conditional_jump(0x87, self.arithmetic_failure); // ja
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler.emit(&[0x85, 0xdb]);
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                self.assembler.emit(&[0x31, 0xd2, 0xf7, 0xf3]);
                if operator == BinaryOperator::Remainder {
                    self.assembler.emit(&[0x89, 0xd0]);
                }
            }
            _ => unreachable!(),
        }
    }

    fn emit_checked_u16(&mut self, operator: BinaryOperator) {
        match operator {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                self.assembler.emit(if operator == BinaryOperator::Add {
                    &[0x01, 0xd8]
                } else {
                    &[0x29, 0xd8]
                });
                self.assembler
                    .conditional_jump(0x82, self.arithmetic_failure);
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[0xf7, 0xe3, 0x09, 0xd2]);
                self.assembler
                    .conditional_jump(0x85, self.arithmetic_failure);
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler.emit(&[0x85, 0xdb]);
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                self.assembler.emit(&[0x31, 0xd2, 0xf7, 0xf3]);
                if operator == BinaryOperator::Remainder {
                    self.assembler.emit(&[0x89, 0xd0]);
                }
            }
            _ => unreachable!(),
        }
    }

    fn emit_checked_u32(&mut self, operator: BinaryOperator) {
        match operator {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                self.assembler.emit(if operator == BinaryOperator::Add {
                    &[0x66, 0x01, 0xd8]
                } else {
                    &[0x66, 0x29, 0xd8]
                });
                self.assembler
                    .conditional_jump(0x82, self.arithmetic_failure);
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[0x66, 0xf7, 0xe3, 0x66, 0x09, 0xd2]);
                self.assembler
                    .conditional_jump(0x85, self.arithmetic_failure);
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler.emit(&[0x66, 0x85, 0xdb]);
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                self.assembler.emit(&[0x66, 0x31, 0xd2, 0x66, 0xf7, 0xf3]);
                if operator == BinaryOperator::Remainder {
                    self.assembler.emit(&[0x66, 0x89, 0xd0]);
                }
            }
            _ => unreachable!(),
        }
    }

    fn emit_checked_i32(&mut self, operator: BinaryOperator) {
        match operator {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                self.assembler.emit(if operator == BinaryOperator::Add {
                    &[0x66, 0x01, 0xd8]
                } else {
                    &[0x66, 0x29, 0xd8]
                });
                self.assembler
                    .conditional_jump(0x80, self.arithmetic_failure); // jo
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[
                    0x66, 0xf7, 0xeb, // imul ebx => edx:eax
                    0x66, 0x89, 0xc1, // mov ecx,eax
                    0x66, 0xc1, 0xf9, 0x1f, // sar ecx,31
                    0x66, 0x39, 0xca, // cmp edx,ecx
                ]);
                self.assembler
                    .conditional_jump(0x85, self.arithmetic_failure);
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler.emit(&[0x66, 0x85, 0xdb]);
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                let safe = self.assembler.label();
                self.assembler.emit(&[0x66, 0x3d]); // cmp eax, INT_MIN
                self.assembler.emit(&0x8000_0000u32.to_le_bytes());
                self.assembler.conditional_jump(0x85, safe);
                self.assembler.emit(&[0x66, 0x83, 0xfb, 0xff]); // cmp ebx,-1
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                self.assembler.bind(safe);
                self.assembler.emit(&[0x66, 0x99, 0x66, 0xf7, 0xfb]); // cdq; idiv ebx
                if operator == BinaryOperator::Remainder {
                    self.assembler.emit(&[0x66, 0x89, 0xd0]);
                }
            }
            _ => unreachable!(),
        }
    }

    fn emit_assignment_operator(&mut self, operator: AssignmentOperator, kind: ValueKind) {
        let binary = match operator {
            AssignmentOperator::Assign => return,
            AssignmentOperator::Add => BinaryOperator::Add,
            AssignmentOperator::Subtract => BinaryOperator::Subtract,
            AssignmentOperator::Multiply => BinaryOperator::Multiply,
            AssignmentOperator::Divide => BinaryOperator::Divide,
        };
        self.emit_checked_arithmetic(binary, kind);
    }

    fn emit_comparison(&mut self, condition: u8, kind: ValueKind) {
        if kind.wide() {
            self.assembler
                .emit(&[0x66, 0x39, 0xd8, 0x0f, condition, 0xc0]);
        } else {
            self.assembler.emit(&[0x39, 0xd8, 0x0f, condition, 0xc0]);
        }
        self.assembler.emit(&[0x30, 0xe4]); // xor ah, ah
    }

    fn emit_print_bool(&mut self) {
        let false_label = self.assembler.label();
        let output = self.assembler.label();
        let true_text = self.add_string_literal(b"true\0");
        let false_text = self.add_string_literal(b"false\0");
        self.assembler.emit(&[0x85, 0xc0]);
        self.assembler.conditional_jump(0x84, false_label);
        self.assembler.emit(&[0xbe]);
        self.assembler.absolute(true_text);
        self.assembler.jump(output);
        self.assembler.bind(false_label);
        self.assembler.emit(&[0xbe]);
        self.assembler.absolute(false_text);
        self.assembler.bind(output);
        self.assembler.call(self.print_string);
    }

    fn emit_routines(&mut self) {
        self.assembler.bind(self.emit_character);
        self.assembler.emit(&[
            0x66, 0x53, 0x66, 0x51, 0x66, 0x52, 0x66, 0x56, // push ebx, ecx, edx, esi
            0xe6, 0xe9, // out 0xe9, al
            0xb4, 0x0e, 0xbb, 0x07, 0x00, 0xcd, 0x10, // BIOS teletype
            0x66, 0x5e, 0x66, 0x5a, 0x66, 0x59, 0x66, 0x5b, 0xc3, // restore and ret
        ]);

        self.assembler.bind(self.print_string);
        let string_loop = self.assembler.label();
        let string_done = self.assembler.label();
        self.assembler.bind(string_loop);
        self.assembler.emit(&[0xac, 0x84, 0xc0]); // lodsb; test al, al
        self.assembler.conditional_jump(0x84, string_done);
        self.assembler.call(self.emit_character);
        self.assembler.jump(string_loop);
        self.assembler.bind(string_done);
        self.assembler.emit(&[0xc3]);

        self.assembler.bind(self.print_unsigned);
        let nonzero = self.assembler.label();
        let divide = self.assembler.label();
        let digits = self.assembler.label();
        self.assembler.emit(&[0x66, 0x85, 0xc0]); // test eax, eax
        self.assembler.conditional_jump(0x85, nonzero);
        self.assembler.emit(&[0xb0, b'0']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xc3]);
        self.assembler.bind(nonzero);
        self.assembler.emit(&[0x31, 0xc9, 0x66, 0xbb, 10, 0, 0, 0]); // xor cx,cx; mov ebx,10
        self.assembler.bind(divide);
        self.assembler.emit(&[
            0x66, 0x31, 0xd2, 0x66, 0xf7, 0xf3, // xor edx,edx; div ebx
            0x66, 0x52, 0x41, 0x66, 0x85, 0xc0, // push edx; inc cx; test eax,eax
        ]);
        self.assembler.conditional_jump(0x85, divide);
        self.assembler.bind(digits);
        self.assembler.emit(&[0x66, 0x58, 0x04, b'0']); // pop eax; add al,'0'
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xe2]); // loop digits (short, fixed backward displacement)
        let displacement =
            self.assembler.bytes.len() + 1 - self.assembler.labels[digits.0].unwrap();
        self.assembler
            .emit(&[(0u8).wrapping_sub(displacement as u8), 0xc3]);

        self.assembler.bind(self.print_signed);
        let nonnegative = self.assembler.label();
        self.assembler.emit(&[0x66, 0x85, 0xc0]); // test eax,eax
        self.assembler.conditional_jump(0x89, nonnegative); // jns
        self.assembler.emit(&[0x66, 0x50, 0xb0, b'-']); // push eax; mov al,'-'
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0x66, 0x58, 0x66, 0xf7, 0xd8]); // pop eax; neg eax
        self.assembler.bind(nonnegative);
        self.assembler.jump(self.print_unsigned);

        self.assembler.bind(self.newline);
        self.assembler.emit(&[0xb0, b'\r']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xb0, b'\n']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xc3]);

        self.assembler.bind(self.arithmetic_failure);
        let failure = self.add_string_literal(b"freestanding arithmetic failure\0");
        self.assembler.emit(&[0xbe]);
        self.assembler.absolute(failure);
        self.assembler.call(self.print_string);
        self.assembler.call(self.newline);
        self.assembler.jump(self.halt);

        self.assembler.bind(self.stack_failure);
        let failure = self.add_string_literal(b"freestanding stack limit exceeded\0");
        self.assembler.emit(&[0xbe]);
        self.assembler.absolute(failure);
        self.assembler.call(self.print_string);
        self.assembler.call(self.newline);
        self.assembler.jump(self.halt);

        self.assembler.bind(self.halt);
        self.assembler.emit(&[0xfa, 0xf4, 0xeb, 0xfd]);
    }

    fn add_string(&mut self, text: &str, span: Span) -> Result<Label, Diagnostic> {
        let mut bytes = Vec::new();
        encode_text(text, span, &mut bytes)?;
        bytes.push(0);
        Ok(self.add_string_literal(&bytes))
    }

    fn add_string_literal(&mut self, bytes: &[u8]) -> Label {
        let label = self.assembler.label();
        self.data.push((label, bytes.to_vec()));
        label
    }

    fn allocate_local(&mut self, kind: ValueKind, span: Span) -> Result<Local, Diagnostic> {
        if self.next_local == MAX_LOCALS {
            return Err(profile_error(
                span,
                format!("freestanding programs support at most {MAX_LOCALS} local variables"),
            ));
        }
        let aligned = (self.local_bytes + kind.bytes() - 1) & !(kind.bytes() - 1);
        let next = aligned + kind.bytes();
        if next > MAX_LOCAL_BYTES {
            return Err(profile_error(
                span,
                format!("freestanding local storage exceeds {MAX_LOCAL_BYTES} bytes"),
            ));
        }
        let address = LOCAL_MEMORY_ORIGIN + aligned as u16;
        self.next_local += 1;
        self.local_bytes = next;
        Ok(Local { address, kind })
    }

    fn preallocate_block_locals(
        &mut self,
        function: &str,
        block: &Block,
        frame_slots: &mut Vec<Local>,
    ) -> Result<(), Diagnostic> {
        for statement in &block.statements {
            match &statement.node {
                Statement::Binding { annotation, .. } => {
                    let annotation = annotation.as_ref().ok_or_else(|| {
                        profile_error(
                            statement.span,
                            "freestanding locals require an explicit `u8`, `u16`, `u32`, `i32`, or `bool` annotation",
                        )
                    })?;
                    let kind = ValueKind::from_annotation(&annotation.name).ok_or_else(|| {
                        profile_error(
                            annotation.span,
                            "freestanding locals support only `u8`, `u16`, `u32`, `i32`, and `bool`",
                        )
                    })?;
                    let local = self.allocate_local(kind, statement.span)?;
                    self.preallocated_locals
                        .insert((function.to_owned(), statement.span), local);
                    frame_slots.push(local);
                }
                Statement::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.preallocate_block_locals(function, then_branch, frame_slots)?;
                    if let Some(else_branch) = else_branch {
                        self.preallocate_block_locals(function, else_branch, frame_slots)?;
                    }
                }
                Statement::While { body, .. } | Statement::Loop(body) => {
                    self.preallocate_block_locals(function, body, frame_slots)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn guard_stack(&mut self, required: usize, span: Span) -> Result<(), Diagnostic> {
        let threshold = usize::from(STACK_FLOOR)
            .checked_add(STACK_EXPRESSION_RESERVE)
            .and_then(|value| value.checked_add(required))
            .ok_or_else(|| profile_error(span, "freestanding stack requirement overflowed"))?;
        if threshold >= usize::from(BOOT_ORIGIN) {
            return Err(profile_error(
                span,
                "freestanding function frame cannot fit within the guarded real-mode stack",
            ));
        }
        self.assembler.emit(&[0x81, 0xfc]); // cmp sp, threshold
        self.assembler.immediate_u16(threshold as u16);
        self.assembler.conditional_jump(0x82, self.stack_failure); // jb
        Ok(())
    }

    fn lookup(&self, name: &str, span: Span) -> Result<Local, Diagnostic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| profile_error(span, format!("unknown freestanding local `{name}`")))
    }

    fn load(&mut self, local: Local) {
        if local.kind.byte_sized() {
            self.assembler.emit(&[0xa0]);
        } else if local.kind.wide() {
            self.assembler.emit(&[0x66, 0xa1]);
        } else {
            self.assembler.emit(&[0xa1]);
        }
        self.assembler.immediate_u16(local.address);
        if local.kind.byte_sized() {
            self.assembler.emit(&[0x30, 0xe4]); // xor ah,ah
        }
    }

    fn store(&mut self, local: Local) {
        if local.kind.byte_sized() {
            self.assembler.emit(&[0xa2]);
        } else if local.kind.wide() {
            self.assembler.emit(&[0x66, 0xa3]);
        } else {
            self.assembler.emit(&[0xa3]);
        }
        self.assembler.immediate_u16(local.address);
    }

    fn push_accumulator(&mut self, kind: ValueKind) {
        if kind.wide() {
            self.assembler.emit(&[0x66, 0x50]);
        } else {
            self.assembler.emit(&[0x50]);
        }
    }

    fn pop_accumulator(&mut self, kind: ValueKind) {
        if kind.wide() {
            self.assembler.emit(&[0x66, 0x58]);
        } else {
            self.assembler.emit(&[0x58]);
        }
    }

    fn move_accumulator_to_secondary(&mut self, kind: ValueKind) {
        if kind.wide() {
            self.assembler.emit(&[0x66, 0x89, 0xc3]);
        } else {
            self.assembler.emit(&[0x89, 0xc3]);
        }
    }

    fn move_accumulator_to_tertiary(&mut self, kind: ValueKind) {
        if kind.wide() {
            self.assembler.emit(&[0x66, 0x89, 0xc1]); // mov ecx,eax
        } else {
            self.assembler.emit(&[0x89, 0xc1]); // mov cx,ax
        }
    }

    fn move_tertiary_to_accumulator(&mut self, kind: ValueKind) {
        if kind.wide() {
            self.assembler.emit(&[0x66, 0x89, 0xc8]); // mov eax,ecx
        } else {
            self.assembler.emit(&[0x89, 0xc8]); // mov ax,cx
        }
    }

    fn require_kind(
        &self,
        actual: ValueKind,
        expected: ValueKind,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if actual == expected {
            Ok(())
        } else {
            Err(profile_error(
                span,
                "freestanding expression has an unsupported value type in this position",
            ))
        }
    }
}

fn encode_text(text: &str, span: Span, output: &mut Vec<u8>) -> Result<(), Diagnostic> {
    for character in text.chars() {
        match character {
            '\n' => output.extend_from_slice(b"\r\n"),
            '\r' => output.push(b'\r'),
            ' '..='~' => output.push(character as u8),
            _ => {
                return Err(profile_error(
                    span,
                    "x86 BIOS freestanding output currently supports printable ASCII, newline, and carriage return only",
                ));
            }
        }
    }
    Ok(())
}

fn reject_hosted_declarations(program: &Program) -> Result<(), Diagnostic> {
    let unsupported = program
        .module
        .as_ref()
        .map(|item| item.span)
        .or_else(|| program.imports.first().map(|item| item.span))
        .or_else(|| program.public_items.first().map(|item| item.span))
        .or_else(|| program.structs.first().map(|item| item.span))
        .or_else(|| program.enums.first().map(|item| item.span))
        .or_else(|| program.traits.first().map(|item| item.span))
        .or_else(|| program.implementations.first().map(|item| item.span));
    if let Some(span) = unsupported {
        return Err(profile_error(
            span,
            "modules, imports, visibility declarations, types, traits, and implementations are not yet available in the initial freestanding profile",
        ));
    }
    Ok(())
}

pub(crate) fn transactional_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("disp-image");
    for attempt in 0..128u32 {
        let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            replace_file(&temporary, path, attempt)?;
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not create a unique freestanding staging file",
    ))
}

fn replace_file(from: &Path, to: &Path, attempt: u32) -> std::io::Result<()> {
    if !to.exists() {
        return fs::rename(from, to);
    }
    let parent = to.parent().unwrap_or_else(|| Path::new("."));
    let name = to
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("disp-image");
    let backup = parent.join(format!(".{name}.{}.{}.backup", std::process::id(), attempt));
    fs::rename(to, &backup)?;
    if let Err(error) = fs::rename(from, to) {
        let _ = fs::rename(&backup, to);
        return Err(error);
    }
    fs::remove_file(backup)
}

fn profile_error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span).with_help(
        "use the hosted target, or reduce the program to the documented freestanding profile",
    )
}

fn error_at(path: &Path, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span).with_file(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_source;

    #[test]
    fn constant_print_compiles_to_deterministic_boot_sector() {
        let program = check_source("fn main() { print(\"Hello, DISP\") }").unwrap();
        let first = compile_x86_bios(&program).unwrap();
        let second = compile_x86_bios(&program).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 512);
        assert_eq!(&first[510..], &[0x55, 0xaa]);
        assert!(first.windows(12).any(|bytes| bytes == b"Hello, DISP\0"));
        assert_eq!(
            &first[..18],
            &[
                0xfa, 0xea, 0x06, 0x7c, 0x00, 0x00, 0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0,
                0xbc, 0x00, 0x7c, 0xfb
            ]
        );
    }

    #[test]
    fn hosted_constructs_are_rejected_instead_of_linked() {
        let program =
            check_source("fn helper(value: String) {} fn main() { print(\"x\") }").unwrap();
        let error = compile_x86_bios(&program).unwrap_err();
        assert!(error.message.contains("parameters support only"));

        let program = check_source("fn main() { let value = 1 print(\"x\") }").unwrap();
        let error = compile_x86_bios(&program).unwrap_err();
        assert!(error.message.contains("explicit `u8`"));
    }

    #[test]
    fn exact_u8_values_use_compact_storage_checked_math_and_safe_calls() {
        let program = check_source(
            r#"
fn byte_math(left: u8, right: u8) -> u8 {
    var sum: u8 = left + right
    var product: u8 = sum * 2
    var divisor: u8 = 2
    return product / divisor
}
fn main() {
    var byte: u8 = byte_math(120, 7)
    byte -= 2
    print(byte)
    print(byte > 100)
}
"#,
        )
        .unwrap();
        let image = compile_x86_bios(&program).unwrap();
        assert_eq!(&image[510..512], &[0x55, 0xaa]);
        assert!(image.windows(3).any(|bytes| bytes == [0xa2, 0x00, 0x60]));
        assert!(image.windows(3).any(|bytes| bytes == [0xa2, 0x01, 0x60]));
        // Five one-byte frame slots still consume five machine words when snapshotted;
        // two arguments consume two more, and the near return address consumes one.
        assert!(
            image
                .windows(6)
                .any(|bytes| bytes == [0x81, 0xfc, 0x10, 0x74, 0x0f, 0x82])
        );
        assert!(
            image
                .windows(5)
                .any(|bytes| bytes == [0x01, 0xd8, 0x3d, 0xff, 0x00])
        ); // checked u8 add
        assert!(
            image
                .windows(5)
                .any(|bytes| bytes == [0xf7, 0xe3, 0x3d, 0xff, 0x00])
        ); // checked u8 multiply
        assert!(
            image
                .windows(4)
                .any(|bytes| bytes == [0x31, 0xd2, 0xf7, 0xf3])
        );
        assert!(
            image
                .windows(4)
                .any(|bytes| bytes == [0x66, 0x0f, 0xb7, 0xc0])
        );
    }

    #[test]
    fn scalar_functions_preserve_nested_arguments_returns_and_recursive_frames() {
        let program = check_source(
            r#"
fn add(left: u32, right: u32) -> u32 {
    return left + right
}
fn nested(value: u32) -> u32 {
    return add(value, add(2, 3))
}
fn negate(value: i32) -> i32 {
    return -value
}
fn choose(flag: bool, yes: u32, no: u32) -> u32 {
    if flag { return yes }
    return no
}
fn factorial(value: u16) -> u16 {
    if value <= 1 { return 1 }
    var previous: u16 = value - 1
    var partial: u16 = factorial(previous)
    return value * partial
}
fn even(value: u16) -> bool {
    if value == 0 { return true }
    return odd(value - 1)
}
fn odd(value: u16) -> bool {
    if value == 0 { return false }
    return even(value - 1)
}
fn main() {
    print(nested(10))
    print(negate(-7))
    print(choose(true, 11, 22))
    print(factorial(6))
    print(even(10))
}
"#,
        )
        .unwrap();
        let image = compile_x86_bios(&program).unwrap();
        assert_eq!(&image[510..512], &[0x55, 0xaa]);
        assert!(image.windows(1).any(|bytes| bytes == [0xe8])); // near call
        assert!(image.windows(1).any(|bytes| bytes == [0xc3])); // near return
        assert!(image.windows(2).any(|bytes| bytes == [0x66, 0xa3]));
        assert!(
            image
                .windows(6)
                .any(|bytes| bytes[..2] == [0x81, 0xfc] && bytes[4..] == [0x0f, 0x82])
        ); // cmp sp, guarded floor; jb stack failure
        assert!(
            image
                .windows(34)
                .any(|bytes| bytes == b"freestanding stack limit exceeded\0")
        );

        let calls_main = check_source("fn helper() { main() } fn main() { helper() }").unwrap();
        let error = compile_x86_bios(&calls_main).unwrap_err();
        assert!(error.message.contains("entry point and cannot be called"));
    }

    #[test]
    fn structured_loops_bind_break_and_continue_to_the_innermost_scope() {
        let program = check_source(
            r#"
fn calculate() -> u16 {
    var outer: u16 = 0
    var total: u16 = 0
    loop {
        outer += 1
        if outer > 3 { break }
        var inner: u16 = 0
        loop {
            inner += 1
            if inner == 2 { continue }
            if inner > 3 { break }
            total += outer
        }
    }
    return total
}
fn main() { print(calculate()) }
"#,
        )
        .unwrap();
        let image = compile_x86_bios(&program).unwrap();
        assert_eq!(&image[510..512], &[0x55, 0xaa]);
        assert!(image.iter().filter(|byte| **byte == 0xe9).count() >= 6);

        let invalid = check_source("fn main() { break }").unwrap_err();
        assert!(invalid.message.contains("inside loops"));
    }

    #[test]
    fn allocation_free_u16_control_flow_emits_checked_machine_code() {
        let program = check_source(
            r#"
fn main() {
    var total: u16 = 0
    var next: u16 = 1
    while next <= 10 {
        total += next
        next += 1
    }
    if total == 55 {
        print(total)
    }
}
"#,
        )
        .unwrap();
        let image = compile_x86_bios(&program).unwrap();
        assert_eq!(&image[510..], &[0x55, 0xaa]);
        assert!(image.windows(2).any(|bytes| bytes == [0x01, 0xd8]));
        assert!(image.windows(3).any(|bytes| bytes == [0x0f, 0x96, 0xc0]));
        assert!(
            image
                .windows(32)
                .any(|bytes| bytes == b"freestanding arithmetic failure\0")
        );

        let unsupported = check_source("fn main() { var wide: u64 = 1 print(wide) }").unwrap();
        assert!(
            compile_x86_bios(&unsupported)
                .unwrap_err()
                .message
                .contains("support only")
        );
    }

    #[test]
    fn wider_signed_and_boolean_values_have_exact_checked_codegen() {
        let program = check_source(
            r#"
fn main() {
    var unsigned: u32 = 4000000000
    unsigned += 5
    var signed: i32 = -2000000000
    signed -= 100
    var signed_divisor: i32 = -2
    var signed_quotient: i32 = signed / signed_divisor
    var signed_product: i32 = signed_quotient * 2
    var unsigned_divisor: u32 = 5
    var unsigned_quotient: u32 = unsigned / unsigned_divisor
    var unsigned_product: u32 = unsigned_quotient * 5
    var ordered: bool = signed < -1 && unsigned > 3999999999
    print(unsigned)
    print(signed)
    print(ordered)
}
"#,
        )
        .unwrap();
        let image = compile_x86_bios(&program).unwrap();
        assert_eq!(&image[510..512], &[0x55, 0xaa]);
        assert!(image.windows(3).any(|bytes| bytes == [0x66, 0x01, 0xd8]));
        assert!(image.windows(3).any(|bytes| bytes == [0x66, 0x29, 0xd8]));
        assert!(image.windows(2).any(|bytes| bytes == [0x0f, 0x80]));
        assert!(
            image
                .windows(6)
                .any(|bytes| bytes == [0x66, 0x39, 0xd8, 0x0f, 0x9c, 0xc0])
        );
        assert!(image.windows(3).any(|bytes| bytes == [0x66, 0xf7, 0xfb]));
        assert!(image.windows(3).any(|bytes| bytes == [0x66, 0xf7, 0xeb]));
        assert!(image.windows(3).any(|bytes| bytes == [0x66, 0xf7, 0xf3]));
        assert!(image.windows(3).any(|bytes| bytes == [0x66, 0xf7, 0xe3]));
        assert!(
            image
                .windows(6)
                .any(|bytes| bytes == [0x66, 0x3d, 0, 0, 0, 0x80])
        );
        assert!(image.windows(5).any(|bytes| bytes == b"true\0"));
        assert!(image.windows(6).any(|bytes| bytes == b"false\0"));
    }

    #[test]
    fn encoding_and_multisector_capacity_fail_closed() {
        let program = check_source("fn main() { print(\"こんにちは\") }").unwrap();
        assert!(
            compile_x86_bios(&program)
                .unwrap_err()
                .message
                .contains("ASCII")
        );

        let source = format!("fn main() {{ print(\"{}\") }}", "x".repeat(474));
        let program = check_source(&source).unwrap();
        let image = compile_x86_bios(&program).unwrap();
        assert!(image.len() > BOOT_SECTOR_BYTES);
        assert_eq!(image.len() % BOOT_SECTOR_BYTES, 0);
        assert_eq!(&image[BOOT_PAYLOAD_BYTES..BOOT_SECTOR_BYTES], &[0x55, 0xaa]);
        assert_eq!(
            &image[BOOT_SECTOR_BYTES..BOOT_SECTOR_BYTES + 6],
            &[0xfa, 0xea, 0x06, 0x7e, 0, 0]
        );
        assert!(
            image[..BOOT_SECTOR_BYTES]
                .windows(2)
                .any(|bytes| bytes == [0xb4, 0x42])
        );
        let stage_sectors = ((image.len() / BOOT_SECTOR_BYTES) - 1) as u16;
        let mut packet = vec![0x10, 0x00];
        packet.extend_from_slice(&stage_sectors.to_le_bytes());
        packet.extend_from_slice(&STAGE_ORIGIN.to_le_bytes());
        packet.extend_from_slice(&0u16.to_le_bytes());
        packet.extend_from_slice(&1u64.to_le_bytes());
        assert!(
            image[..BOOT_SECTOR_BYTES]
                .windows(packet.len())
                .any(|bytes| bytes == packet)
        );

        let source = format!("fn main() {{ print(\"{}\") }}", "x".repeat(33_000));
        let program = check_source(&source).unwrap();
        assert!(
            compile_x86_bios(&program)
                .unwrap_err()
                .message
                .contains("safe real-mode limit")
        );
    }
}
