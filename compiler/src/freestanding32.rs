//! Direct 32-bit x86 protected-mode boot-image generation.

use crate::{
    ast::{
        AssignmentOperator, BinaryOperator, Block, Capability, Expr, Expression, Function, Program,
        Statement, UnaryOperator,
    },
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    freestanding::{
        BOOT_PAYLOAD_BYTES, BOOT_SECTOR_BYTES, MAX_STAGE_SECTORS, STAGE_ORIGIN, boot_loader,
        transactional_write,
    },
};
use std::collections::HashMap;
use std::{
    fs,
    path::{Path, PathBuf},
};

const BOOT_ORIGIN: u16 = 0x7c00;
const CODE_SELECTOR: u16 = 0x08;
const DATA_SELECTOR: u16 = 0x10;
const PROTECTED_STACK: u32 = 0x0009_0000;
const VGA_TEXT: u32 = 0x000b_8000;
const VGA_TEXT_END: u32 = VGA_TEXT + 80 * 25 * 2;
const LOCAL_ORIGIN: u32 = 0x0010_0000;
const MAX_LOCALS: usize = 128;
const MAX_LOCAL_BYTES: usize = 4096;
const IDT_ORIGIN: u32 = LOCAL_ORIGIN + MAX_LOCAL_BYTES as u32;
const IDT_EXCEPTION_ENTRIES: usize = 32;
const PAGE_DIRECTORY: u32 = 0x0010_2000;
const FIRST_PAGE_TABLE: u32 = 0x0010_3000;
const PAGE_BYTES: u32 = 4096;
const PAGE_TABLE_ENTRIES: u32 = 1024;
const STAGE_READ_ONLY_FIRST_PAGE: u32 = 7;
const STAGE_READ_ONLY_PAGES: u32 = 9;
const MAX_FUNCTIONS: usize = 256;
const STACK_FLOOR: u32 = 0x0008_0000;
const STACK_EXPRESSION_RESERVE: usize = 1024;

/// Builds the initial flat 32-bit protected-mode DISP profile directly from validated syntax.
pub fn build_x86_protected(program: &Program, source_path: &Path) -> Result<PathBuf, Diagnostic> {
    if !source_path.is_file()
        || source_path.extension().and_then(|value| value.to_str()) != Some("disp")
    {
        return Err(error_at(
            source_path,
            Span::point(1, 1),
            "the x86 protected-mode target requires one `.disp` source file",
        ));
    }
    let image = compile_x86_protected(program).map_err(|error| {
        if error.file.is_some() {
            error
        } else {
            error.with_file(source_path.display().to_string())
        }
    })?;
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            error_at(
                source_path,
                Span::point(1, 1),
                "the protected-mode source filename must be valid UTF-8",
            )
        })?;
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let build = parent.join("build");
    fs::create_dir_all(&build).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not create protected-mode build directory: {cause}"),
        )
    })?;
    let destination = build.join(format!("{stem}-x86-protected32.img"));
    transactional_write(&destination, &image).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not write protected-mode image safely: {cause}"),
        )
    })?;
    Ok(destination)
}

/// Compiles the initial protected-mode profile to a deterministic BIOS disk image.
pub fn compile_x86_protected(program: &Program) -> Result<Vec<u8>, Diagnostic> {
    let main = validate_profile(program)?;
    let direct = compile_at(program, BOOT_ORIGIN)?;
    if direct.len() <= BOOT_PAYLOAD_BYTES {
        let mut image = vec![0; BOOT_SECTOR_BYTES];
        image[..direct.len()].copy_from_slice(&direct);
        image[BOOT_PAYLOAD_BYTES..].copy_from_slice(&[0x55, 0xaa]);
        return Ok(image);
    }

    let stage = compile_at(program, STAGE_ORIGIN)?;
    let sectors = stage.len().div_ceil(BOOT_SECTOR_BYTES);
    if sectors > MAX_STAGE_SECTORS {
        return Err(profile_error(
            main.body.span,
            format!(
                "protected-mode stage needs {sectors} sectors but the safe real-mode limit is {MAX_STAGE_SECTORS}"
            ),
        )
        .with_help("reduce the program while the protected target remains below the real-mode load ceiling"));
    }
    let mut image = boot_loader(sectors, main.body.span)?;
    image.resize((sectors + 1) * BOOT_SECTOR_BYTES, 0);
    image[BOOT_SECTOR_BYTES..BOOT_SECTOR_BYTES + stage.len()].copy_from_slice(&stage);
    Ok(image)
}

fn compile_at(program: &Program, origin: u16) -> Result<Vec<u8>, Diagnostic> {
    let mut output = Vec::new();

    // Real-mode bootstrap: normalize segments, enable A20, load a flat GDT, and set CR0.PE.
    output.extend_from_slice(&[
        0xfa, // cli
        0x31, 0xc0, // xor ax,ax
        0x8e, 0xd8, // mov ds,ax
        0x8e, 0xc0, // mov es,ax
        0x8e, 0xd0, // mov ss,ax
        0xbc, 0x00, 0x7c, // mov sp,0x7c00
        0xe4, 0x92, // in al,0x92
        0x0c, 0x02, // or al,2
        0x24, 0xfe, // and al,0xfe
        0xe6, 0x92, // out 0x92,al
        0xbe, 0x00, 0x05, // mov si,0x0500
        0xbf, 0x10, 0x05, // mov di,0x0510 (ffff:0510 = 0x100500)
        0x8a, 0x04, 0x50, // mov al,[si]; push ax
        0xb8, 0xff, 0xff, 0x8e, 0xc0, // mov ax,0xffff; mov es,ax
        0x26, 0x8a, 0x05, 0x50, // mov al,es:[di]; push ax
        0xc6, 0x04, 0x00, // mov byte [si],0
        0x26, 0xc6, 0x05, 0xff, // mov byte es:[di],0xff
        0x80, 0x3c, 0xff, 0x0f, 0x95, 0xc3, // cmp byte [si],0xff; setne bl
        0x58, 0x26, 0x88, 0x05, // pop ax; mov es:[di],al
        0x58, 0x88, 0x04, // pop ax; mov [si],al
        0x31, 0xc0, 0x8e, 0xc0, // xor ax,ax; mov es,ax
        0x84, 0xdb, 0x75, 0x0f, // test bl,bl; jne A20 verified
        0xb0, b'A', 0xe6, 0xe9, // visible A20 failure
        0xb4, 0x0e, 0xbb, 0x07, 0x00, 0xcd, 0x10, 0xfa, 0xf4, 0xeb, 0xfd, // halt forever
        0x0f, 0x01, 0x16, // lgdt [absolute16]
    ]);
    let gdtr_operand = output.len();
    output.extend_from_slice(&[0, 0]);
    output.extend_from_slice(&[
        0x0f, 0x20, 0xc0, // mov eax,cr0
        0x66, 0x83, 0xc8, 0x01, // or eax,1
        0x0f, 0x22, 0xc0, // mov cr0,eax
        0xea, // far jump ptr16:16
    ]);
    let protected_entry_operand = output.len();
    output.extend_from_slice(&[0, 0]);
    output.extend_from_slice(&CODE_SELECTOR.to_le_bytes());

    let protected_entry = output.len();
    output.extend_from_slice(&[0x66, 0xb8]); // mov ax,data selector
    output.extend_from_slice(&DATA_SELECTOR.to_le_bytes());
    output.extend_from_slice(&[
        0x8e, 0xd8, // mov ds,ax
        0x8e, 0xc0, // mov es,ax
        0x8e, 0xd0, // mov ss,ax
        0x8e, 0xe0, // mov fs,ax
        0x8e, 0xe8, // mov gs,ax
        0xbc, // mov esp,protected stack
    ]);
    output.extend_from_slice(&PROTECTED_STACK.to_le_bytes());
    output.push(0xfc); // cld
    output.push(0xbf); // mov edi,VGA text memory
    output.extend_from_slice(&VGA_TEXT.to_le_bytes());

    output = Compiler32::new(output, program)?.compile(program, origin)?;

    while output.len() % 8 != 0 {
        output.push(0);
    }
    let gdt = output.len();
    output.extend_from_slice(&[
        0, 0, 0, 0, 0, 0, 0, 0, // null descriptor
        0xff, 0xff, 0, 0, 0, 0x9a, 0xcf, 0, // flat 4 GiB code, 32-bit
        0xff, 0xff, 0, 0, 0, 0x92, 0xcf, 0, // flat 4 GiB data, 32-bit stack
    ]);
    let gdtr = output.len();
    output.extend_from_slice(&(24u16 - 1).to_le_bytes());
    output.extend_from_slice(&(u32::from(origin) + gdt as u32).to_le_bytes());

    if let (Ok(gdtr_address), Ok(entry_address)) = (
        u16::try_from(u32::from(origin) + gdtr as u32),
        u16::try_from(u32::from(origin) + protected_entry as u32),
    ) {
        output[gdtr_operand..gdtr_operand + 2].copy_from_slice(&gdtr_address.to_le_bytes());
        output[protected_entry_operand..protected_entry_operand + 2]
            .copy_from_slice(&entry_address.to_le_bytes());
    }

    Ok(output)
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
            Self::U16 => 2,
            Self::U32 | Self::I32 | Self::Bool => 4,
        }
    }

    const fn numeric(self) -> bool {
        !matches!(self, Self::Bool)
    }

    const fn signed(self) -> bool {
        matches!(self, Self::I32)
    }
}

#[derive(Clone, Copy)]
struct Local {
    address: u32,
    kind: ValueKind,
}

#[derive(Clone, Copy)]
struct ArrayLocal {
    address: u32,
    element: ValueKind,
    length: usize,
}

impl ArrayLocal {
    fn element(self, index: usize) -> Local {
        Local {
            address: self.address + (index * self.element.bytes()) as u32,
            kind: self.element,
        }
    }
}

#[derive(Clone, Copy)]
enum LocalValue {
    Scalar(Local),
    Array(ArrayLocal),
}

#[derive(Clone)]
struct FunctionInfo {
    label: Label,
    parameters: Vec<(String, Local)>,
    frame_slots: Vec<Local>,
    return_kind: Option<ValueKind>,
}

#[derive(Clone, Copy)]
struct Label(usize);

struct Fixup32 {
    immediate: usize,
    label: Label,
    relative: bool,
}

struct Assembler32 {
    bytes: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup32>,
}

impl Assembler32 {
    fn with_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

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

    fn relative(&mut self, label: Label) {
        let immediate = self.bytes.len();
        self.emit(&[0; 4]);
        self.fixups.push(Fixup32 {
            immediate,
            label,
            relative: true,
        });
    }

    fn absolute(&mut self, label: Label) {
        let immediate = self.bytes.len();
        self.emit(&[0; 4]);
        self.fixups.push(Fixup32 {
            immediate,
            label,
            relative: false,
        });
    }

    fn jump(&mut self, label: Label) {
        self.emit(&[0xe9]);
        self.relative(label);
    }

    fn conditional_jump(&mut self, condition: u8, label: Label) {
        self.emit(&[0x0f, condition]);
        self.relative(label);
    }

    fn call(&mut self, label: Label) {
        self.emit(&[0xe8]);
        self.relative(label);
    }

    fn finish(mut self, origin: u16, span: Span) -> Result<Vec<u8>, Diagnostic> {
        for fixup in self.fixups {
            let target = self.labels[fixup.label.0]
                .ok_or_else(|| profile_error(span, "unbound protected-mode machine-code label"))?;
            let value = if fixup.relative {
                let displacement = target as i64 - (fixup.immediate + 4) as i64;
                i32::try_from(displacement).map_err(|_| {
                    profile_error(
                        span,
                        "protected-mode branch exceeds the 32-bit target range",
                    )
                })? as u32
            } else {
                u32::from(origin)
                    .checked_add(u32::try_from(target).map_err(|_| {
                        profile_error(
                            span,
                            "protected-mode image exceeds the 32-bit address space",
                        )
                    })?)
                    .ok_or_else(|| {
                        profile_error(
                            span,
                            "protected-mode image exceeds the 32-bit address space",
                        )
                    })?
            };
            self.bytes[fixup.immediate..fixup.immediate + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(self.bytes)
    }
}

#[derive(Clone, Copy)]
struct LoopContext {
    continue_target: Label,
    break_target: Label,
}

struct Compiler32 {
    assembler: Assembler32,
    scopes: Vec<HashMap<String, LocalValue>>,
    next_local: usize,
    local_bytes: usize,
    functions: HashMap<String, FunctionInfo>,
    preallocated_locals: HashMap<(String, Span), LocalValue>,
    current_function: String,
    current_return: Option<ValueKind>,
    current_is_main: bool,
    stack_guard_used: bool,
    bounds_guard_used: bool,
    device_io_depth: usize,
    data: Vec<(Label, Vec<u8>)>,
    loops: Vec<LoopContext>,
    emit_character: Label,
    print_string: Label,
    print_unsigned: Label,
    print_signed: Label,
    newline: Label,
    arithmetic_failure: Label,
    stack_failure: Label,
    bounds_failure: Label,
    exception_handler: Label,
    idtr: Label,
    halt: Label,
}

impl Compiler32 {
    fn new(bytes: Vec<u8>, program: &Program) -> Result<Self, Diagnostic> {
        let mut assembler = Assembler32::with_bytes(bytes);
        let emit_character = assembler.label();
        let print_string = assembler.label();
        let print_unsigned = assembler.label();
        let print_signed = assembler.label();
        let newline = assembler.label();
        let arithmetic_failure = assembler.label();
        let stack_failure = assembler.label();
        let bounds_failure = assembler.label();
        let exception_handler = assembler.label();
        let idtr = assembler.label();
        let halt = assembler.label();
        let mut compiler = Self {
            assembler,
            scopes: Vec::new(),
            next_local: 0,
            local_bytes: 0,
            functions: HashMap::new(),
            preallocated_locals: HashMap::new(),
            current_function: String::new(),
            current_return: None,
            current_is_main: true,
            stack_guard_used: false,
            bounds_guard_used: false,
            device_io_depth: 0,
            data: Vec::new(),
            loops: Vec::new(),
            emit_character,
            print_string,
            print_unsigned,
            print_signed,
            newline,
            arithmetic_failure,
            stack_failure,
            bounds_failure,
            exception_handler,
            idtr,
            halt,
        };
        let mut idtr_bytes = Vec::with_capacity(6);
        idtr_bytes.extend_from_slice(&((IDT_EXCEPTION_ENTRIES * 8 - 1) as u16).to_le_bytes());
        idtr_bytes.extend_from_slice(&IDT_ORIGIN.to_le_bytes());
        compiler.data.push((idtr, idtr_bytes));
        for function in &program.functions {
            let label = compiler.assembler.label();
            let mut parameters = Vec::new();
            let mut frame_slots = Vec::new();
            for parameter in &function.parameters {
                let kind = ValueKind::from_annotation(&parameter.ty.name).ok_or_else(|| {
                    profile_error(parameter.ty.span, "unsupported protected32 parameter type")
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

    fn compile(mut self, program: &Program, origin: u16) -> Result<Vec<u8>, Diagnostic> {
        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("protected32 main was validated");
        self.emit_idt_setup();
        self.emit_paging_setup();
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
        self.assembler.finish(origin, main.body.span)
    }

    fn compile_function(&mut self, function: &Function, main: bool) -> Result<(), Diagnostic> {
        let info = self
            .functions
            .get(&function.name)
            .expect("protected32 function was registered")
            .clone();
        self.assembler.bind(info.label);
        self.current_function.clone_from(&function.name);
        self.current_return = info.return_kind;
        self.current_is_main = main;
        self.loops.clear();
        self.scopes.push(
            info.parameters
                .into_iter()
                .map(|(name, local)| (name, LocalValue::Scalar(local)))
                .collect(),
        );
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
                name,
                annotation,
                value,
                ..
            } => {
                annotation.as_ref().ok_or_else(|| {
                    profile_error(
                        span,
                        "protected32 locals require an explicit `u8`, `u16`, `u32`, `i32`, or `bool` annotation",
                    )
                })?;
                let value = value.as_ref().ok_or_else(|| {
                    profile_error(span, "protected32 locals must be initialized when declared")
                })?;
                let local = self
                    .preallocated_locals
                    .get(&(self.current_function.clone(), span))
                    .copied()
                    .expect("protected32 local was preallocated");
                match local {
                    LocalValue::Scalar(local) => {
                        let actual = self.compile_expression(value, Some(local.kind))?;
                        self.require_kind(actual, local.kind, value.span)?;
                        self.store(local);
                    }
                    LocalValue::Array(array) => {
                        let Expression::Array(values) = &value.node else {
                            return Err(profile_error(
                                value.span,
                                "protected32 fixed arrays require an array-literal initializer",
                            ));
                        };
                        if values.len() != array.length {
                            return Err(profile_error(
                                value.span,
                                "protected32 fixed-array initializer length does not match its annotation",
                            ));
                        }
                        for (index, value) in values.iter().enumerate() {
                            let actual = self.compile_expression(value, Some(array.element))?;
                            self.require_kind(actual, array.element, value.span)?;
                            self.store(array.element(index));
                        }
                    }
                }
                self.scopes
                    .last_mut()
                    .expect("protected32 block scope exists")
                    .insert(name.clone(), local);
                Ok(())
            }
            Statement::Assignment {
                name,
                operator,
                value,
                ..
            } => {
                let local = self.lookup_scalar(name, span)?;
                let actual = self.compile_expression(value, Some(local.kind))?;
                self.require_kind(actual, local.kind, value.span)?;
                if *operator != AssignmentOperator::Assign {
                    if !local.kind.numeric() {
                        return Err(profile_error(
                            span,
                            "boolean compound assignment is unavailable",
                        ));
                    }
                    self.assembler.emit(&[0x89, 0xc3]); // mov ebx,eax
                    self.load(local);
                    let operation = match operator {
                        AssignmentOperator::Add => BinaryOperator::Add,
                        AssignmentOperator::Subtract => BinaryOperator::Subtract,
                        AssignmentOperator::Multiply => BinaryOperator::Multiply,
                        AssignmentOperator::Divide => BinaryOperator::Divide,
                        AssignmentOperator::Assign => unreachable!(),
                    };
                    self.emit_checked_arithmetic(operation, local.kind);
                }
                self.store(local);
                Ok(())
            }
            Statement::PlaceAssignment {
                target,
                operator,
                value,
            } => self.compile_place_assignment(target, *operator, value, span),
            Statement::Expression(expression) => self.compile_call_statement(expression),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let actual = self.compile_expression(condition, Some(ValueKind::Bool))?;
                self.require_kind(actual, ValueKind::Bool, condition.span)?;
                self.assembler.emit(&[0x85, 0xc0]);
                let alternate = self.assembler.label();
                let end = self.assembler.label();
                self.assembler.conditional_jump(0x84, alternate);
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
                let actual = self.compile_expression(condition, Some(ValueKind::Bool))?;
                self.require_kind(actual, ValueKind::Bool, condition.span)?;
                self.assembler.emit(&[0x85, 0xc0]);
                self.assembler.conditional_jump(0x84, end);
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
                    profile_error(span, "protected32 `break` requires an enclosing loop")
                })?;
                self.assembler.jump(context.break_target);
                Ok(())
            }
            Statement::Continue => {
                let context = self.loops.last().copied().ok_or_else(|| {
                    profile_error(span, "protected32 `continue` requires an enclosing loop")
                })?;
                self.assembler.jump(context.continue_target);
                Ok(())
            }
            Statement::Return(value) => self.compile_return(value.as_ref(), span),
            Statement::Unsafe { capabilities, body } => {
                let authorized = capabilities.as_ref().is_some_and(|capabilities| {
                    !capabilities.is_empty()
                        && capabilities
                            .iter()
                            .all(|use_| use_.capability == Capability::DeviceIo)
                });
                if !authorized {
                    return Err(profile_error(
                        span,
                        "protected32 unsafe regions require an explicit supported capability contract",
                    ));
                }
                self.device_io_depth += 1;
                let result = self.compile_block(body);
                self.device_io_depth -= 1;
                result
            }
            _ => Err(profile_error(
                span,
                "this statement is not yet available in the protected32 scalar profile",
            )),
        }
    }

    fn compile_return(&mut self, value: Option<&Expr>, span: Span) -> Result<(), Diagnostic> {
        if self.current_is_main {
            if value.is_some() {
                return Err(profile_error(
                    span,
                    "protected32 `main` cannot return a value",
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
                    "protected32 scalar function must return a value",
                ));
            }
            (None, Some(_)) => {
                return Err(profile_error(
                    span,
                    "protected32 `Unit` function cannot return a value",
                ));
            }
        }
        Ok(())
    }

    fn compile_call_statement(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let Expression::Call { callee, arguments } = &expression.node else {
            return Err(profile_error(
                expression.span,
                "protected32 expression statements must be `print(value)` calls",
            ));
        };
        if let Expression::FieldAccess { object, field, .. } = &callee.node
            && matches!(&object.node, Expression::Identifier(owner) if owner == "Port")
        {
            return self.compile_port_write(field, arguments, expression.span);
        }
        let Expression::Identifier(name) = &callee.node else {
            return Err(profile_error(
                expression.span,
                "protected32 calls require a direct function name",
            ));
        };
        if name != "print" {
            self.compile_user_call(name, arguments, None, expression.span)?;
            return Ok(());
        }
        if arguments.len() != 1 {
            return Err(profile_error(
                expression.span,
                "protected32 output requires one argument",
            ));
        }
        if let Expression::String(text) = &arguments[0].node {
            let label = self.add_string(text, arguments[0].span)?;
            self.assembler.emit(&[0xbe]); // mov esi,string
            self.assembler.absolute(label);
            self.assembler.call(self.print_string);
        } else {
            let kind = self.compile_expression(&arguments[0], None)?;
            match kind {
                ValueKind::U8 | ValueKind::U16 | ValueKind::U32 => {
                    self.assembler.call(self.print_unsigned)
                }
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
                let kind = expected.unwrap_or(ValueKind::U32);
                if !kind.numeric() {
                    return Err(profile_error(
                        expression.span,
                        "integer literal cannot initialize a protected32 boolean",
                    ));
                }
                let value = match kind {
                    ValueKind::U8 => u8::try_from(*value).map(u32::from).map_err(|_| {
                        profile_error(expression.span, "protected32 `u8` literal exceeds 255")
                    })?,
                    ValueKind::U16 => u16::try_from(*value).map(u32::from).map_err(|_| {
                        profile_error(expression.span, "protected32 `u16` literal exceeds 65535")
                    })?,
                    ValueKind::U32 => u32::try_from(*value).map_err(|_| {
                        profile_error(
                            expression.span,
                            "protected32 `u32` literal exceeds 4294967295",
                        )
                    })?,
                    ValueKind::I32 => {
                        i32::try_from(*value)
                            .map(|value| value as u32)
                            .map_err(|_| {
                                profile_error(
                                    expression.span,
                                    "positive protected32 `i32` literal exceeds 2147483647",
                                )
                            })?
                    }
                    ValueKind::Bool => unreachable!(),
                };
                self.assembler.emit(&[0xb8]);
                self.assembler.emit(&value.to_le_bytes());
                Ok(kind)
            }
            Expression::Bool(value) => {
                if let Some(expected) = expected {
                    self.require_kind(ValueKind::Bool, expected, expression.span)?;
                }
                self.assembler.emit(&[0xb8]);
                self.assembler.emit(&u32::from(*value).to_le_bytes());
                Ok(ValueKind::Bool)
            }
            Expression::Identifier(name) => {
                let local = self.lookup_scalar(name, expression.span)?;
                if let Some(expected) = expected {
                    self.require_kind(local.kind, expected, expression.span)?;
                }
                self.load(local);
                Ok(local.kind)
            }
            Expression::Index { object, index } => {
                let array = self.direct_array(object)?;
                self.compile_array_offset(array, index)?;
                self.load_indexed_eax(array);
                if let Some(expected) = expected {
                    self.require_kind(array.element, expected, expression.span)?;
                }
                Ok(array.element)
            }
            Expression::Unary {
                operator: UnaryOperator::Not,
                operand,
            } => {
                let actual = self.compile_expression(operand, Some(ValueKind::Bool))?;
                self.require_kind(actual, ValueKind::Bool, operand.span)?;
                self.assembler
                    .emit(&[0x85, 0xc0, 0x0f, 0x94, 0xc0, 0x0f, 0xb6, 0xc0]);
                Ok(ValueKind::Bool)
            }
            Expression::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => {
                if expected.is_some_and(|kind| kind != ValueKind::I32) {
                    return Err(profile_error(
                        expression.span,
                        "protected32 negation requires an `i32` value",
                    ));
                }
                if let Expression::Integer(value) = operand.node
                    && value == (i32::MAX as u128) + 1
                {
                    self.assembler.emit(&[0xb8]);
                    self.assembler.emit(&0x8000_0000u32.to_le_bytes());
                    return Ok(ValueKind::I32);
                }
                let actual = self.compile_expression(operand, Some(ValueKind::I32))?;
                self.require_kind(actual, ValueKind::I32, operand.span)?;
                self.assembler.emit(&[0xf7, 0xd8]);
                self.assembler
                    .conditional_jump(0x80, self.arithmetic_failure);
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
                let operand_expected = if matches!(
                    operator,
                    BinaryOperator::Equal
                        | BinaryOperator::NotEqual
                        | BinaryOperator::Less
                        | BinaryOperator::LessEqual
                        | BinaryOperator::Greater
                        | BinaryOperator::GreaterEqual
                ) {
                    None
                } else {
                    expected
                };
                let kind =
                    self.infer_binary_kind(left, right, operand_expected, expression.span)?;
                let left_kind = self.compile_expression(left, Some(kind))?;
                self.assembler.emit(&[0x50]);
                let right_kind = self.compile_expression(right, Some(kind))?;
                self.assembler.emit(&[0x89, 0xc3, 0x58]); // mov ebx,eax; pop eax
                self.emit_binary_operator(*operator, left_kind, right_kind, expression.span)
            }
            Expression::Call { callee, arguments } => {
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(owner) if owner == "Port")
                {
                    return self.compile_port_read(field, arguments, expected, expression.span);
                }
                let Expression::Identifier(name) = &callee.node else {
                    return Err(profile_error(
                        expression.span,
                        "protected32 calls require a direct function name",
                    ));
                };
                if name == "print" {
                    return Err(profile_error(
                        expression.span,
                        "`print` returns `Unit` and cannot be used as a value",
                    ));
                }
                if let Some(kind) = ValueKind::from_annotation(name) {
                    if arguments.len() != 1 {
                        return Err(profile_error(
                            expression.span,
                            "protected32 exact scalar constructor requires one argument",
                        ));
                    }
                    if let Some(expected) = expected {
                        self.require_kind(kind, expected, expression.span)?;
                    }
                    let actual = self.compile_expression(&arguments[0], Some(kind))?;
                    self.require_kind(actual, kind, arguments[0].span)?;
                    return Ok(kind);
                }
                self.compile_user_call(name, arguments, expected, expression.span)?
                    .ok_or_else(|| {
                        profile_error(
                            expression.span,
                            format!("protected32 `Unit` function `{name}` has no value"),
                        )
                    })
            }
            _ => Err(profile_error(
                expression.span,
                "this expression is not yet available in the protected32 scalar profile",
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
            profile_error(span, format!("`{name}` is not a protected32 function"))
        })?;
        if name == "main" {
            return Err(profile_error(
                span,
                "protected32 `main` is an entry point and cannot be called",
            ));
        }
        if arguments.len() != info.parameters.len() {
            return Err(profile_error(
                span,
                format!(
                    "protected32 function `{name}` expects {} arguments but received {}",
                    info.parameters.len(),
                    arguments.len()
                ),
            ));
        }
        let frame_bytes = info.frame_slots.len() * 4;
        let argument_bytes = info.parameters.len() * 4;
        self.guard_stack(frame_bytes + argument_bytes + 4, span)?;
        for local in &info.frame_slots {
            self.load(*local);
            self.assembler.emit(&[0x50]);
        }
        for (argument, (_, parameter)) in arguments.iter().zip(&info.parameters) {
            let actual = self.compile_expression(argument, Some(parameter.kind))?;
            self.require_kind(actual, parameter.kind, argument.span)?;
            self.assembler.emit(&[0x50]);
        }
        for (_, parameter) in info.parameters.iter().rev() {
            self.assembler.emit(&[0x58]);
            self.store(*parameter);
        }
        self.assembler.call(info.label);
        if info.return_kind.is_some() {
            self.assembler.emit(&[0x89, 0xc1]); // mov ecx,eax
        }
        for local in info.frame_slots.iter().rev() {
            self.assembler.emit(&[0x58]);
            self.store(*local);
        }
        if info.return_kind.is_some() {
            self.assembler.emit(&[0x89, 0xc8]); // mov eax,ecx
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
        let actual = self.compile_expression(left, Some(ValueKind::Bool))?;
        self.require_kind(actual, ValueKind::Bool, left.span)?;
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
        let actual = self.compile_expression(right, Some(ValueKind::Bool))?;
        self.require_kind(actual, ValueKind::Bool, right.span)?;
        self.assembler.jump(end);
        self.assembler.bind(shortcut);
        self.assembler.emit(&[0xb8]);
        self.assembler
            .emit(&u32::from(operator == BinaryOperator::Or).to_le_bytes());
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
        let mut selected = expected;
        for hint in [self.expression_hint(left)?, self.expression_hint(right)?]
            .into_iter()
            .flatten()
        {
            if selected.is_some_and(|selected| selected != hint) {
                return Err(profile_error(
                    span,
                    "protected32 binary operands have different types",
                ));
            }
            selected = Some(hint);
        }
        Ok(selected.unwrap_or(ValueKind::U32))
    }

    fn expression_hint(&self, expression: &Expr) -> Result<Option<ValueKind>, Diagnostic> {
        match &expression.node {
            Expression::Identifier(name) => {
                Ok(Some(self.lookup_scalar(name, expression.span)?.kind))
            }
            Expression::Index { object, .. } => Ok(Some(self.direct_array(object)?.element)),
            Expression::Bool(_) => Ok(Some(ValueKind::Bool)),
            Expression::Integer(_) => Ok(None),
            Expression::Unary {
                operator: UnaryOperator::Not,
                ..
            } => Ok(Some(ValueKind::Bool)),
            Expression::Unary {
                operator: UnaryOperator::Negate,
                ..
            } => Ok(Some(ValueKind::I32)),
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
                        "protected32 expression mixes scalar types",
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
                "protected32 binary operands must have exactly the same type",
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
                self.emit_comparison(if operator == BinaryOperator::Equal {
                    0x94
                } else {
                    0x95
                });
                Ok(ValueKind::Bool)
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if !left.numeric() {
                    return Err(profile_error(
                        span,
                        "ordered boolean comparison is unavailable",
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
                self.emit_comparison(condition);
                Ok(ValueKind::Bool)
            }
            BinaryOperator::And | BinaryOperator::Or => unreachable!(),
        }
    }

    fn emit_checked_arithmetic(&mut self, operator: BinaryOperator, kind: ValueKind) {
        match kind {
            ValueKind::U8 => self.emit_checked_narrow(operator, u8::MAX.into()),
            ValueKind::U16 => self.emit_checked_narrow(operator, u16::MAX.into()),
            ValueKind::U32 => self.emit_checked_u32(operator),
            ValueKind::I32 => self.emit_checked_i32(operator),
            ValueKind::Bool => unreachable!(),
        }
    }

    fn emit_checked_narrow(&mut self, operator: BinaryOperator, maximum: u32) {
        match operator {
            BinaryOperator::Add => {
                self.assembler.emit(&[0x01, 0xd8, 0x3d]);
                self.assembler.emit(&maximum.to_le_bytes());
                self.assembler
                    .conditional_jump(0x87, self.arithmetic_failure); // ja
            }
            BinaryOperator::Subtract => {
                self.assembler.emit(&[0x29, 0xd8]);
                self.assembler
                    .conditional_jump(0x82, self.arithmetic_failure); // jb
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[0xf7, 0xe3, 0x3d]);
                self.assembler.emit(&maximum.to_le_bytes());
                self.assembler
                    .conditional_jump(0x87, self.arithmetic_failure); // ja
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.emit_checked_unsigned_division(operator);
            }
            _ => unreachable!(),
        }
    }

    fn emit_checked_u32(&mut self, operator: BinaryOperator) {
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
                self.assembler.emit(&[0xf7, 0xe3, 0x85, 0xd2]); // mul ebx; test edx,edx
                self.assembler
                    .conditional_jump(0x85, self.arithmetic_failure);
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.emit_checked_unsigned_division(operator);
            }
            _ => unreachable!(),
        }
    }

    fn emit_checked_unsigned_division(&mut self, operator: BinaryOperator) {
        self.assembler.emit(&[0x85, 0xdb]);
        self.assembler
            .conditional_jump(0x84, self.arithmetic_failure);
        self.assembler.emit(&[0x31, 0xd2, 0xf7, 0xf3]);
        if operator == BinaryOperator::Remainder {
            self.assembler.emit(&[0x89, 0xd0]);
        }
    }

    fn emit_checked_i32(&mut self, operator: BinaryOperator) {
        match operator {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                self.assembler.emit(if operator == BinaryOperator::Add {
                    &[0x01, 0xd8]
                } else {
                    &[0x29, 0xd8]
                });
                self.assembler
                    .conditional_jump(0x80, self.arithmetic_failure); // jo
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[
                    0xf7, 0xeb, // imul ebx => edx:eax
                    0x89, 0xc1, // mov ecx,eax
                    0xc1, 0xf9, 0x1f, // sar ecx,31
                    0x39, 0xca, // cmp edx,ecx
                ]);
                self.assembler
                    .conditional_jump(0x85, self.arithmetic_failure);
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler.emit(&[0x85, 0xdb]);
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                let safe = self.assembler.label();
                self.assembler.emit(&[0x3d]);
                self.assembler.emit(&0x8000_0000u32.to_le_bytes());
                self.assembler.conditional_jump(0x85, safe);
                self.assembler.emit(&[0x83, 0xfb, 0xff]); // cmp ebx,-1
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                self.assembler.bind(safe);
                self.assembler.emit(&[0x99, 0xf7, 0xfb]); // cdq; idiv ebx
                if operator == BinaryOperator::Remainder {
                    self.assembler.emit(&[0x89, 0xd0]);
                }
            }
            _ => unreachable!(),
        }
    }

    fn emit_comparison(&mut self, condition: u8) {
        self.assembler
            .emit(&[0x39, 0xd8, 0x0f, condition, 0xc0, 0x0f, 0xb6, 0xc0]);
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
        self.emit_character_routine();

        self.assembler.bind(self.print_string);
        let string_loop = self.assembler.label();
        let string_done = self.assembler.label();
        self.assembler.bind(string_loop);
        self.assembler.emit(&[0xac, 0x84, 0xc0]); // lodsb; test al,al
        self.assembler.conditional_jump(0x84, string_done);
        self.assembler.call(self.emit_character);
        self.assembler.jump(string_loop);
        self.assembler.bind(string_done);
        self.assembler.emit(&[0xc3]);

        self.assembler.bind(self.print_unsigned);
        let nonzero = self.assembler.label();
        let divide = self.assembler.label();
        let digits = self.assembler.label();
        self.assembler.emit(&[0x85, 0xc0]);
        self.assembler.conditional_jump(0x85, nonzero);
        self.assembler.emit(&[0xb0, b'0']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xc3]);
        self.assembler.bind(nonzero);
        self.assembler.emit(&[0x31, 0xc9, 0xbb, 10, 0, 0, 0]);
        self.assembler.bind(divide);
        self.assembler
            .emit(&[0x31, 0xd2, 0xf7, 0xf3, 0x52, 0x41, 0x85, 0xc0]);
        self.assembler.conditional_jump(0x85, divide);
        self.assembler.bind(digits);
        self.assembler.emit(&[0x58, 0x04, b'0']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0x49]);
        self.assembler.conditional_jump(0x85, digits);
        self.assembler.emit(&[0xc3]);

        self.assembler.bind(self.print_signed);
        let nonnegative = self.assembler.label();
        self.assembler.emit(&[0x85, 0xc0]);
        self.assembler.conditional_jump(0x89, nonnegative); // jns
        self.assembler.emit(&[0x50, 0xb0, b'-']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0x58, 0xf7, 0xd8]); // pop eax; neg eax
        self.assembler.bind(nonnegative);
        self.assembler.jump(self.print_unsigned);

        self.assembler.bind(self.newline);
        self.assembler.emit(&[0xb0, b'\r']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xb0, b'\n']);
        self.assembler.call(self.emit_character);
        self.assembler.emit(&[0xc3]);

        self.assembler.bind(self.arithmetic_failure);
        let failure = self.add_string_literal(b"protected32 arithmetic failure\0");
        self.assembler.emit(&[0xbe]);
        self.assembler.absolute(failure);
        self.assembler.call(self.print_string);
        self.assembler.call(self.newline);
        self.assembler.jump(self.halt);

        if self.stack_guard_used {
            self.assembler.bind(self.stack_failure);
            let failure = self.add_string_literal(b"protected32 stack limit exceeded\0");
            self.assembler.emit(&[0xbe]);
            self.assembler.absolute(failure);
            self.assembler.call(self.print_string);
            self.assembler.call(self.newline);
            self.assembler.jump(self.halt);
        }

        if self.bounds_guard_used {
            self.assembler.bind(self.bounds_failure);
            let failure = self.add_string_literal(b"protected32 index out of bounds\0");
            self.assembler.emit(&[0xbe]);
            self.assembler.absolute(failure);
            self.assembler.call(self.print_string);
            self.assembler.call(self.newline);
            self.assembler.jump(self.halt);
        }

        self.assembler.bind(self.exception_handler);
        self.assembler.emit(&[
            0xfa, // cli
            0x66, 0xb8, // mov ax,data selector
        ]);
        self.assembler.emit(&DATA_SELECTOR.to_le_bytes());
        self.assembler.emit(&[
            0x8e, 0xd8, // mov ds,ax
            0x8e, 0xc0, // mov es,ax
            0x8e, 0xd0, // mov ss,ax
            0xbc, // mov esp,known top
        ]);
        self.assembler.emit(&PROTECTED_STACK.to_le_bytes());
        self.assembler.emit(&[0xbf]); // mov edi,VGA
        self.assembler.emit(&VGA_TEXT.to_le_bytes());
        let failure = self.add_string_literal(b"protected32 CPU exception\0");
        self.assembler.emit(&[0xbe]);
        self.assembler.absolute(failure);
        self.assembler.call(self.print_string);
        self.assembler.call(self.newline);
        self.assembler.jump(self.halt);

        self.assembler.bind(self.halt);
        self.assembler.emit(&[0xfa, 0xf4]);
        self.assembler.jump(self.halt);
    }

    fn emit_idt_setup(&mut self) {
        self.assembler.emit(&[0xbf]); // mov edi,IDT_ORIGIN
        self.assembler.emit(&IDT_ORIGIN.to_le_bytes());
        self.assembler.emit(&[0xb9]); // mov ecx,entry count
        self.assembler
            .emit(&(IDT_EXCEPTION_ENTRIES as u32).to_le_bytes());
        self.assembler.emit(&[0xb8]); // mov eax,handler
        self.assembler.absolute(self.exception_handler);
        let next = self.assembler.label();
        self.assembler.bind(next);
        self.assembler.emit(&[
            0x66, 0x89, 0x07, // mov [edi],ax
            0x66, 0xc7, 0x47, 0x02, // mov word [edi+2],code selector
        ]);
        self.assembler.emit(&CODE_SELECTOR.to_le_bytes());
        self.assembler.emit(&[
            0xc6, 0x47, 0x04, 0, // reserved byte
            0xc6, 0x47, 0x05, 0x8e, // present 32-bit interrupt gate, DPL0
            0x89, 0xc2, // mov edx,eax
            0xc1, 0xea, 0x10, // shr edx,16
            0x66, 0x89, 0x57, 0x06, // mov [edi+6],dx
            0x83, 0xc7, 0x08, // add edi,8
            0x49, // dec ecx
        ]);
        self.assembler.conditional_jump(0x85, next); // jnz
        self.assembler.emit(&[0x0f, 0x01, 0x1d]); // lidt [absolute32]
        self.assembler.absolute(self.idtr);
        self.assembler.emit(&[0xbf]); // restore VGA cursor after IDT construction
        self.assembler.emit(&VGA_TEXT.to_le_bytes());
    }

    fn emit_paging_setup(&mut self) {
        // Clear one page directory and its first page table as one contiguous 8 KiB region.
        self.assembler.emit(&[0xbf]); // mov edi,PAGE_DIRECTORY
        self.assembler.emit(&PAGE_DIRECTORY.to_le_bytes());
        self.assembler.emit(&[0x31, 0xc0, 0xb9]); // xor eax,eax; mov ecx,2048 dwords
        self.assembler.emit(&(PAGE_TABLE_ENTRIES * 2).to_le_bytes());
        self.assembler.emit(&[0xf3, 0xab]); // rep stosd

        self.assembler.emit(&[0xc7, 0x05]); // mov dword [PAGE_DIRECTORY],table|P|RW
        self.assembler.emit(&PAGE_DIRECTORY.to_le_bytes());
        self.assembler.emit(&(FIRST_PAGE_TABLE | 0x3).to_le_bytes());

        // Leave PTE[0] absent and identity-map pages 1..1023 as supervisor read/write.
        self.assembler.emit(&[0xbf]);
        self.assembler
            .emit(&(FIRST_PAGE_TABLE + u32::BITS / 8).to_le_bytes());
        self.assembler.emit(&[0xb8]);
        self.assembler.emit(&(PAGE_BYTES | 0x3).to_le_bytes());
        self.assembler.emit(&[0xb9]);
        self.assembler.emit(&(PAGE_TABLE_ENTRIES - 1).to_le_bytes());
        let next = self.assembler.label();
        self.assembler.bind(next);
        self.assembler.emit(&[
            0x89, 0x07, // mov [edi],eax
            0x05, // add eax,PAGE_BYTES
        ]);
        self.assembler.emit(&PAGE_BYTES.to_le_bytes());
        self.assembler.emit(&[0x83, 0xc7, 0x04, 0x49]); // add edi,4; dec ecx
        self.assembler.conditional_jump(0x85, next);

        // Protect the complete possible loader/stage envelope (0x7000..0xffff) from writes.
        self.assembler.emit(&[0xbf]);
        self.assembler
            .emit(&(FIRST_PAGE_TABLE + STAGE_READ_ONLY_FIRST_PAGE * (u32::BITS / 8)).to_le_bytes());
        self.assembler.emit(&[0xb8]);
        self.assembler
            .emit(&((STAGE_READ_ONLY_FIRST_PAGE * PAGE_BYTES) | 0x1).to_le_bytes());
        self.assembler.emit(&[0xb9]);
        self.assembler.emit(&STAGE_READ_ONLY_PAGES.to_le_bytes());
        let protect = self.assembler.label();
        self.assembler.bind(protect);
        self.assembler.emit(&[0x89, 0x07, 0x05]); // mov [edi],eax; add eax,PAGE_BYTES
        self.assembler.emit(&PAGE_BYTES.to_le_bytes());
        self.assembler.emit(&[0x83, 0xc7, 0x04, 0x49]);
        self.assembler.conditional_jump(0x85, protect);

        self.assembler.emit(&[0xb8]); // mov eax,PAGE_DIRECTORY
        self.assembler.emit(&PAGE_DIRECTORY.to_le_bytes());
        self.assembler.emit(&[
            0x0f, 0x22, 0xd8, // mov cr3,eax
            0x0f, 0x20, 0xc0, // mov eax,cr0
            0x0d, // or eax,CR0.PG|CR0.WP
        ]);
        self.assembler.emit(&0x8001_0000u32.to_le_bytes());
        self.assembler.emit(&[0x0f, 0x22, 0xc0]); // mov cr0,eax
        let enabled = self.assembler.label();
        self.assembler.jump(enabled); // serialize instruction fetch after enabling paging
        self.assembler.bind(enabled);
        self.assembler.emit(&[0xbf]);
        self.assembler.emit(&VGA_TEXT.to_le_bytes());
    }

    fn emit_character_routine(&mut self) {
        self.assembler.bind(self.emit_character);
        let carriage = self.assembler.label();
        let linefeed = self.assembler.label();
        let check_wrap = self.assembler.label();
        let done = self.assembler.label();
        self.assembler.emit(&[0x53, 0x51, 0x52, 0xe6, 0xe9]); // save; out 0xe9,al
        self.assembler.emit(&[0x3c, b'\r']);
        self.assembler.conditional_jump(0x84, carriage);
        self.assembler.emit(&[0x3c, b'\n']);
        self.assembler.conditional_jump(0x84, linefeed);
        self.assembler
            .emit(&[0xb4, 0x07, 0x66, 0x89, 0x07, 0x83, 0xc7, 0x02]);
        self.assembler.jump(check_wrap);

        self.assembler.bind(carriage);
        self.emit_cursor_remainder();
        self.assembler.emit(&[0x29, 0xd7]); // sub edi,edx
        self.assembler.jump(done);

        self.assembler.bind(linefeed);
        self.emit_cursor_remainder();
        self.assembler.emit(&[0x29, 0xd7, 0x81, 0xc7]); // sub edi,edx; add edi,160
        self.assembler.emit(&160u32.to_le_bytes());

        self.assembler.bind(check_wrap);
        self.assembler.emit(&[0x81, 0xff]);
        self.assembler.emit(&VGA_TEXT_END.to_le_bytes());
        self.assembler.conditional_jump(0x82, done);
        self.assembler.emit(&[0xbf]);
        self.assembler.emit(&VGA_TEXT.to_le_bytes());
        self.assembler.bind(done);
        self.assembler.emit(&[0x5a, 0x59, 0x5b, 0xc3]);
    }

    fn emit_cursor_remainder(&mut self) {
        self.assembler.emit(&[0x89, 0xf8, 0x2d]); // mov eax,edi; sub eax,VGA
        self.assembler.emit(&VGA_TEXT.to_le_bytes());
        self.assembler.emit(&[0x31, 0xd2, 0xbb]); // xor edx,edx; mov ebx,160
        self.assembler.emit(&160u32.to_le_bytes());
        self.assembler.emit(&[0xf7, 0xf3]); // div ebx
    }

    fn add_string(&mut self, text: &str, span: Span) -> Result<Label, Diagnostic> {
        let mut bytes = Vec::new();
        for character in text.chars() {
            match character {
                '\n' => bytes.extend_from_slice(b"\r\n"),
                '\r' => bytes.push(b'\r'),
                ' '..='~' => bytes.push(character as u8),
                _ => {
                    return Err(profile_error(
                        span,
                        "protected32 output supports printable ASCII, newline, and carriage return only",
                    ));
                }
            }
        }
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
                format!("protected32 programs support at most {MAX_LOCALS} local storage slots"),
            ));
        }
        let aligned = (self.local_bytes + kind.bytes() - 1) & !(kind.bytes() - 1);
        let next = aligned + kind.bytes();
        if next > MAX_LOCAL_BYTES {
            return Err(profile_error(
                span,
                format!("protected32 local storage exceeds {MAX_LOCAL_BYTES} bytes"),
            ));
        }
        let address = LOCAL_ORIGIN + aligned as u32;
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
                            "protected32 locals require an explicit scalar annotation",
                        )
                    })?;
                    let local = if let Some((element, length)) = array_annotation(annotation)? {
                        let mut first = None;
                        for _ in 0..length {
                            let element_local = self.allocate_local(element, statement.span)?;
                            first.get_or_insert(element_local.address);
                            frame_slots.push(element_local);
                        }
                        LocalValue::Array(ArrayLocal {
                            address: first.unwrap_or(LOCAL_ORIGIN + self.local_bytes as u32),
                            element,
                            length,
                        })
                    } else {
                        let kind = ValueKind::from_annotation(&annotation.name).ok_or_else(|| {
                            profile_error(
                                annotation.span,
                                "protected32 locals support only exact scalars and bounded fixed arrays of exact scalars",
                            )
                        })?;
                        let local = self.allocate_local(kind, statement.span)?;
                        frame_slots.push(local);
                        LocalValue::Scalar(local)
                    };
                    self.preallocated_locals
                        .insert((function.to_owned(), statement.span), local);
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
                Statement::Unsafe { body, .. } => {
                    self.preallocate_block_locals(function, body, frame_slots)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn guard_stack(&mut self, required: usize, span: Span) -> Result<(), Diagnostic> {
        self.stack_guard_used = true;
        let threshold = usize::try_from(STACK_FLOOR)
            .expect("protected32 stack floor fits usize")
            .checked_add(STACK_EXPRESSION_RESERVE)
            .and_then(|value| value.checked_add(required))
            .ok_or_else(|| profile_error(span, "protected32 stack requirement overflowed"))?;
        if threshold >= usize::try_from(PROTECTED_STACK).expect("protected32 stack fits usize") {
            return Err(profile_error(
                span,
                "protected32 function frame cannot fit within the guarded stack",
            ));
        }
        self.assembler.emit(&[0x81, 0xfc]); // cmp esp,threshold
        self.assembler.emit(&(threshold as u32).to_le_bytes());
        self.assembler.conditional_jump(0x82, self.stack_failure);
        Ok(())
    }

    fn lookup_value(&self, name: &str, span: Span) -> Result<LocalValue, Diagnostic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| profile_error(span, format!("unknown protected32 local `{name}`")))
    }

    fn lookup_scalar(&self, name: &str, span: Span) -> Result<Local, Diagnostic> {
        match self.lookup_value(name, span)? {
            LocalValue::Scalar(local) => Ok(local),
            LocalValue::Array(_) => Err(profile_error(
                span,
                format!("protected32 fixed array `{name}` requires an index"),
            )),
        }
    }

    fn direct_array(&self, object: &Expr) -> Result<ArrayLocal, Diagnostic> {
        let Expression::Identifier(name) = &object.node else {
            return Err(profile_error(
                object.span,
                "protected32 indexing currently requires a direct fixed-array local",
            ));
        };
        match self.lookup_value(name, object.span)? {
            LocalValue::Array(array) => Ok(array),
            LocalValue::Scalar(_) => Err(profile_error(
                object.span,
                format!("protected32 scalar `{name}` cannot be indexed"),
            )),
        }
    }

    fn compile_array_offset(&mut self, array: ArrayLocal, index: &Expr) -> Result<(), Diagnostic> {
        let kind = self.compile_expression(index, None)?;
        if !kind.numeric() {
            return Err(profile_error(
                index.span,
                "protected32 array index must be an integer",
            ));
        }
        self.bounds_guard_used = true;
        self.assembler.emit(&[0x3d]); // cmp eax,length
        self.assembler.emit(&(array.length as u32).to_le_bytes());
        self.assembler.conditional_jump(0x83, self.bounds_failure); // jae
        match array.element.bytes() {
            1 => {}
            2 => self.assembler.emit(&[0xd1, 0xe0]), // shl eax,1
            4 => self.assembler.emit(&[0xc1, 0xe0, 0x02]), // shl eax,2
            _ => unreachable!(),
        }
        Ok(())
    }

    fn load_indexed_eax(&mut self, array: ArrayLocal) {
        self.assembler.emit(match array.element {
            ValueKind::U8 => &[0x0f, 0xb6, 0x80][..],
            ValueKind::U16 => &[0x0f, 0xb7, 0x80][..],
            ValueKind::U32 | ValueKind::I32 | ValueKind::Bool => &[0x8b, 0x80][..],
        });
        self.assembler.emit(&array.address.to_le_bytes());
    }

    fn load_indexed_ecx(&mut self, array: ArrayLocal) {
        self.assembler.emit(match array.element {
            ValueKind::U8 => &[0x0f, 0xb6, 0x81][..],
            ValueKind::U16 => &[0x0f, 0xb7, 0x81][..],
            ValueKind::U32 | ValueKind::I32 | ValueKind::Bool => &[0x8b, 0x81][..],
        });
        self.assembler.emit(&array.address.to_le_bytes());
    }

    fn store_indexed_ecx_from_ebx(&mut self, array: ArrayLocal) {
        self.assembler.emit(match array.element {
            ValueKind::U8 => &[0x88, 0x99][..],
            ValueKind::U16 => &[0x66, 0x89, 0x99][..],
            ValueKind::U32 | ValueKind::I32 | ValueKind::Bool => &[0x89, 0x99][..],
        });
        self.assembler.emit(&array.address.to_le_bytes());
    }

    fn compile_place_assignment(
        &mut self,
        target: &Expr,
        operator: AssignmentOperator,
        value: &Expr,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let Expression::Index { object, index } = &target.node else {
            return Err(profile_error(
                target.span,
                "protected32 place assignment requires a fixed-array element",
            ));
        };
        let array = self.direct_array(object)?;
        self.compile_array_offset(array, index)?;
        self.assembler.emit(&[0x50]); // preserve checked byte offset
        let actual = self.compile_expression(value, Some(array.element))?;
        self.require_kind(actual, array.element, value.span)?;
        self.assembler.emit(&[0x89, 0xc3, 0x59]); // value -> ebx; offset -> ecx
        if operator != AssignmentOperator::Assign {
            if !array.element.numeric() {
                return Err(profile_error(
                    span,
                    "boolean compound assignment is unavailable",
                ));
            }
            self.load_indexed_ecx(array);
            self.assembler.emit(&[0x51]); // arithmetic may use ecx
            let operation = match operator {
                AssignmentOperator::Add => BinaryOperator::Add,
                AssignmentOperator::Subtract => BinaryOperator::Subtract,
                AssignmentOperator::Multiply => BinaryOperator::Multiply,
                AssignmentOperator::Divide => BinaryOperator::Divide,
                AssignmentOperator::Assign => unreachable!(),
            };
            self.emit_checked_arithmetic(operation, array.element);
            self.assembler.emit(&[0x59]);
            self.assembler.emit(&[0x89, 0xc3]); // result -> ebx
        }
        self.store_indexed_ecx_from_ebx(array);
        Ok(())
    }

    fn require_device_io(&self, span: Span) -> Result<(), Diagnostic> {
        if self.device_io_depth == 0 {
            Err(profile_error(
                span,
                "protected32 hardware port access requires `unsafe uses DeviceIo { ... }`",
            ))
        } else {
            Ok(())
        }
    }

    fn compile_port_read(
        &mut self,
        field: &str,
        arguments: &[Expr],
        expected: Option<ValueKind>,
        span: Span,
    ) -> Result<ValueKind, Diagnostic> {
        self.require_device_io(span)?;
        if field != "read_u8" || arguments.len() != 1 {
            return Err(profile_error(
                span,
                format!(
                    "no protected32 hardware input `Port.{field}` with {} arguments",
                    arguments.len()
                ),
            ));
        }
        let actual = self.compile_expression(&arguments[0], Some(ValueKind::U16))?;
        self.require_kind(actual, ValueKind::U16, arguments[0].span)?;
        self.assembler.emit(&[0x89, 0xc2, 0xec, 0x0f, 0xb6, 0xc0]); // mov edx,eax; in al,dx; movzx eax,al
        if let Some(expected) = expected {
            self.require_kind(ValueKind::U8, expected, span)?;
        }
        Ok(ValueKind::U8)
    }

    fn compile_port_write(
        &mut self,
        field: &str,
        arguments: &[Expr],
        span: Span,
    ) -> Result<(), Diagnostic> {
        self.require_device_io(span)?;
        if field != "write_u8" || arguments.len() != 2 {
            return Err(profile_error(
                span,
                format!(
                    "no protected32 hardware output `Port.{field}` with {} arguments",
                    arguments.len()
                ),
            ));
        }
        let port = self.compile_expression(&arguments[0], Some(ValueKind::U16))?;
        self.require_kind(port, ValueKind::U16, arguments[0].span)?;
        self.assembler.emit(&[0x50]);
        let value = self.compile_expression(&arguments[1], Some(ValueKind::U8))?;
        self.require_kind(value, ValueKind::U8, arguments[1].span)?;
        self.assembler.emit(&[0x5a, 0xee]); // pop edx; out dx,al
        Ok(())
    }

    fn load(&mut self, local: Local) {
        match local.kind {
            ValueKind::U8 => {
                self.assembler.emit(&[0xa0]);
                self.assembler.emit(&local.address.to_le_bytes());
                self.assembler.emit(&[0x0f, 0xb6, 0xc0]);
            }
            ValueKind::U16 => {
                self.assembler.emit(&[0x66, 0xa1]);
                self.assembler.emit(&local.address.to_le_bytes());
                self.assembler.emit(&[0x0f, 0xb7, 0xc0]);
            }
            ValueKind::U32 | ValueKind::I32 | ValueKind::Bool => {
                self.assembler.emit(&[0xa1]);
                self.assembler.emit(&local.address.to_le_bytes());
            }
        }
    }

    fn store(&mut self, local: Local) {
        self.assembler.emit(match local.kind {
            ValueKind::U8 => &[0xa2][..],
            ValueKind::U16 => &[0x66, 0xa3][..],
            ValueKind::U32 | ValueKind::I32 | ValueKind::Bool => &[0xa3][..],
        });
        self.assembler.emit(&local.address.to_le_bytes());
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
                "protected32 expression has the wrong scalar type in this position",
            ))
        }
    }
}

fn array_annotation(
    annotation: &crate::ast::TypeName,
) -> Result<Option<(ValueKind, usize)>, Diagnostic> {
    let Some(length) = annotation
        .name
        .strip_prefix("[;")
        .and_then(|name| name.strip_suffix(']'))
    else {
        return Ok(None);
    };
    if annotation.arguments.len() != 1 {
        return Err(profile_error(
            annotation.span,
            "protected32 fixed arrays require exactly one element type",
        ));
    }
    let length = length
        .parse::<usize>()
        .map_err(|_| profile_error(annotation.span, "invalid protected32 fixed-array length"))?;
    let element = ValueKind::from_annotation(&annotation.arguments[0].name).ok_or_else(|| {
        profile_error(
            annotation.arguments[0].span,
            "protected32 fixed arrays support only `u8`, `u16`, `u32`, `i32`, and `bool` elements",
        )
    })?;
    Ok(Some((element, length)))
}

fn validate_profile(program: &Program) -> Result<&crate::ast::Function, Diagnostic> {
    if program.module.is_some()
        || !program.imports.is_empty()
        || !program.public_items.is_empty()
        || !program.structs.is_empty()
        || !program.enums.is_empty()
        || !program.traits.is_empty()
        || !program.implementations.is_empty()
    {
        return Err(profile_error(
            Span::point(1, 1),
            "protected32 does not yet accept modules, imports, user types, traits, or implementations",
        ));
    }
    if program.functions.len() > MAX_FUNCTIONS {
        return Err(profile_error(
            program
                .functions
                .get(MAX_FUNCTIONS)
                .map_or(Span::point(1, 1), |function| function.name_span),
            format!("protected32 programs support at most {MAX_FUNCTIONS} functions"),
        ));
    }
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| {
            profile_error(
                Span::point(1, 1),
                "protected32 requires a plain `fn main()` entry function",
            )
        })?;
    if main.name != "main"
        || !main.parameters.is_empty()
        || main.return_type.is_some()
        || main.asynchronous
        || !main.generics.is_empty()
        || main.external.is_some()
    {
        return Err(profile_error(
            main.span,
            "protected32 requires synchronous non-generic `fn main()` with no parameters or return type",
        ));
    }
    for function in &program.functions {
        if function.name == "print" {
            return Err(profile_error(
                function.name_span,
                "`print` is reserved by the protected32 output ABI",
            ));
        }
        let unsupported_capability = function.capabilities.as_ref().is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|use_| use_.capability != Capability::DeviceIo)
        });
        if function.asynchronous
            || !function.generics.is_empty()
            || unsupported_capability
            || function.external.is_some()
        {
            return Err(profile_error(
                function.span,
                "protected32 functions cannot be async, generic, external, or carry capabilities other than `DeviceIo`",
            ));
        }
        for parameter in &function.parameters {
            if ValueKind::from_annotation(&parameter.ty.name).is_none() {
                return Err(profile_error(
                    parameter.ty.span,
                    "protected32 parameters support only `u8`, `u16`, `u32`, `i32`, and `bool`",
                ));
            }
        }
        if let Some(return_type) = &function.return_type
            && return_type.name != "Unit"
            && ValueKind::from_annotation(&return_type.name).is_none()
        {
            return Err(profile_error(
                return_type.span,
                "protected32 returns support only exact scalar values and `Unit`",
            ));
        }
    }
    Ok(main)
}

fn profile_error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span)
        .with_help("use `--freestanding` for the wider real-mode profile while protected-mode lowering expands")
}

fn error_at(path: &Path, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span).with_file(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_source;

    #[test]
    fn protected32_bootstrap_is_flat_deterministic_and_fail_closed() {
        let program = check_source("fn main() { print(\"DISP protected\") }").unwrap();
        let first = compile_x86_protected(&program).unwrap();
        let second = compile_x86_protected(&program).unwrap();
        assert_eq!(first, second);
        assert!(first.len() > BOOT_SECTOR_BYTES);
        assert_eq!(first.len() % BOOT_SECTOR_BYTES, 0);
        assert_eq!(&first[BOOT_PAYLOAD_BYTES..BOOT_SECTOR_BYTES], &[0x55, 0xaa]);
        let stage = &first[BOOT_SECTOR_BYTES..];
        let lgdt = stage
            .windows(3)
            .position(|bytes| bytes == [0x0f, 0x01, 0x16])
            .unwrap();
        let gdtr_address = u16::from_le_bytes([stage[lgdt + 3], stage[lgdt + 4]]);
        let gdtr = usize::from(gdtr_address) - STAGE_ORIGIN as usize;
        assert_eq!(u16::from_le_bytes([stage[gdtr], stage[gdtr + 1]]), 23);
        let gdt_address = u32::from_le_bytes([
            stage[gdtr + 2],
            stage[gdtr + 3],
            stage[gdtr + 4],
            stage[gdtr + 5],
        ]);
        let gdt = (gdt_address - u32::from(STAGE_ORIGIN)) as usize;
        assert_eq!(gdt + 24, gdtr);
        assert!(stage.windows(3).any(|bytes| bytes == [0x0f, 0x20, 0xc0]));
        assert!(stage.windows(3).any(|bytes| bytes == [0x0f, 0x22, 0xc0]));
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0x80, 0x3c, 0xff, 0x0f, 0x95, 0xc3])
        ); // restored low/high alias probe must verify A20
        assert!(
            stage
                .windows(8)
                .any(|bytes| bytes == [0xff, 0xff, 0, 0, 0, 0x9a, 0xcf, 0])
        );
        assert!(stage.windows(5).any(|bytes| bytes == [0xbc, 0, 0, 9, 0]));
        assert!(
            stage
                .windows(5)
                .any(|bytes| bytes == [0xbf, 0, 0x80, 0x0b, 0])
        );
        assert!(stage.windows(2).any(|bytes| bytes == [0xe6, 0xe9]));
        assert!(stage.windows(15).any(|bytes| bytes == b"DISP protected\0"));

        let unsupported = check_source("fn main() { var value: u64 = 1 print(value) }").unwrap();
        let error = compile_x86_protected(&unsupported).unwrap_err();
        assert!(error.message.contains("support only"));

        let multisector =
            check_source(&format!("fn main() {{ print(\"{}\") }}", "x".repeat(600))).unwrap();
        let first = compile_x86_protected(&multisector).unwrap();
        let second = compile_x86_protected(&multisector).unwrap();
        assert_eq!(first, second);
        assert!(first.len() > BOOT_SECTOR_BYTES);
        assert_eq!(first.len() % BOOT_SECTOR_BYTES, 0);
        assert_eq!(&first[BOOT_PAYLOAD_BYTES..BOOT_SECTOR_BYTES], &[0x55, 0xaa]);
        assert_eq!(
            &first[BOOT_SECTOR_BYTES..BOOT_SECTOR_BYTES + 3],
            &[0xfa, 0x31, 0xc0]
        );
        assert!(
            first[..BOOT_SECTOR_BYTES]
                .windows(2)
                .any(|bytes| bytes == [0xb4, 0x42])
        );
        let stage = &first[BOOT_SECTOR_BYTES..];
        let lgdt = stage
            .windows(3)
            .position(|bytes| bytes == [0x0f, 0x01, 0x16])
            .unwrap();
        let gdtr_address = u16::from_le_bytes([stage[lgdt + 3], stage[lgdt + 4]]);
        assert!(gdtr_address >= STAGE_ORIGIN);

        let oversized = check_source(&format!(
            "fn main() {{ print(\"{}\") }}",
            "x".repeat(33_000)
        ))
        .unwrap();
        let error = compile_x86_protected(&oversized).unwrap_err();
        assert!(error.message.contains("safe real-mode limit"));
    }

    #[test]
    fn protected32_installs_bounded_exception_gates_and_a_known_state_handler() {
        let program = check_source("fn main() { print(\"IDT ready\") }").unwrap();
        let first = compile_x86_protected(&program).unwrap();
        let second = compile_x86_protected(&program).unwrap();
        assert_eq!(first, second);
        assert!(first.len() > BOOT_SECTOR_BYTES);
        let stage = &first[BOOT_SECTOR_BYTES..];
        let lidt = stage
            .windows(3)
            .position(|bytes| bytes == [0x0f, 0x01, 0x1d])
            .unwrap();
        let idtr_address = u32::from_le_bytes([
            stage[lidt + 3],
            stage[lidt + 4],
            stage[lidt + 5],
            stage[lidt + 6],
        ]);
        let idtr = (idtr_address - u32::from(STAGE_ORIGIN)) as usize;
        assert_eq!(
            u16::from_le_bytes([stage[idtr], stage[idtr + 1]]),
            (IDT_EXCEPTION_ENTRIES * 8 - 1) as u16
        );
        assert_eq!(
            u32::from_le_bytes([
                stage[idtr + 2],
                stage[idtr + 3],
                stage[idtr + 4],
                stage[idtr + 5],
            ]),
            IDT_ORIGIN
        );
        assert_eq!(IDT_ORIGIN, LOCAL_ORIGIN + MAX_LOCAL_BYTES as u32);
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0xc6, 0x47, 0x05, 0x8e, 0x89, 0xc2])
        );
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0xc1, 0xea, 0x10, 0x66])
        );
        assert!(
            stage
                .windows(26)
                .any(|bytes| bytes == b"protected32 CPU exception\0")
        );
    }

    #[test]
    fn protected32_enables_bounded_identity_paging_with_an_unmapped_null_page() {
        let program = check_source("fn main() { print(\"paging ready\") }").unwrap();
        let first = compile_x86_protected(&program).unwrap();
        let second = compile_x86_protected(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];
        let lidt = stage
            .windows(3)
            .position(|bytes| bytes == [0x0f, 0x01, 0x1d])
            .unwrap();
        let cr3 = stage
            .windows(3)
            .position(|bytes| bytes == [0x0f, 0x22, 0xd8])
            .unwrap();
        let paging = cr3
            + stage[cr3..]
                .windows(3)
                .position(|bytes| bytes == [0x0f, 0x22, 0xc0])
                .unwrap();
        assert!(lidt < cr3 && cr3 < paging);
        assert!(stage.windows(2).any(|bytes| bytes == [0xf3, 0xab]));
        let mut directory_entry = vec![0xc7, 0x05];
        directory_entry.extend_from_slice(&PAGE_DIRECTORY.to_le_bytes());
        directory_entry.extend_from_slice(&(FIRST_PAGE_TABLE | 3).to_le_bytes());
        assert!(
            stage
                .windows(directory_entry.len())
                .any(|bytes| bytes == directory_entry)
        );
        let mut first_present = vec![0xbf];
        first_present.extend_from_slice(&(FIRST_PAGE_TABLE + 4).to_le_bytes());
        first_present.push(0xb8);
        first_present.extend_from_slice(&(PAGE_BYTES | 3).to_le_bytes());
        assert!(
            stage
                .windows(first_present.len())
                .any(|bytes| bytes == first_present)
        );
        let mut read_only_stage = vec![0xbf];
        read_only_stage
            .extend_from_slice(&(FIRST_PAGE_TABLE + STAGE_READ_ONLY_FIRST_PAGE * 4).to_le_bytes());
        read_only_stage.push(0xb8);
        read_only_stage
            .extend_from_slice(&((STAGE_READ_ONLY_FIRST_PAGE * PAGE_BYTES) | 1).to_le_bytes());
        assert!(
            stage
                .windows(read_only_stage.len())
                .any(|bytes| bytes == read_only_stage)
        );
        assert!(stage.windows(5).any(|bytes| bytes == [0x0d, 0, 0, 1, 0x80]));
        assert_eq!(PAGE_DIRECTORY % PAGE_BYTES, 0);
        assert_eq!(FIRST_PAGE_TABLE, PAGE_DIRECTORY + PAGE_BYTES);
        assert!(PAGE_DIRECTORY >= IDT_ORIGIN + (IDT_EXCEPTION_ENTRIES * 8) as u32);
    }

    #[test]
    fn protected32_u32_control_flow_is_checked_and_uses_memory_above_one_mibibyte() {
        let program = check_source(
            r#"
fn main() {
    var total: u32 = 0
    var next: u32 = 1
    while next <= 10 {
        total += next
        next += 1
    }
    var divisor: u32 = 5
    var quotient: u32 = total / divisor
    var product: u32 = quotient * divisor
    var ordered: bool = total == product && total > 50
    print(total)
    print(ordered)
    loop {
        if total == 55 { break }
        continue
    }
}
"#,
        )
        .unwrap();
        let image = compile_x86_protected(&program).unwrap();
        assert!(image.len() > BOOT_SECTOR_BYTES);
        let stage = &image[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(5).any(|bytes| bytes == [0xa3, 0, 0, 0x10, 0]));
        assert!(stage.windows(5).any(|bytes| bytes == [0xa3, 4, 0, 0x10, 0]));
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0x01, 0xd8, 0x0f, 0x82])
        );
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0xf7, 0xe3, 0x85, 0xd2])
        );
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0x31, 0xd2, 0xf7, 0xf3])
        );
        assert!(
            stage
                .windows(8)
                .any(|bytes| { bytes == [0x39, 0xd8, 0x0f, 0x96, 0xc0, 0x0f, 0xb6, 0xc0] })
        );
        assert!(stage.windows(5).any(|bytes| bytes == b"true\0"));
        assert!(
            stage
                .windows(31)
                .any(|bytes| { bytes == b"protected32 arithmetic failure\0" })
        );
    }

    #[test]
    fn protected32_exact_compact_and_signed_widths_fail_closed() {
        let program = check_source(
            r#"
fn main() {
    var byte: u8 = 250
    byte += 3
    var small: u16 = 65000
    small += 535
    var wide: u32 = 4000000000
    wide += 5
    var signed: i32 = -2000000000
    signed -= 100
    var divisor: i32 = -2
    var half: i32 = signed / divisor
    var product: i32 = half * 2
    var ordered: bool = signed < -1 && wide > 3999999999
    var minimum: i32 = -2147483648
    print(byte)
    print(small)
    print(wide)
    print(signed)
    print(half)
    print(product)
    print(ordered)
    print(minimum)
}
"#,
        )
        .unwrap();
        let image = compile_x86_protected(&program).unwrap();
        assert_eq!(image, compile_x86_protected(&program).unwrap());
        let stage = &image[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(5).any(|bytes| bytes == [0xa2, 0, 0, 0x10, 0]));
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0x66, 0xa3, 2, 0, 0x10, 0])
        );
        assert!(
            stage
                .windows(8)
                .any(|bytes| bytes == [0x01, 0xd8, 0x3d, 0xff, 0, 0, 0, 0x0f])
        );
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0x29, 0xd8, 0x0f, 0x80])
        );
        assert!(stage.windows(3).any(|bytes| bytes == [0x99, 0xf7, 0xfb]));
        assert!(stage.windows(2).any(|bytes| bytes == [0xf7, 0xeb]));
        assert!(
            stage
                .windows(8)
                .any(|bytes| bytes == [0x39, 0xd8, 0x0f, 0x9c, 0xc0, 0x0f, 0xb6, 0xc0])
        );
        assert!(stage.windows(5).any(|bytes| bytes == [0x3d, 0, 0, 0, 0x80]));
    }

    #[test]
    fn protected32_functions_preserve_nested_and_recursive_frames() {
        let program = check_source(
            r#"
fn add(left: u32, right: u32) -> u32 {
    return left + right
}
fn factorial(value: u16) -> u16 {
    if value <= 1 { return 1 }
    var previous: u16 = value - 1
    var partial: u16 = factorial(previous)
    return value * partial
}
fn even(value: u8) -> bool {
    if value == 0 { return true }
    return odd(value - 1)
}
fn odd(value: u8) -> bool {
    if value == 0 { return false }
    return even(value - 1)
}
fn signed_half(value: i32) -> i32 {
    return value / -2
}
fn main() {
    print(add(4000000000, add(2, 3)))
    print(factorial(6))
    print(even(10))
    print(signed_half(-2000000100))
}
"#,
        )
        .unwrap();
        let image = compile_x86_protected(&program).unwrap();
        let stage = &image[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(1).any(|bytes| bytes == [0xe8]));
        assert!(stage.windows(1).any(|bytes| bytes == [0xc3]));
        assert!(
            stage
                .windows(8)
                .any(|bytes| bytes == [0x81, 0xfc, 0x14, 0x04, 0x08, 0, 0x0f, 0x82])
        );
        assert!(stage.windows(2).any(|bytes| bytes == [0x89, 0xc1]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x89, 0xc8]));
        assert!(
            stage
                .windows(33)
                .any(|bytes| bytes == b"protected32 stack limit exceeded\0")
        );

        let calls_main = check_source("fn helper() { main() } fn main() { helper() }").unwrap();
        let error = compile_x86_protected(&calls_main).unwrap_err();
        assert!(error.message.contains("entry point and cannot be called"));
    }

    #[test]
    fn protected32_fixed_arrays_use_exact_storage_checked_indices_and_recursive_frames() {
        let program = check_source(
            r#"
fn rotate(seed: u8, depth: u8) -> u8 {
    var bytes: [u8; 4] = [seed, u8(2), u8(3), u8(4)]
    bytes[1] += 5
    if depth == 0 { return bytes[1] }
    return rotate(bytes[0], depth - 1)
}
fn main() {
    var words: [u16; 3] = [u16(1000), u16(2000), u16(3000)]
    var wide: [u32; 4] = [u32(10), u32(20), u32(30), u32(40)]
    var signs: [i32; 2] = [i32(-10), i32(20)]
    var flags: [bool; 2] = [false, true]
    var index: u32 = 2
    wide[index] += 5
    words[0] = 1111
    flags[0] = true
    print(words[1])
    print(wide[index])
    print(signs[0])
    print(flags[0])
    print(rotate(9, 2))
}
"#,
        )
        .unwrap();
        let image = compile_x86_protected(&program).unwrap();
        let stage = &image[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(3).any(|bytes| bytes == [0x0f, 0xb6, 0x80]));
        assert!(stage.windows(3).any(|bytes| bytes == [0x0f, 0xb7, 0x80]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x8b, 0x80]));
        assert!(stage.windows(2).any(|bytes| bytes == [0xd1, 0xe0]));
        assert!(stage.windows(3).any(|bytes| bytes == [0xc1, 0xe0, 2]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x88, 0x99]));
        assert!(stage.windows(3).any(|bytes| bytes == [0x66, 0x89, 0x99]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x89, 0x99]));
        assert!(
            stage
                .windows(32)
                .any(|bytes| bytes == b"protected32 index out of bounds\0")
        );
    }

    #[test]
    fn protected32_device_io_requires_explicit_authority_and_emits_exact_port_instructions() {
        let program = check_source(
            r#"
fn probe() -> u8 uses DeviceIo {
    var status: u8 = 0
    unsafe uses DeviceIo {
        status = Port.read_u8(u16(146))
        Port.write_u8(u16(233), u8(80))
    }
    return status
}
fn main() {
    var ignored: u8 = probe()
    print("ort I/O authorized")
}
"#,
        )
        .unwrap();
        let image = compile_x86_protected(&program).unwrap();
        assert_eq!(image, compile_x86_protected(&program).unwrap());
        let stage = &image[BOOT_SECTOR_BYTES..];
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0x89, 0xc2, 0xec, 0x0f, 0xb6, 0xc0])
        );
        assert!(stage.windows(2).any(|bytes| bytes == [0x5a, 0xee]));

        let outside =
            check_source("fn main() { var byte: u8 = Port.read_u8(u16(146)) }").unwrap_err();
        assert!(outside.message.contains("requires an `unsafe` block"));

        let implicit =
            check_source("fn main() { unsafe\n{ var byte: u8 = Port.read_u8(u16(146)) } }")
                .unwrap_err();
        assert!(implicit.message.contains("explicit `unsafe uses DeviceIo`"));

        let wrong_contract =
            check_source("fn main() { unsafe uses RawMemory { Port.write_u8(u16(233), u8(80)) } }")
                .unwrap_err();
        assert!(
            wrong_contract
                .message
                .contains("does not allow capability `DeviceIo`")
        );

        let wrong_width = check_source(
            "fn main() { unsafe uses DeviceIo { var byte: u8 = Port.read_u8(u32(146)) } }",
        )
        .unwrap_err();
        assert!(wrong_width.message.contains("hardware port number"));
    }
}
