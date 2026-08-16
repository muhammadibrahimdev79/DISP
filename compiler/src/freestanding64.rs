//! Direct x86-64 long-mode boot-image generation.

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
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const BOOT_ORIGIN: u16 = 0x7c00;
const CODE32_SELECTOR: u16 = 0x08;
const DATA_SELECTOR: u16 = 0x10;
const CODE64_SELECTOR: u16 = 0x18;
const LONG_STACK: u32 = 0x0009_0000;
const VGA_TEXT: u32 = 0x000b_8000;
const VGA_TEXT_END: u32 = VGA_TEXT + 80 * 25 * 2;
const PML4: u32 = 0x0010_0000;
const PDPT: u32 = 0x0010_1000;
const PAGE_DIRECTORY: u32 = 0x0010_2000;
const FIRST_PAGE_TABLE: u32 = 0x0010_3000;
const IDT_ORIGIN: u32 = 0x0010_4000;
const LOCALS_ORIGIN: u32 = 0x0010_5000;
const TIMER_TICKS: u32 = 0x0010_6000;
const LOCALS_BYTES: u32 = 4096;
const MAX_FUNCTIONS: usize = 256;
const STACK_FLOOR: u32 = 0x0008_0000;
const STACK_EXPRESSION_RESERVE: u32 = 4096;
const PAGE_BYTES: u32 = 4096;
const PAGE_ENTRIES: u32 = 512;
const EXCEPTION_VECTORS: u32 = 32;
const LEGACY_IRQ_VECTORS: u32 = 16;
const IDT_ENTRIES: usize = (EXCEPTION_VECTORS + LEGACY_IRQ_VECTORS) as usize;
const STAGE_READ_ONLY_FIRST_PAGE: u32 = 7;
const STAGE_READ_ONLY_PAGES: u32 = 9;
const PAGE_NX_HIGH: u32 = 0x8000_0000;

const _: () = {
    let executable_end = STAGE_READ_ONLY_FIRST_PAGE + STAGE_READ_ONLY_PAGES;
    assert!(LONG_STACK / PAGE_BYTES >= executable_end);
    assert!(IDT_ORIGIN / PAGE_BYTES >= executable_end);
    assert!(LOCALS_ORIGIN / PAGE_BYTES >= executable_end);
    assert!(TIMER_TICKS / PAGE_BYTES >= executable_end);
};

#[derive(Clone, Copy)]
struct LongIdtHandlers {
    exception: Label,
    invalid_opcode: Label,
    general_protection: Label,
    page_fault: Label,
    unexpected_irq: Label,
    timer: Label,
}

/// Builds a deterministic runtime-free x86-64 long-mode BIOS disk image.
pub fn build_x86_64(program: &Program, source_path: &Path) -> Result<PathBuf, Diagnostic> {
    if !source_path.is_file()
        || source_path.extension().and_then(|value| value.to_str()) != Some("disp")
    {
        return Err(error_at(
            source_path,
            Span::point(1, 1),
            "the x86-64 freestanding target requires one `.disp` source file",
        ));
    }
    let image = compile_x86_64(program).map_err(|error| {
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
                "the x86-64 source filename must be valid UTF-8",
            )
        })?;
    let build = source_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("build");
    fs::create_dir_all(&build).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not create x86-64 build directory: {cause}"),
        )
    })?;
    let destination = build.join(format!("{stem}-x86_64-long.img"));
    transactional_write(&destination, &image).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not write x86-64 image safely: {cause}"),
        )
    })?;
    Ok(destination)
}

/// Compiles the initial long-mode profile to a deterministic BIOS disk image.
pub fn compile_x86_64(program: &Program) -> Result<Vec<u8>, Diagnostic> {
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
                "x86-64 stage needs {sectors} sectors but the safe loader limit is {MAX_STAGE_SECTORS}"
            ),
        ));
    }
    let mut image = boot_loader(sectors, main.body.span)?;
    image.resize((sectors + 1) * BOOT_SECTOR_BYTES, 0);
    image[BOOT_SECTOR_BYTES..BOOT_SECTOR_BYTES + stage.len()].copy_from_slice(&stage);
    Ok(image)
}

fn compile_at(program: &Program, origin: u16) -> Result<Vec<u8>, Diagnostic> {
    let timer_enabled = program.functions.iter().any(|function| {
        function.capabilities.as_ref().is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|use_| use_.capability == Capability::Timer)
        })
    });
    let mut bytes = vec![
        0xfa, // cli
        0x31, 0xc0, // xor ax,ax
        0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, // ds/es/ss=0
        0xbc, 0x00, 0x7c, // stack
        0xe4, 0x92, 0x0c, 0x02, 0x24, 0xfe, 0xe6, 0x92, // request A20
        0xbe, 0x00, 0x05, // mov si,0x0500
        0xbf, 0x10, 0x05, // mov di,0x0510 (ffff:0510 = 0x100500)
        0x8a, 0x04, 0x50, // mov al,[si]; push ax
        0xb8, 0xff, 0xff, 0x8e, 0xc0, // mov ax,0xffff; mov es,ax
        0x26, 0x8a, 0x05, 0x50, // mov al,es:[di]; push ax
        0xc6, 0x04, 0x00, // low probe=0
        0x26, 0xc6, 0x05, 0xff, // high probe=0xff
        0x80, 0x3c, 0xff, 0x0f, 0x95, 0xc3, // setne bl when addresses do not alias
        0x58, 0x26, 0x88, 0x05, // restore high byte
        0x58, 0x88, 0x04, // restore low byte
        0x31, 0xc0, 0x8e, 0xc0, // restore es=0
        0x84, 0xdb, 0x75, 0x0f, // verified -> continue
        0xb0, b'A', 0xe6, 0xe9, // debug-visible A20 failure
        0xb4, 0x0e, 0xbb, 0x07, 0x00, 0xcd, 0x10, 0xfa, 0xf4, 0xeb, 0xfd, 0x0f, 0x01,
        0x16, // lgdt [absolute16]
    ];
    let gdtr_operand = bytes.len();
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&[
        0x0f, 0x20, 0xc0, // mov eax,cr0
        0x66, 0x83, 0xc8, 0x01, // or eax,PE
        0x0f, 0x22, 0xc0, 0xea, // mov cr0,eax; far jump16
    ]);
    let protected_operand = bytes.len();
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&CODE32_SELECTOR.to_le_bytes());

    let protected_entry = bytes.len();
    let mut assembler = Assembler::new(bytes);
    let long_entry = assembler.label();
    let unsupported = assembler.label();
    let emit_character = assembler.label();
    let print_string = assembler.label();
    let print_unsigned = assembler.label();
    let print_signed = assembler.label();
    let newline = assembler.label();
    let arithmetic_failure = assembler.label();
    let stack_failure = assembler.label();
    let bounds_failure = assembler.label();
    let exception_handler = assembler.label();
    let invalid_opcode_handler = assembler.label();
    let general_protection_handler = assembler.label();
    let page_fault_handler = assembler.label();
    let unexpected_irq_handler = assembler.label();
    let timer_handler = assembler.label();
    let halt = assembler.label();
    let idtr = assembler.label();

    assembler.emit(&[0x66, 0xb8]);
    assembler.emit(&DATA_SELECTOR.to_le_bytes());
    assembler.emit(&[
        0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, // flat data segments
        0xbc,
    ]);
    assembler.emit(&LONG_STACK.to_le_bytes());
    emit_long_mode_checks(&mut assembler, unsupported);
    emit_long_page_tables(&mut assembler);
    assembler.emit(&[0xea]); // far jump ptr16:32 to 64-bit code descriptor
    assembler.absolute(long_entry);
    assembler.emit(&CODE64_SELECTOR.to_le_bytes());

    assembler.bind(unsupported);
    assembler.emit(&[
        0xbf, 0, 0x80, 0x0b, 0, // mov edi,VGA
        0xb8, b'L', 0x07, 0, 0, // mov eax,0x074c
        0x66, 0x89, 0x07, // mov [edi],ax
        0xe6, 0xe9, // out 0xe9,al
        0xfa, 0xf4, 0xeb, 0xfd, // halt
    ]);

    assembler.bind(long_entry);
    assembler.emit(&[0xfa]); // keep external interrupts disabled for the bounded profile
    assembler.emit(&[0x66, 0xb8]);
    assembler.emit(&DATA_SELECTOR.to_le_bytes());
    assembler.emit(&[
        0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, 0x8e, 0xe0, 0x8e, 0xe8, // segments
        0xbc,
    ]);
    assembler.emit(&LONG_STACK.to_le_bytes());
    assembler.emit(&[0xfc, 0xbf]);
    assembler.emit(&VGA_TEXT.to_le_bytes());
    emit_long_idt(
        &mut assembler,
        LongIdtHandlers {
            exception: exception_handler,
            invalid_opcode: invalid_opcode_handler,
            general_protection: general_protection_handler,
            page_fault: page_fault_handler,
            unexpected_irq: unexpected_irq_handler,
            timer: timer_handler,
        },
        timer_enabled,
        idtr,
    );

    let mut data = Vec::new();
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("x86-64 main was validated");
    {
        let mut compiler = ScalarCompiler64::new(
            &mut assembler,
            &mut data,
            program,
            print_string,
            print_unsigned,
            print_signed,
            newline,
            arithmetic_failure,
            stack_failure,
            bounds_failure,
            halt,
        )?;
        compiler.compile_program(program)?;
    }
    assembler.jump(halt);

    emit_output_routines(
        &mut assembler,
        &mut data,
        emit_character,
        print_string,
        print_unsigned,
        print_signed,
        newline,
        arithmetic_failure,
        stack_failure,
        bounds_failure,
        halt,
    );
    assembler.bind(exception_handler);
    assembler.emit(&[0xfa, 0xbc]);
    assembler.emit(&LONG_STACK.to_le_bytes());
    assembler.emit(&[0xbf]);
    assembler.emit(&VGA_TEXT.to_le_bytes());
    let fault = add_data(&mut assembler, &mut data, b"x86-64 CPU exception\0");
    assembler.emit(&[0xbe]);
    assembler.absolute(fault);
    assembler.call(print_string);
    assembler.call(newline);
    assembler.jump(halt);

    for (handler, message) in [
        (
            invalid_opcode_handler,
            b"x86-64 invalid opcode\0".as_slice(),
        ),
        (
            general_protection_handler,
            b"x86-64 general protection\0".as_slice(),
        ),
        (page_fault_handler, b"x86-64 page fault\0".as_slice()),
    ] {
        assembler.bind(handler);
        assembler.emit(&[0xfa, 0xbc]);
        assembler.emit(&LONG_STACK.to_le_bytes());
        assembler.emit(&[0xbf]);
        assembler.emit(&VGA_TEXT.to_le_bytes());
        let fault = add_data(&mut assembler, &mut data, message);
        assembler.emit(&[0xbe]);
        assembler.absolute(fault);
        assembler.call(print_string);
        assembler.call(newline);
        assembler.jump(halt);
    }

    assembler.bind(unexpected_irq_handler);
    assembler.emit(&[0xfa, 0xbc]);
    assembler.emit(&LONG_STACK.to_le_bytes());
    assembler.emit(&[0xbf]);
    assembler.emit(&VGA_TEXT.to_le_bytes());
    assembler.emit(&[0xb0, 0x20, 0xe6, 0xa0, 0xe6, 0x20]);
    let fault = add_data(
        &mut assembler,
        &mut data,
        b"x86-64 unexpected hardware interrupt\0",
    );
    assembler.emit(&[0xbe]);
    assembler.absolute(fault);
    assembler.call(print_string);
    assembler.call(newline);
    assembler.jump(halt);

    assembler.bind(timer_handler);
    assembler.emit(&[0x50, 0xff, 0x04, 0x25]);
    assembler.emit(&TIMER_TICKS.to_le_bytes());
    assembler.emit(&[0xb0, 0x20, 0xe6, 0x20, 0x58, 0x48, 0xcf]);

    assembler.bind(halt);
    assembler.emit(&[0xfa, 0xf4]);
    assembler.jump(halt);

    let mut idtr_bytes = Vec::with_capacity(10);
    idtr_bytes.extend_from_slice(&((IDT_ENTRIES * 16 - 1) as u16).to_le_bytes());
    idtr_bytes.extend_from_slice(&(IDT_ORIGIN as u64).to_le_bytes());
    data.insert(0, (idtr, idtr_bytes));
    for (label, value) in data {
        assembler.bind(label);
        assembler.emit(&value);
    }
    let mut output = assembler.finish(origin, main.body.span)?;

    while output.len() % 8 != 0 {
        output.push(0);
    }
    let gdt = output.len();
    output.extend_from_slice(&[
        0, 0, 0, 0, 0, 0, 0, 0, // null
        0xff, 0xff, 0, 0, 0, 0x9a, 0xcf, 0, // 32-bit code
        0xff, 0xff, 0, 0, 0, 0x92, 0xcf, 0, // data
        0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0, // 64-bit code (L=1,D=0)
    ]);
    let gdtr = output.len();
    output.extend_from_slice(&(32u16 - 1).to_le_bytes());
    output.extend_from_slice(&(u32::from(origin) + gdt as u32).to_le_bytes());
    let gdtr_address = u16::try_from(u32::from(origin) + gdtr as u32)
        .map_err(|_| profile_error(main.body.span, "x86-64 GDTR escaped real-mode reach"))?;
    let entry_address =
        u16::try_from(u32::from(origin) + protected_entry as u32).map_err(|_| {
            profile_error(
                main.body.span,
                "x86-64 protected entry escaped real-mode reach",
            )
        })?;
    output[gdtr_operand..gdtr_operand + 2].copy_from_slice(&gdtr_address.to_le_bytes());
    output[protected_operand..protected_operand + 2].copy_from_slice(&entry_address.to_le_bytes());
    Ok(output)
}

fn emit_long_mode_checks(assembler: &mut Assembler, unsupported: Label) {
    assembler.emit(&[
        0x9c, 0x58, 0x89, 0xc1, // pushfd; pop eax; mov ecx,eax
        0x35, 0, 0, 0x20, 0, // xor eax,EFLAGS.ID
        0x50, 0x9d, 0x9c, 0x58, // push eax; popfd; pushfd; pop eax
        0x31, 0xc8, // xor eax,ecx
        0xf7, 0xc0, 0, 0, 0x20, 0, // test eax,EFLAGS.ID
    ]);
    assembler.conditional_jump(0x84, unsupported);
    assembler.emit(&[
        0x51, 0x9d, // restore original flags
        0xb8, 0, 0, 0, 0x80, // extended CPUID maximum
        0x0f, 0xa2, 0x3d, 1, 0, 0, 0x80, // cpuid; cmp eax,0x80000001
    ]);
    assembler.conditional_jump(0x82, unsupported);
    assembler.emit(&[
        0xb8, 1, 0, 0, 0x80, 0x0f, 0xa2, // extended feature CPUID
        0xf7, 0xc2, 0, 0, 0, 0x20, // test edx,LongMode
    ]);
    assembler.conditional_jump(0x84, unsupported);
    assembler.emit(&[0xf7, 0xc2, 0, 0, 0x10, 0]); // test edx,NX
    assembler.conditional_jump(0x84, unsupported);
}

fn emit_long_page_tables(assembler: &mut Assembler) {
    assembler.emit(&[0xbf]);
    assembler.emit(&PML4.to_le_bytes());
    assembler.emit(&[0x31, 0xc0, 0xb9]);
    assembler.emit(&(PAGE_ENTRIES * 8).to_le_bytes());
    assembler.emit(&[0xf3, 0xab]); // clear four contiguous pages
    for (slot, child) in [
        (PML4, PDPT),
        (PDPT, PAGE_DIRECTORY),
        (PAGE_DIRECTORY, FIRST_PAGE_TABLE),
    ] {
        assembler.emit(&[0xc7, 0x05]);
        assembler.emit(&slot.to_le_bytes());
        assembler.emit(&(child | 3).to_le_bytes());
    }
    assembler.emit(&[0xbf]);
    assembler.emit(&(FIRST_PAGE_TABLE + 8).to_le_bytes());
    assembler.emit(&[0xb8]);
    assembler.emit(&(PAGE_BYTES | 3).to_le_bytes());
    assembler.emit(&[0xb9]);
    assembler.emit(&(PAGE_ENTRIES - 1).to_le_bytes());
    let fill = assembler.label();
    assembler.bind(fill);
    assembler.emit(&[0x89, 0x07, 0xc7, 0x47, 0x04]);
    assembler.emit(&PAGE_NX_HIGH.to_le_bytes());
    assembler.emit(&[0x05]);
    assembler.emit(&PAGE_BYTES.to_le_bytes());
    assembler.emit(&[0x83, 0xc7, 0x08, 0x49]);
    assembler.conditional_jump(0x85, fill);

    assembler.emit(&[0xbf]);
    assembler.emit(&(FIRST_PAGE_TABLE + STAGE_READ_ONLY_FIRST_PAGE * 8).to_le_bytes());
    assembler.emit(&[0xb8]);
    assembler.emit(&((STAGE_READ_ONLY_FIRST_PAGE * PAGE_BYTES) | 1).to_le_bytes());
    assembler.emit(&[0xb9]);
    assembler.emit(&STAGE_READ_ONLY_PAGES.to_le_bytes());
    let protect = assembler.label();
    assembler.bind(protect);
    assembler.emit(&[0x89, 0x07, 0xc7, 0x47, 0x04, 0, 0, 0, 0, 0x05]);
    assembler.emit(&PAGE_BYTES.to_le_bytes());
    assembler.emit(&[0x83, 0xc7, 0x08, 0x49]);
    assembler.conditional_jump(0x85, protect);

    assembler.emit(&[
        0x0f, 0x20, 0xe0, 0x83, 0xc8, 0x20, 0x0f, 0x22, 0xe0, // CR4.PAE
        0xb8,
    ]);
    assembler.emit(&PML4.to_le_bytes());
    assembler.emit(&[
        0x0f, 0x22, 0xd8, // CR3
        0xb9, 0x80, 0, 0, 0xc0, // ECX=EFER MSR
        0x0f, 0x32, // rdmsr
        0x0d, 0, 9, 0, 0, // EFER.LME|NXE
        0x0f, 0x30, // wrmsr
        0x0f, 0x20, 0xc0, 0x0d, 0, 0, 1, 0x80, 0x0f, 0x22, 0xc0, // PG|WP
    ]);
}

fn emit_long_idt(
    assembler: &mut Assembler,
    handlers: LongIdtHandlers,
    timer_enabled: bool,
    idtr: Label,
) {
    emit_long_gate_block(assembler, 0, EXCEPTION_VECTORS, handlers.exception);
    emit_long_gate_block(
        assembler,
        EXCEPTION_VECTORS,
        LEGACY_IRQ_VECTORS,
        handlers.unexpected_irq,
    );
    emit_long_gate(assembler, 6, handlers.invalid_opcode);
    emit_long_gate(assembler, 13, handlers.general_protection);
    emit_long_gate(assembler, 14, handlers.page_fault);
    if timer_enabled {
        emit_long_gate(assembler, EXCEPTION_VECTORS, handlers.timer);
    }
    assembler.emit(&[0x0f, 0x01, 0x1c, 0x25]);
    assembler.absolute(idtr);
    emit_pic_configuration(assembler, timer_enabled);
    assembler.emit(&[0xbf]);
    assembler.emit(&VGA_TEXT.to_le_bytes());
}

fn emit_long_gate_block(assembler: &mut Assembler, first: u32, count: u32, handler: Label) {
    assembler.emit(&[0xbf]);
    assembler.emit(&(IDT_ORIGIN + first * 16).to_le_bytes());
    assembler.emit(&[0xb9]);
    assembler.emit(&count.to_le_bytes());
    assembler.emit(&[0xb8]);
    assembler.absolute(handler);
    let fill = assembler.label();
    assembler.bind(fill);
    assembler.emit(&[0x66, 0x89, 0x07, 0x66, 0xc7, 0x47, 0x02]);
    assembler.emit(&CODE64_SELECTOR.to_le_bytes());
    assembler.emit(&[
        0xc6, 0x47, 0x04, 0, 0xc6, 0x47, 0x05, 0x8e, 0x48, 0x89, 0xc2, 0x48, 0xc1, 0xea, 0x10,
        0x66, 0x89, 0x57, 0x06, 0x48, 0xc1, 0xea, 0x10, 0x89, 0x57, 0x08, 0xc7, 0x47, 0x0c, 0, 0,
        0, 0, 0x48, 0x83, 0xc7, 0x10, 0x48, 0xff, 0xc9,
    ]);
    assembler.conditional_jump(0x85, fill);
}

fn emit_long_gate(assembler: &mut Assembler, vector: u32, handler: Label) {
    assembler.emit(&[0xbf]);
    assembler.emit(&(IDT_ORIGIN + vector * 16).to_le_bytes());
    assembler.emit(&[0xb8]);
    assembler.absolute(handler);
    assembler.emit(&[0x66, 0x89, 0x07, 0x66, 0xc7, 0x47, 0x02]);
    assembler.emit(&CODE64_SELECTOR.to_le_bytes());
    assembler.emit(&[
        0xc6, 0x47, 0x04, 0, 0xc6, 0x47, 0x05, 0x8e, 0x48, 0x89, 0xc2, 0x48, 0xc1, 0xea, 0x10,
        0x66, 0x89, 0x57, 0x06, 0x48, 0xc1, 0xea, 0x10, 0x89, 0x57, 0x08, 0xc7, 0x47, 0x0c, 0, 0,
        0, 0,
    ]);
}

fn emit_pic_configuration(assembler: &mut Assembler, timer_enabled: bool) {
    for (port, value) in [
        (0x20, 0x11), // master ICW1: initialize, expect ICW4
        (0xa0, 0x11), // slave ICW1
        (0x21, 0x20), // master ICW2: vectors 32..39
        (0xa1, 0x28), // slave ICW2: vectors 40..47
        (0x21, 0x04), // master ICW3: slave on IRQ2
        (0xa1, 0x02), // slave ICW3: cascade identity 2
        (0x21, 0x01), // master ICW4: 8086 mode
        (0xa1, 0x01), // slave ICW4: 8086 mode
    ] {
        assembler.emit(&[0xb0, value, 0xe6, port, 0xe6, 0x80]);
    }
    if timer_enabled {
        assembler.emit(&[
            0xb0, 0x36, 0xe6, 0x43, 0xe6, 0x80, // PIT channel 0, low/high, mode 3
            0xb0, 0x9c, 0xe6, 0x40, 0xe6, 0x80, // divisor 11932 low
            0xb0, 0x2e, 0xe6, 0x40, 0xe6, 0x80, // divisor 11932 high (~100 Hz)
            0xc7, 0x04, 0x25,
        ]);
        assembler.emit(&TIMER_TICKS.to_le_bytes());
        assembler.emit(&[0, 0, 0, 0]);
    }
    for (port, value) in [
        (0x21, if timer_enabled { 0xfe } else { 0xff }),
        (0xa1, 0xff),
    ] {
        assembler.emit(&[0xb0, value, 0xe6, port, 0xe6, 0x80]);
    }
    if timer_enabled {
        assembler.emit(&[0xfb]);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScalarKind64 {
    U8,
    U16,
    U32,
    I32,
    Bool,
}

impl ScalarKind64 {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "i32" => Some(Self::I32),
            "bool" => Some(Self::Bool),
            _ => None,
        }
    }

    const fn bytes(self) -> u32 {
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

#[derive(Clone, Copy, Debug)]
struct ScalarLocal64 {
    address: u32,
    kind: ScalarKind64,
}

#[derive(Clone, Copy, Debug)]
struct ArrayLocal64 {
    address: u32,
    element: ScalarKind64,
    length: usize,
}

impl ArrayLocal64 {
    fn element(self, index: usize) -> ScalarLocal64 {
        ScalarLocal64 {
            address: self.address + index as u32 * self.element.bytes(),
            kind: self.element,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum LocalValue64 {
    Scalar(ScalarLocal64),
    Array(ArrayLocal64),
}

#[derive(Clone, Copy)]
struct Loop64 {
    continue_target: Label,
    break_target: Label,
}

#[derive(Clone)]
struct Function64 {
    label: Label,
    parameters: Vec<(String, ScalarLocal64)>,
    frame_slots: Vec<ScalarLocal64>,
    return_kind: Option<ScalarKind64>,
    timer_authority: bool,
}

struct ScalarCompiler64<'a> {
    assembler: &'a mut Assembler,
    data: &'a mut Vec<(Label, Vec<u8>)>,
    scopes: Vec<HashMap<String, LocalValue64>>,
    next_local: u32,
    functions: HashMap<String, Function64>,
    preallocated_locals: HashMap<(String, Span), LocalValue64>,
    current_function: String,
    current_return: Option<ScalarKind64>,
    current_is_main: bool,
    device_io_depth: usize,
    loops: Vec<Loop64>,
    print_string: Label,
    print_unsigned: Label,
    print_signed: Label,
    newline: Label,
    arithmetic_failure: Label,
    stack_failure: Label,
    bounds_failure: Label,
    halt: Label,
}

impl<'a> ScalarCompiler64<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        assembler: &'a mut Assembler,
        data: &'a mut Vec<(Label, Vec<u8>)>,
        program: &Program,
        print_string: Label,
        print_unsigned: Label,
        print_signed: Label,
        newline: Label,
        arithmetic_failure: Label,
        stack_failure: Label,
        bounds_failure: Label,
        halt: Label,
    ) -> Result<Self, Diagnostic> {
        let mut compiler = Self {
            assembler,
            data,
            scopes: Vec::new(),
            next_local: 0,
            functions: HashMap::new(),
            preallocated_locals: HashMap::new(),
            current_function: String::new(),
            current_return: None,
            current_is_main: true,
            device_io_depth: 0,
            loops: Vec::new(),
            print_string,
            print_unsigned,
            print_signed,
            newline,
            arithmetic_failure,
            stack_failure,
            bounds_failure,
            halt,
        };
        for function in &program.functions {
            let label = compiler.assembler.label();
            let mut parameters = Vec::new();
            let mut frame_slots = Vec::new();
            for parameter in &function.parameters {
                let kind = ScalarKind64::from_name(&parameter.ty.name).ok_or_else(|| {
                    profile_error(
                        parameter.ty.span,
                        "unsupported x86-64 scalar parameter type",
                    )
                })?;
                let local = compiler.allocate_local(kind, parameter.ty.span)?;
                parameters.push((parameter.name.clone(), local));
                frame_slots.push(local);
            }
            compiler.preallocate_block_locals(&function.name, &function.body, &mut frame_slots)?;
            let return_kind = function
                .return_type
                .as_ref()
                .and_then(|return_type| ScalarKind64::from_name(&return_type.name));
            let timer_authority = function.capabilities.as_ref().is_some_and(|capabilities| {
                capabilities
                    .iter()
                    .any(|use_| use_.capability == Capability::Timer)
            });
            compiler.functions.insert(
                function.name.clone(),
                Function64 {
                    label,
                    parameters,
                    frame_slots,
                    return_kind,
                    timer_authority,
                },
            );
        }
        Ok(compiler)
    }

    fn compile_program(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("x86-64 main was validated");
        self.compile_function(main, true)?;
        self.assembler.jump(self.halt);
        for function in &program.functions {
            if function.name != "main" {
                self.compile_function(function, false)?;
            }
        }
        Ok(())
    }

    fn compile_function(&mut self, function: &Function, main: bool) -> Result<(), Diagnostic> {
        let info = self
            .functions
            .get(&function.name)
            .expect("x86-64 function was registered")
            .clone();
        self.assembler.bind(info.label);
        self.current_function.clone_from(&function.name);
        self.current_return = info.return_kind;
        self.current_is_main = main;
        self.loops.clear();
        self.scopes.push(
            info.parameters
                .into_iter()
                .map(|(name, local)| (name, LocalValue64::Scalar(local)))
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
                        "x86-64 locals require an explicit `u8`, `u16`, `u32`, `i32`, or `bool` annotation",
                    )
                })?;
                let value = value.as_ref().ok_or_else(|| {
                    profile_error(span, "x86-64 locals must be initialized when declared")
                })?;
                let local = self
                    .preallocated_locals
                    .get(&(self.current_function.clone(), span))
                    .copied()
                    .expect("x86-64 local was preallocated");
                match local {
                    LocalValue64::Scalar(local) => {
                        let actual = self.compile_expression(value, Some(local.kind))?;
                        self.require_kind(actual, local.kind, value.span)?;
                        self.store(local);
                    }
                    LocalValue64::Array(array) => {
                        let Expression::Array(values) = &value.node else {
                            return Err(profile_error(
                                value.span,
                                "x86-64 fixed arrays require an array-literal initializer",
                            ));
                        };
                        if values.len() != array.length {
                            return Err(profile_error(
                                value.span,
                                "x86-64 fixed-array initializer length does not match its annotation",
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
                    .expect("x86-64 block scope exists")
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
                let actual = self.compile_expression(value, Some(local.kind))?;
                self.require_kind(actual, local.kind, value.span)?;
                if *operator != AssignmentOperator::Assign {
                    if !local.kind.numeric() {
                        return Err(profile_error(
                            span,
                            "boolean compound assignment is unavailable in x86-64",
                        ));
                    }
                    self.assembler.emit(&[0x89, 0xc3]);
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
                let actual = self.compile_expression(condition, Some(ScalarKind64::Bool))?;
                self.require_kind(actual, ScalarKind64::Bool, condition.span)?;
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
                let actual = self.compile_expression(condition, Some(ScalarKind64::Bool))?;
                self.require_kind(actual, ScalarKind64::Bool, condition.span)?;
                self.assembler.emit(&[0x85, 0xc0]);
                self.assembler.conditional_jump(0x84, end);
                self.loops.push(Loop64 {
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
                self.loops.push(Loop64 {
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
                    profile_error(span, "x86-64 `break` requires an enclosing loop")
                })?;
                self.assembler.jump(context.break_target);
                Ok(())
            }
            Statement::Continue => {
                let context = self.loops.last().copied().ok_or_else(|| {
                    profile_error(span, "x86-64 `continue` requires an enclosing loop")
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
                        "x86-64 unsafe regions require an explicit supported capability contract",
                    ));
                }
                self.device_io_depth += 1;
                let result = self.compile_block(body);
                self.device_io_depth -= 1;
                result
            }
            _ => Err(profile_error(
                span,
                "this statement is not yet available in the x86-64 scalar profile",
            )),
        }
    }

    fn compile_return(&mut self, value: Option<&Expr>, span: Span) -> Result<(), Diagnostic> {
        if self.current_is_main {
            if value.is_some() {
                return Err(profile_error(span, "x86-64 `main` cannot return a value"));
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
                    "x86-64 scalar function must return a value",
                ));
            }
            (None, Some(_)) => {
                return Err(profile_error(
                    span,
                    "x86-64 `Unit` function cannot return a value",
                ));
            }
        }
        Ok(())
    }

    fn compile_call_statement(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let Expression::Call { callee, arguments } = &expression.node else {
            return Err(profile_error(
                expression.span,
                "x86-64 expression statements must be `print(value)` calls",
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
                "x86-64 calls require a direct function name",
            ));
        };
        if name != "print" {
            self.compile_user_call(name, arguments, None, expression.span)?;
            return Ok(());
        }
        if arguments.len() != 1 {
            return Err(profile_error(
                expression.span,
                "x86-64 output requires one argument",
            ));
        }
        if let Expression::String(text) = &arguments[0].node {
            let label = self.add_string(text, arguments[0].span)?;
            self.assembler.emit(&[0xbe]);
            self.assembler.absolute(label);
            self.assembler.call(self.print_string);
        } else {
            let kind = self.compile_expression(&arguments[0], None)?;
            match kind {
                ScalarKind64::U8 | ScalarKind64::U16 | ScalarKind64::U32 => {
                    self.assembler.call(self.print_unsigned);
                }
                ScalarKind64::I32 => self.assembler.call(self.print_signed),
                ScalarKind64::Bool => self.emit_print_bool(),
            }
        }
        self.assembler.call(self.newline);
        Ok(())
    }

    fn compile_expression(
        &mut self,
        expression: &Expr,
        expected: Option<ScalarKind64>,
    ) -> Result<ScalarKind64, Diagnostic> {
        match &expression.node {
            Expression::Integer(value) => {
                let kind = expected.unwrap_or(ScalarKind64::U32);
                if !kind.numeric() {
                    return Err(profile_error(
                        expression.span,
                        "integer literal cannot initialize an x86-64 boolean",
                    ));
                }
                let value = match kind {
                    ScalarKind64::U8 => u8::try_from(*value).map(u32::from).map_err(|_| {
                        profile_error(expression.span, "x86-64 `u8` literal exceeds 255")
                    })?,
                    ScalarKind64::U16 => u16::try_from(*value).map(u32::from).map_err(|_| {
                        profile_error(expression.span, "x86-64 `u16` literal exceeds 65535")
                    })?,
                    ScalarKind64::U32 => u32::try_from(*value).map_err(|_| {
                        profile_error(expression.span, "x86-64 `u32` literal exceeds 4294967295")
                    })?,
                    ScalarKind64::I32 => {
                        i32::try_from(*value)
                            .map(|value| value as u32)
                            .map_err(|_| {
                                profile_error(
                                    expression.span,
                                    "positive x86-64 `i32` literal exceeds 2147483647",
                                )
                            })?
                    }
                    ScalarKind64::Bool => unreachable!(),
                };
                self.assembler.emit(&[0xb8]);
                self.assembler.emit(&value.to_le_bytes());
                Ok(kind)
            }
            Expression::Bool(value) => {
                if let Some(expected) = expected {
                    self.require_kind(ScalarKind64::Bool, expected, expression.span)?;
                }
                self.assembler.emit(&[0xb8]);
                self.assembler.emit(&u32::from(*value).to_le_bytes());
                Ok(ScalarKind64::Bool)
            }
            Expression::Identifier(name) => {
                let local = self.lookup(name, expression.span)?;
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
                let actual = self.compile_expression(operand, Some(ScalarKind64::Bool))?;
                self.require_kind(actual, ScalarKind64::Bool, operand.span)?;
                self.assembler
                    .emit(&[0x85, 0xc0, 0x0f, 0x94, 0xc0, 0x0f, 0xb6, 0xc0]);
                Ok(ScalarKind64::Bool)
            }
            Expression::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => {
                if expected.is_some_and(|kind| kind != ScalarKind64::I32) {
                    return Err(profile_error(
                        expression.span,
                        "x86-64 negation requires an `i32` value",
                    ));
                }
                if let Expression::Integer(value) = operand.node
                    && value == (i32::MAX as u128) + 1
                {
                    self.assembler.emit(&[0xb8]);
                    self.assembler.emit(&0x8000_0000u32.to_le_bytes());
                    return Ok(ScalarKind64::I32);
                }
                let actual = self.compile_expression(operand, Some(ScalarKind64::I32))?;
                self.require_kind(actual, ScalarKind64::I32, operand.span)?;
                self.assembler.emit(&[0xf7, 0xd8]);
                self.assembler
                    .conditional_jump(0x80, self.arithmetic_failure);
                Ok(ScalarKind64::I32)
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
                self.assembler.emit(&[0x89, 0xc3, 0x58]);
                self.emit_binary_operator(*operator, left_kind, right_kind, expression.span)
            }
            Expression::Call { callee, arguments } => {
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(owner) if owner == "Time")
                {
                    return self.compile_timer_ticks(field, arguments, expected, expression.span);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(owner) if owner == "Port")
                {
                    return self.compile_port_read(field, arguments, expected, expression.span);
                }
                let Expression::Identifier(name) = &callee.node else {
                    return Err(profile_error(
                        expression.span,
                        "x86-64 calls require a direct function name",
                    ));
                };
                if name == "print" {
                    return Err(profile_error(
                        expression.span,
                        "`print` returns `Unit` and cannot be used as a value",
                    ));
                }
                if let Some(kind) = ScalarKind64::from_name(name) {
                    if arguments.len() != 1 {
                        return Err(profile_error(
                            expression.span,
                            "x86-64 exact scalar constructors require one argument",
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
                            format!("x86-64 `Unit` function `{name}` has no value"),
                        )
                    })
            }
            _ => Err(profile_error(
                expression.span,
                "this expression is not yet available in the x86-64 scalar profile",
            )),
        }
    }

    fn compile_short_circuit(
        &mut self,
        left: &Expr,
        operator: BinaryOperator,
        right: &Expr,
    ) -> Result<ScalarKind64, Diagnostic> {
        let actual = self.compile_expression(left, Some(ScalarKind64::Bool))?;
        self.require_kind(actual, ScalarKind64::Bool, left.span)?;
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
        let actual = self.compile_expression(right, Some(ScalarKind64::Bool))?;
        self.require_kind(actual, ScalarKind64::Bool, right.span)?;
        self.assembler.jump(end);
        self.assembler.bind(shortcut);
        self.assembler.emit(&[0xb8]);
        self.assembler
            .emit(&u32::from(operator == BinaryOperator::Or).to_le_bytes());
        self.assembler.bind(end);
        Ok(ScalarKind64::Bool)
    }

    fn compile_user_call(
        &mut self,
        name: &str,
        arguments: &[Expr],
        expected: Option<ScalarKind64>,
        span: Span,
    ) -> Result<Option<ScalarKind64>, Diagnostic> {
        let info =
            self.functions.get(name).cloned().ok_or_else(|| {
                profile_error(span, format!("`{name}` is not an x86-64 function"))
            })?;
        if name == "main" {
            return Err(profile_error(
                span,
                "x86-64 `main` is an entry point and cannot be called",
            ));
        }
        if arguments.len() != info.parameters.len() {
            return Err(profile_error(
                span,
                format!(
                    "x86-64 function `{name}` expects {} arguments but received {}",
                    info.parameters.len(),
                    arguments.len()
                ),
            ));
        }
        let stack_words = info
            .frame_slots
            .len()
            .checked_add(info.parameters.len())
            .and_then(|words| words.checked_add(1))
            .ok_or_else(|| profile_error(span, "x86-64 function stack requirement overflowed"))?;
        let required = u32::try_from(stack_words)
            .ok()
            .and_then(|words| words.checked_mul(8))
            .ok_or_else(|| profile_error(span, "x86-64 function stack requirement overflowed"))?;
        self.guard_stack(required, span)?;
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
            self.assembler.emit(&[0x89, 0xc1]);
        }
        for local in info.frame_slots.iter().rev() {
            self.assembler.emit(&[0x58]);
            self.store(*local);
        }
        if info.return_kind.is_some() {
            self.assembler.emit(&[0x89, 0xc8]);
        }
        if let (Some(expected), Some(actual)) = (expected, info.return_kind) {
            self.require_kind(actual, expected, span)?;
        }
        Ok(info.return_kind)
    }

    fn guard_stack(&mut self, required: u32, span: Span) -> Result<(), Diagnostic> {
        let threshold = STACK_FLOOR
            .checked_add(STACK_EXPRESSION_RESERVE)
            .and_then(|value| value.checked_add(required))
            .ok_or_else(|| profile_error(span, "x86-64 stack guard threshold overflowed"))?;
        if threshold >= LONG_STACK {
            return Err(profile_error(
                span,
                "x86-64 function frame cannot fit within the guarded stack",
            ));
        }
        self.assembler.emit(&[0x48, 0x81, 0xfc]);
        self.assembler.emit(&threshold.to_le_bytes());
        self.assembler.conditional_jump(0x82, self.stack_failure);
        Ok(())
    }

    fn infer_binary_kind(
        &self,
        left: &Expr,
        right: &Expr,
        expected: Option<ScalarKind64>,
        span: Span,
    ) -> Result<ScalarKind64, Diagnostic> {
        let mut selected = expected;
        for hint in [self.expression_hint(left)?, self.expression_hint(right)?]
            .into_iter()
            .flatten()
        {
            if selected.is_some_and(|selected| selected != hint) {
                return Err(profile_error(
                    span,
                    "x86-64 binary operands have different scalar types",
                ));
            }
            selected = Some(hint);
        }
        Ok(selected.unwrap_or(ScalarKind64::U32))
    }

    fn expression_hint(&self, expression: &Expr) -> Result<Option<ScalarKind64>, Diagnostic> {
        match &expression.node {
            Expression::Identifier(name) => Ok(Some(self.lookup(name, expression.span)?.kind)),
            Expression::Index { object, .. } => Ok(Some(self.direct_array(object)?.element)),
            Expression::Bool(_) => Ok(Some(ScalarKind64::Bool)),
            Expression::Integer(_) => Ok(None),
            Expression::Unary {
                operator: UnaryOperator::Not,
                ..
            } => Ok(Some(ScalarKind64::Bool)),
            Expression::Unary {
                operator: UnaryOperator::Negate,
                ..
            } => Ok(Some(ScalarKind64::I32)),
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
            } => Ok(Some(ScalarKind64::Bool)),
            Expression::Binary { left, right, .. } => {
                let left = self.expression_hint(left)?;
                let right = self.expression_hint(right)?;
                match (left, right) {
                    (Some(left), Some(right)) if left != right => Err(profile_error(
                        expression.span,
                        "x86-64 expression mixes scalar types",
                    )),
                    (Some(kind), _) | (_, Some(kind)) => Ok(Some(kind)),
                    _ => Ok(None),
                }
            }
            Expression::Call { callee, .. } => {
                if let Expression::Identifier(name) = &callee.node {
                    Ok(ScalarKind64::from_name(name).or_else(|| {
                        self.functions
                            .get(name)
                            .and_then(|function| function.return_kind)
                    }))
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
        left: ScalarKind64,
        right: ScalarKind64,
        span: Span,
    ) -> Result<ScalarKind64, Diagnostic> {
        if left != right {
            return Err(profile_error(
                span,
                "x86-64 binary operands must have exactly the same type",
            ));
        }
        match operator {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                if !left.numeric() {
                    return Err(profile_error(span, "boolean arithmetic is unavailable"));
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
                Ok(ScalarKind64::Bool)
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
                Ok(ScalarKind64::Bool)
            }
            BinaryOperator::And | BinaryOperator::Or => unreachable!(),
        }
    }

    fn emit_checked_arithmetic(&mut self, operator: BinaryOperator, kind: ScalarKind64) {
        match kind {
            ScalarKind64::U8 => self.emit_checked_narrow(operator, u8::MAX.into()),
            ScalarKind64::U16 => self.emit_checked_narrow(operator, u16::MAX.into()),
            ScalarKind64::U32 => self.emit_checked_u32(operator),
            ScalarKind64::I32 => self.emit_checked_i32(operator),
            ScalarKind64::Bool => unreachable!(),
        }
    }

    fn emit_checked_narrow(&mut self, operator: BinaryOperator, maximum: u32) {
        match operator {
            BinaryOperator::Add => {
                self.assembler.emit(&[0x01, 0xd8, 0x3d]);
                self.assembler.emit(&maximum.to_le_bytes());
                self.assembler
                    .conditional_jump(0x87, self.arithmetic_failure);
            }
            BinaryOperator::Subtract => {
                self.assembler.emit(&[0x29, 0xd8]);
                self.assembler
                    .conditional_jump(0x82, self.arithmetic_failure);
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(&[0xf7, 0xe3, 0x3d]);
                self.assembler.emit(&maximum.to_le_bytes());
                self.assembler
                    .conditional_jump(0x87, self.arithmetic_failure);
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
                self.assembler.emit(&[0xf7, 0xe3, 0x85, 0xd2]);
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
                    .conditional_jump(0x80, self.arithmetic_failure);
            }
            BinaryOperator::Multiply => {
                self.assembler
                    .emit(&[0xf7, 0xeb, 0x89, 0xc1, 0xc1, 0xf9, 0x1f, 0x39, 0xca]);
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
                self.assembler.emit(&[0x83, 0xfb, 0xff]);
                self.assembler
                    .conditional_jump(0x84, self.arithmetic_failure);
                self.assembler.bind(safe);
                self.assembler.emit(&[0x99, 0xf7, 0xfb]);
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

    fn allocate_local(
        &mut self,
        kind: ScalarKind64,
        span: Span,
    ) -> Result<ScalarLocal64, Diagnostic> {
        let end = self
            .next_local
            .checked_add(kind.bytes())
            .ok_or_else(|| profile_error(span, "x86-64 local storage overflowed"))?;
        if end > LOCALS_BYTES {
            return Err(profile_error(
                span,
                "x86-64 local storage exceeds the bounded 4096-byte page",
            ));
        }
        let local = ScalarLocal64 {
            address: LOCALS_ORIGIN + self.next_local,
            kind,
        };
        self.next_local = end;
        Ok(local)
    }

    fn preallocate_block_locals(
        &mut self,
        function: &str,
        block: &Block,
        frame_slots: &mut Vec<ScalarLocal64>,
    ) -> Result<(), Diagnostic> {
        for statement in &block.statements {
            match &statement.node {
                Statement::Binding { annotation, .. } => {
                    let annotation = annotation.as_ref().ok_or_else(|| {
                        profile_error(
                            statement.span,
                            "x86-64 locals require an explicit scalar annotation",
                        )
                    })?;
                    let local = if let Some((element, length)) = array_annotation64(annotation)? {
                        let mut first = None;
                        for _ in 0..length {
                            let element_local = self.allocate_local(element, statement.span)?;
                            first.get_or_insert(element_local.address);
                            frame_slots.push(element_local);
                        }
                        LocalValue64::Array(ArrayLocal64 {
                            address: first.unwrap_or(LOCALS_ORIGIN + self.next_local),
                            element,
                            length,
                        })
                    } else {
                        let kind = ScalarKind64::from_name(&annotation.name).ok_or_else(|| {
                            profile_error(
                                annotation.span,
                                "x86-64 locals support only exact scalars and bounded fixed arrays",
                            )
                        })?;
                        let scalar = self.allocate_local(kind, statement.span)?;
                        frame_slots.push(scalar);
                        LocalValue64::Scalar(scalar)
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

    fn lookup_value(&self, name: &str, span: Span) -> Result<LocalValue64, Diagnostic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| profile_error(span, format!("unknown x86-64 local `{name}`")))
    }

    fn lookup(&self, name: &str, span: Span) -> Result<ScalarLocal64, Diagnostic> {
        match self.lookup_value(name, span)? {
            LocalValue64::Scalar(local) => Ok(local),
            LocalValue64::Array(_) => Err(profile_error(
                span,
                format!("x86-64 fixed array `{name}` requires an index"),
            )),
        }
    }

    fn direct_array(&self, object: &Expr) -> Result<ArrayLocal64, Diagnostic> {
        let Expression::Identifier(name) = &object.node else {
            return Err(profile_error(
                object.span,
                "x86-64 indexing currently requires a direct fixed-array local",
            ));
        };
        match self.lookup_value(name, object.span)? {
            LocalValue64::Array(array) => Ok(array),
            LocalValue64::Scalar(_) => Err(profile_error(
                object.span,
                format!("x86-64 scalar `{name}` cannot be indexed"),
            )),
        }
    }

    fn compile_array_offset(
        &mut self,
        array: ArrayLocal64,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let kind = self.compile_expression(index, None)?;
        if !kind.numeric() {
            return Err(profile_error(
                index.span,
                "x86-64 array index must be an integer",
            ));
        }
        self.assembler.emit(&[0x3d]);
        self.assembler.emit(&(array.length as u32).to_le_bytes());
        self.assembler.conditional_jump(0x83, self.bounds_failure);
        match array.element.bytes() {
            1 => {}
            2 => self.assembler.emit(&[0xd1, 0xe0]),
            4 => self.assembler.emit(&[0xc1, 0xe0, 0x02]),
            _ => unreachable!(),
        }
        Ok(())
    }

    fn load_indexed_eax(&mut self, array: ArrayLocal64) {
        self.assembler.emit(match array.element {
            ScalarKind64::U8 => &[0x0f, 0xb6, 0x80][..],
            ScalarKind64::U16 => &[0x0f, 0xb7, 0x80][..],
            ScalarKind64::U32 | ScalarKind64::I32 | ScalarKind64::Bool => &[0x8b, 0x80][..],
        });
        self.assembler.emit(&array.address.to_le_bytes());
    }

    fn load_indexed_ecx(&mut self, array: ArrayLocal64) {
        self.assembler.emit(match array.element {
            ScalarKind64::U8 => &[0x0f, 0xb6, 0x81][..],
            ScalarKind64::U16 => &[0x0f, 0xb7, 0x81][..],
            ScalarKind64::U32 | ScalarKind64::I32 | ScalarKind64::Bool => &[0x8b, 0x81][..],
        });
        self.assembler.emit(&array.address.to_le_bytes());
    }

    fn store_indexed_ecx_from_ebx(&mut self, array: ArrayLocal64) {
        self.assembler.emit(match array.element {
            ScalarKind64::U8 => &[0x88, 0x99][..],
            ScalarKind64::U16 => &[0x66, 0x89, 0x99][..],
            ScalarKind64::U32 | ScalarKind64::I32 | ScalarKind64::Bool => &[0x89, 0x99][..],
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
                "x86-64 place assignment requires a fixed-array element",
            ));
        };
        let array = self.direct_array(object)?;
        self.compile_array_offset(array, index)?;
        self.assembler.emit(&[0x50]);
        let actual = self.compile_expression(value, Some(array.element))?;
        self.require_kind(actual, array.element, value.span)?;
        self.assembler.emit(&[0x89, 0xc3, 0x59]);
        if operator != AssignmentOperator::Assign {
            if !array.element.numeric() {
                return Err(profile_error(
                    span,
                    "boolean compound assignment is unavailable",
                ));
            }
            self.load_indexed_ecx(array);
            self.assembler.emit(&[0x51]);
            let operation = match operator {
                AssignmentOperator::Add => BinaryOperator::Add,
                AssignmentOperator::Subtract => BinaryOperator::Subtract,
                AssignmentOperator::Multiply => BinaryOperator::Multiply,
                AssignmentOperator::Divide => BinaryOperator::Divide,
                AssignmentOperator::Assign => unreachable!(),
            };
            self.emit_checked_arithmetic(operation, array.element);
            self.assembler.emit(&[0x59, 0x89, 0xc3]);
        }
        self.store_indexed_ecx_from_ebx(array);
        Ok(())
    }

    fn require_device_io(&self, span: Span) -> Result<(), Diagnostic> {
        if self.device_io_depth == 0 {
            Err(profile_error(
                span,
                "x86-64 hardware port access requires `unsafe uses DeviceIo { ... }`",
            ))
        } else {
            Ok(())
        }
    }

    fn compile_timer_ticks(
        &mut self,
        field: &str,
        arguments: &[Expr],
        expected: Option<ScalarKind64>,
        span: Span,
    ) -> Result<ScalarKind64, Diagnostic> {
        if field != "ticks" || !arguments.is_empty() {
            return Err(profile_error(
                span,
                format!(
                    "no x86-64 timer operation `Time.{field}` with {} arguments",
                    arguments.len()
                ),
            ));
        }
        let authorized = self
            .functions
            .get(&self.current_function)
            .is_some_and(|function| function.timer_authority);
        if !authorized {
            return Err(profile_error(
                span,
                "x86-64 `Time.ticks()` requires an explicit `uses Timer` function contract",
            ));
        }
        if let Some(expected) = expected {
            self.require_kind(ScalarKind64::U32, expected, span)?;
        }
        self.assembler.emit(&[0x8b, 0x04, 0x25]);
        self.assembler.emit(&TIMER_TICKS.to_le_bytes());
        Ok(ScalarKind64::U32)
    }

    fn compile_port_read(
        &mut self,
        field: &str,
        arguments: &[Expr],
        expected: Option<ScalarKind64>,
        span: Span,
    ) -> Result<ScalarKind64, Diagnostic> {
        self.require_device_io(span)?;
        if field != "read_u8" || arguments.len() != 1 {
            return Err(profile_error(
                span,
                format!(
                    "no x86-64 hardware input `Port.{field}` with {} arguments",
                    arguments.len()
                ),
            ));
        }
        let actual = self.compile_expression(&arguments[0], Some(ScalarKind64::U16))?;
        self.require_kind(actual, ScalarKind64::U16, arguments[0].span)?;
        self.assembler.emit(&[0x89, 0xc2, 0xec, 0x0f, 0xb6, 0xc0]);
        if let Some(expected) = expected {
            self.require_kind(ScalarKind64::U8, expected, span)?;
        }
        Ok(ScalarKind64::U8)
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
                    "no x86-64 hardware output `Port.{field}` with {} arguments",
                    arguments.len()
                ),
            ));
        }
        let port = self.compile_expression(&arguments[0], Some(ScalarKind64::U16))?;
        self.require_kind(port, ScalarKind64::U16, arguments[0].span)?;
        self.assembler.emit(&[0x89, 0xc2, 0x52]);
        let value = self.compile_expression(&arguments[1], Some(ScalarKind64::U8))?;
        self.require_kind(value, ScalarKind64::U8, arguments[1].span)?;
        self.assembler.emit(&[0x89, 0xc3, 0x5a, 0x89, 0xd8, 0xee]);
        Ok(())
    }

    fn load(&mut self, local: ScalarLocal64) {
        self.assembler.emit(match local.kind {
            ScalarKind64::U8 => &[0x0f, 0xb6, 0x04, 0x25],
            ScalarKind64::U16 => &[0x0f, 0xb7, 0x04, 0x25],
            ScalarKind64::U32 | ScalarKind64::I32 | ScalarKind64::Bool => &[0x8b, 0x04, 0x25],
        });
        self.assembler.emit(&local.address.to_le_bytes());
    }

    fn store(&mut self, local: ScalarLocal64) {
        self.assembler.emit(match local.kind {
            ScalarKind64::U8 => &[0x88, 0x04, 0x25],
            ScalarKind64::U16 => &[0x66, 0x89, 0x04, 0x25],
            ScalarKind64::U32 | ScalarKind64::I32 | ScalarKind64::Bool => &[0x89, 0x04, 0x25],
        });
        self.assembler.emit(&local.address.to_le_bytes());
    }

    fn require_kind(
        &self,
        actual: ScalarKind64,
        expected: ScalarKind64,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if actual == expected {
            Ok(())
        } else {
            Err(profile_error(
                span,
                "x86-64 scalar expression has a different exact type",
            ))
        }
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
                        "x86-64 output accepts printable ASCII, newline, and carriage return only",
                    ));
                }
            }
        }
        bytes.push(0);
        Ok(add_data(self.assembler, self.data, &bytes))
    }

    fn emit_print_bool(&mut self) {
        let false_label = self.assembler.label();
        let output = self.assembler.label();
        let true_text = add_data(self.assembler, self.data, b"true\0");
        let false_text = add_data(self.assembler, self.data, b"false\0");
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
}

#[allow(clippy::too_many_arguments)]
fn emit_output_routines(
    assembler: &mut Assembler,
    data: &mut Vec<(Label, Vec<u8>)>,
    emit_character: Label,
    print_string: Label,
    print_unsigned: Label,
    print_signed: Label,
    newline: Label,
    arithmetic_failure: Label,
    stack_failure: Label,
    bounds_failure: Label,
    halt: Label,
) {
    assembler.bind(emit_character);
    let carriage = assembler.label();
    let linefeed = assembler.label();
    let check_wrap = assembler.label();
    let done = assembler.label();
    assembler.emit(&[0x53, 0x51, 0x52, 0xe6, 0xe9, 0x3c, b'\r']);
    assembler.conditional_jump(0x84, carriage);
    assembler.emit(&[0x3c, b'\n']);
    assembler.conditional_jump(0x84, linefeed);
    assembler.emit(&[0xb4, 0x07, 0x66, 0x89, 0x07, 0x83, 0xc7, 0x02]);
    assembler.jump(check_wrap);
    assembler.bind(carriage);
    emit_cursor_remainder(assembler);
    assembler.emit(&[0x29, 0xd7]);
    assembler.jump(done);
    assembler.bind(linefeed);
    emit_cursor_remainder(assembler);
    assembler.emit(&[0x29, 0xd7, 0x81, 0xc7]);
    assembler.emit(&160u32.to_le_bytes());
    assembler.bind(check_wrap);
    assembler.emit(&[0x81, 0xff]);
    assembler.emit(&VGA_TEXT_END.to_le_bytes());
    assembler.conditional_jump(0x82, done);
    assembler.emit(&[0xbf]);
    assembler.emit(&VGA_TEXT.to_le_bytes());
    assembler.bind(done);
    assembler.emit(&[0x5a, 0x59, 0x5b, 0xc3]);

    assembler.bind(print_string);
    let next = assembler.label();
    let end = assembler.label();
    assembler.bind(next);
    assembler.emit(&[0xac, 0x84, 0xc0]);
    assembler.conditional_jump(0x84, end);
    assembler.call(emit_character);
    assembler.jump(next);
    assembler.bind(end);
    assembler.emit(&[0xc3]);

    assembler.bind(print_unsigned);
    let nonzero = assembler.label();
    let divide = assembler.label();
    let digits = assembler.label();
    assembler.emit(&[0x85, 0xc0]);
    assembler.conditional_jump(0x85, nonzero);
    assembler.emit(&[0xb0, b'0']);
    assembler.call(emit_character);
    assembler.emit(&[0xc3]);
    assembler.bind(nonzero);
    assembler.emit(&[0x31, 0xc9, 0xbb, 10, 0, 0, 0]);
    assembler.bind(divide);
    assembler.emit(&[0x31, 0xd2, 0xf7, 0xf3, 0x52, 0xff, 0xc1, 0x85, 0xc0]);
    assembler.conditional_jump(0x85, divide);
    assembler.bind(digits);
    assembler.emit(&[0x58, 0x04, b'0']);
    assembler.call(emit_character);
    assembler.emit(&[0xff, 0xc9]);
    assembler.conditional_jump(0x85, digits);
    assembler.emit(&[0xc3]);

    assembler.bind(print_signed);
    let nonnegative = assembler.label();
    assembler.emit(&[0x85, 0xc0]);
    assembler.conditional_jump(0x89, nonnegative);
    assembler.emit(&[0x50, 0xb0, b'-']);
    assembler.call(emit_character);
    assembler.emit(&[0x58, 0xf7, 0xd8]);
    assembler.bind(nonnegative);
    assembler.jump(print_unsigned);

    assembler.bind(newline);
    assembler.emit(&[0xb0, b'\r']);
    assembler.call(emit_character);
    assembler.emit(&[0xb0, b'\n']);
    assembler.call(emit_character);
    assembler.emit(&[0xc3]);

    assembler.bind(arithmetic_failure);
    let failure = add_data(assembler, data, b"x86-64 arithmetic failure\0");
    assembler.emit(&[0xbe]);
    assembler.absolute(failure);
    assembler.call(print_string);
    assembler.call(newline);
    assembler.jump(halt);

    assembler.bind(stack_failure);
    let failure = add_data(assembler, data, b"x86-64 stack limit exceeded\0");
    assembler.emit(&[0xbe]);
    assembler.absolute(failure);
    assembler.call(print_string);
    assembler.call(newline);
    assembler.jump(halt);

    assembler.bind(bounds_failure);
    let failure = add_data(assembler, data, b"x86-64 index out of bounds\0");
    assembler.emit(&[0xbe]);
    assembler.absolute(failure);
    assembler.call(print_string);
    assembler.call(newline);
    assembler.jump(halt);
}

fn emit_cursor_remainder(assembler: &mut Assembler) {
    assembler.emit(&[0x89, 0xf8, 0x2d]);
    assembler.emit(&VGA_TEXT.to_le_bytes());
    assembler.emit(&[0x31, 0xd2, 0xbb]);
    assembler.emit(&160u32.to_le_bytes());
    assembler.emit(&[0xf7, 0xf3]);
}

fn add_data(assembler: &mut Assembler, data: &mut Vec<(Label, Vec<u8>)>, bytes: &[u8]) -> Label {
    let label = assembler.label();
    data.push((label, bytes.to_vec()));
    label
}

#[derive(Clone, Copy)]
struct Label(usize);

struct Fixup {
    at: usize,
    label: Label,
    relative: bool,
}

struct Assembler {
    bytes: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
}

impl Assembler {
    fn new(bytes: Vec<u8>) -> Self {
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

    fn relative(&mut self, label: Label) {
        let at = self.bytes.len();
        self.emit(&[0; 4]);
        self.fixups.push(Fixup {
            at,
            label,
            relative: true,
        });
    }

    fn absolute(&mut self, label: Label) {
        let at = self.bytes.len();
        self.emit(&[0; 4]);
        self.fixups.push(Fixup {
            at,
            label,
            relative: false,
        });
    }

    fn finish(mut self, origin: u16, span: Span) -> Result<Vec<u8>, Diagnostic> {
        for fixup in self.fixups {
            let target = self.labels[fixup.label.0]
                .ok_or_else(|| profile_error(span, "unbound x86-64 machine-code label"))?;
            let value = if fixup.relative {
                i32::try_from(target as i64 - (fixup.at + 4) as i64)
                    .map(|value| value as u32)
                    .map_err(|_| {
                        profile_error(span, "x86-64 relative branch escaped 32-bit reach")
                    })?
            } else {
                u32::from(origin)
                    .checked_add(target as u32)
                    .ok_or_else(|| profile_error(span, "x86-64 absolute address overflowed"))?
            };
            self.bytes[fixup.at..fixup.at + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(self.bytes)
    }
}

fn array_annotation64(
    annotation: &crate::ast::TypeName,
) -> Result<Option<(ScalarKind64, usize)>, Diagnostic> {
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
            "x86-64 fixed arrays require exactly one element type",
        ));
    }
    let length = length
        .parse::<usize>()
        .map_err(|_| profile_error(annotation.span, "invalid x86-64 fixed-array length"))?;
    let element = ScalarKind64::from_name(&annotation.arguments[0].name).ok_or_else(|| {
        profile_error(
            annotation.arguments[0].span,
            "x86-64 fixed arrays support only `u8`, `u16`, `u32`, `i32`, and `bool` elements",
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
            "x86-64 does not yet accept modules, imports, user types, traits, or implementations",
        ));
    }
    if program.functions.len() > MAX_FUNCTIONS {
        return Err(profile_error(
            program
                .functions
                .get(MAX_FUNCTIONS)
                .map_or(Span::point(1, 1), |function| function.name_span),
            format!("x86-64 programs support at most {MAX_FUNCTIONS} functions"),
        ));
    }
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| {
            profile_error(
                Span::point(1, 1),
                "x86-64 requires a plain `fn main()` entry function",
            )
        })?;
    if !main.parameters.is_empty()
        || main.return_type.is_some()
        || main.asynchronous
        || !main.generics.is_empty()
        || main.capabilities.is_some()
        || main.external.is_some()
    {
        return Err(profile_error(
            main.span,
            "x86-64 requires plain `fn main()` with no parameters, result, generics, capabilities, or external ABI",
        ));
    }
    for function in &program.functions {
        if function.name == "print" {
            return Err(profile_error(
                function.name_span,
                "`print` is reserved by the x86-64 output ABI",
            ));
        }
        let unsupported_capability = function.capabilities.as_ref().is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|use_| !matches!(use_.capability, Capability::DeviceIo | Capability::Timer))
        });
        if function.asynchronous
            || !function.generics.is_empty()
            || unsupported_capability
            || function.external.is_some()
        {
            return Err(profile_error(
                function.span,
                "x86-64 functions cannot be async, generic, external, or carry capabilities other than `DeviceIo` and `Timer`",
            ));
        }
        for parameter in &function.parameters {
            if ScalarKind64::from_name(&parameter.ty.name).is_none() {
                return Err(profile_error(
                    parameter.ty.span,
                    "x86-64 parameters support only `u8`, `u16`, `u32`, `i32`, and `bool`",
                ));
            }
        }
        if let Some(return_type) = &function.return_type
            && return_type.name != "Unit"
            && ScalarKind64::from_name(&return_type.name).is_none()
        {
            return Err(profile_error(
                return_type.span,
                "x86-64 returns support only exact scalar values and `Unit`",
            ));
        }
    }
    Ok(main)
}

fn profile_error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span)
        .with_help("use `--freestanding32` while the x86-64 language profile expands")
}

fn error_at(path: &Path, span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span).with_file(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_source;

    #[test]
    fn x86_64_bootstrap_checks_cpu_builds_four_level_paging_and_enters_long_mode() {
        let program = check_source("fn main() { print(\"Hello from 64-bit DISP\") }").unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        assert!(first.len() > BOOT_SECTOR_BYTES);
        assert_eq!(&first[BOOT_PAYLOAD_BYTES..BOOT_SECTOR_BYTES], &[0x55, 0xaa]);
        let stage = &first[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(2).any(|bytes| bytes == [0x0f, 0xa2]));
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0x80, 0x3c, 0xff, 0x0f, 0x95, 0xc3])
        );
        assert!(stage.windows(5).any(|bytes| bytes == [0xb9, 0, 0x10, 0, 0]));
        assert!(stage.windows(3).any(|bytes| bytes == [0x0f, 0x22, 0xe0]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x0f, 0x32]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x0f, 0x30]));
        assert!(stage.windows(5).any(|bytes| bytes == [0x0d, 0, 0, 1, 0x80]));
        assert!(stage.windows(1).any(|bytes| bytes == [0xea]));
        assert!(
            stage
                .windows(8)
                .any(|bytes| bytes == [0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0])
        );
        assert!(
            stage
                .windows(23)
                .any(|bytes| bytes == b"Hello from 64-bit DISP\0")
        );
    }

    #[test]
    fn x86_64_idt_and_profile_are_bounded_and_fail_closed() {
        let program = check_source("fn main() { print(\"long IDT\") }").unwrap();
        let image = compile_x86_64(&program).unwrap();
        let stage = &image[BOOT_SECTOR_BYTES..];
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0x0f, 0x01, 0x1c, 0x25])
        );
        assert!(
            stage
                .windows(5)
                .any(|bytes| bytes == [0x48, 0xff, 0xc9, 0x0f, 0x85])
        );
        assert!(
            stage
                .windows(21)
                .any(|bytes| bytes == b"x86-64 CPU exception\0")
        );
        let unsupported = check_source("struct Thing {} fn main() {}").unwrap();
        let error = compile_x86_64(&unsupported).unwrap_err();
        assert!(error.message.contains("user types"));
    }

    #[test]
    fn x86_64_nx_paging_whitelists_only_the_bounded_stage_for_execution() {
        let program = check_source("fn main(){print(\"NX paging active\")}").unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0xf7, 0xc2, 0, 0, 0x10, 0])
        );
        assert!(stage.windows(5).any(|bytes| bytes == [0x0d, 0, 9, 0, 0]));
        assert!(
            stage
                .windows(7)
                .any(|bytes| bytes == [0xc7, 0x47, 0x04, 0, 0, 0, 0x80])
        );
        assert!(
            stage
                .windows(7)
                .any(|bytes| bytes == [0xc7, 0x47, 0x04, 0, 0, 0, 0])
        );
        assert_eq!(STAGE_READ_ONLY_FIRST_PAGE, 7);
        assert_eq!(STAGE_READ_ONLY_PAGES, 9);
    }

    #[test]
    fn x86_64_idt_routes_security_critical_faults_to_distinct_fail_closed_handlers() {
        let program = check_source("fn main(){print(\"fault routing active\")}").unwrap();
        let image = compile_x86_64(&program).unwrap();
        let stage = &image[BOOT_SECTOR_BYTES..];
        let mut handlers = Vec::new();
        for slot in [
            IDT_ORIGIN + 6 * 16,
            IDT_ORIGIN + 13 * 16,
            IDT_ORIGIN + 14 * 16,
        ] {
            let mut instruction = vec![0xbf];
            instruction.extend_from_slice(&slot.to_le_bytes());
            let gate = stage
                .windows(instruction.len())
                .position(|bytes| bytes == instruction)
                .unwrap();
            assert_eq!(stage[gate + 5], 0xb8);
            handlers.push(u32::from_le_bytes(
                stage[gate + 6..gate + 10].try_into().unwrap(),
            ));
            assert_eq!(
                &stage[gate + 10..gate + 27],
                &[
                    0x66, 0x89, 0x07, 0x66, 0xc7, 0x47, 0x02, 0x18, 0, 0xc6, 0x47, 0x04, 0, 0xc6,
                    0x47, 0x05, 0x8e
                ]
            );
        }
        handlers.sort_unstable();
        handlers.dedup();
        assert_eq!(handlers.len(), 3);
        for handler in handlers {
            let offset = usize::try_from(handler - u32::from(STAGE_ORIGIN)).unwrap();
            assert_eq!(stage[offset], 0xfa);
            assert_eq!(stage[offset + 1], 0xbc);
            assert_eq!(&stage[offset + 2..offset + 6], &LONG_STACK.to_le_bytes());
            assert_eq!(stage[offset + 6], 0xbf);
            assert_eq!(&stage[offset + 7..offset + 11], &VGA_TEXT.to_le_bytes());
        }
        for message in [
            b"x86-64 invalid opcode\0".as_slice(),
            b"x86-64 general protection\0".as_slice(),
            b"x86-64 page fault\0".as_slice(),
        ] {
            assert!(stage.windows(message.len()).any(|bytes| bytes == message));
        }
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0x0f, 0x01, 0x1c, 0x25])
        );
    }

    #[test]
    fn x86_64_pic_is_remapped_fully_masked_and_routed_before_user_code() {
        let program = check_source("fn main(){print(\"PIC quarantine active\")}").unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];

        let irq_table = IDT_ORIGIN + EXCEPTION_VECTORS * 16;
        let mut fill_prefix = vec![0xbf];
        fill_prefix.extend_from_slice(&irq_table.to_le_bytes());
        fill_prefix.push(0xb9);
        fill_prefix.extend_from_slice(&LEGACY_IRQ_VECTORS.to_le_bytes());
        fill_prefix.push(0xb8);
        let fill = stage
            .windows(fill_prefix.len())
            .position(|bytes| bytes == fill_prefix)
            .unwrap();
        let handler = u32::from_le_bytes(
            stage[fill + fill_prefix.len()..fill + fill_prefix.len() + 4]
                .try_into()
                .unwrap(),
        );
        let handler = usize::try_from(handler - u32::from(STAGE_ORIGIN)).unwrap();
        let mut handler_prefix = vec![0xfa, 0xbc];
        handler_prefix.extend_from_slice(&LONG_STACK.to_le_bytes());
        handler_prefix.push(0xbf);
        handler_prefix.extend_from_slice(&VGA_TEXT.to_le_bytes());
        handler_prefix.extend_from_slice(&[0xb0, 0x20, 0xe6, 0xa0, 0xe6, 0x20]);
        assert_eq!(
            &stage[handler..handler + handler_prefix.len()],
            handler_prefix
        );
        assert!(
            stage
                .windows(37)
                .any(|bytes| bytes == b"x86-64 unexpected hardware interrupt\0")
        );

        let mut pic_sequence = Vec::new();
        for (port, value) in [
            (0x20, 0x11),
            (0xa0, 0x11),
            (0x21, 0x20),
            (0xa1, 0x28),
            (0x21, 0x04),
            (0xa1, 0x02),
            (0x21, 0x01),
            (0xa1, 0x01),
            (0x21, 0xff),
            (0xa1, 0xff),
        ] {
            pic_sequence.extend_from_slice(&[0xb0, value, 0xe6, port, 0xe6, 0x80]);
        }
        assert!(
            stage
                .windows(pic_sequence.len())
                .any(|bytes| bytes == pic_sequence)
        );
        assert!(!stage.contains(&0xfb)); // no STI in the bounded profile image

        let lidt = stage
            .windows(4)
            .position(|bytes| bytes == [0x0f, 0x01, 0x1c, 0x25])
            .unwrap();
        let idtr = u32::from_le_bytes(stage[lidt + 4..lidt + 8].try_into().unwrap());
        let idtr = usize::try_from(idtr - u32::from(STAGE_ORIGIN)).unwrap();
        assert_eq!(
            u16::from_le_bytes(stage[idtr..idtr + 2].try_into().unwrap()),
            (IDT_ENTRIES * 16 - 1) as u16
        );
        assert_eq!(
            u64::from_le_bytes(stage[idtr + 2..idtr + 10].try_into().unwrap()),
            u64::from(IDT_ORIGIN)
        );
    }

    #[test]
    fn x86_64_timer_capability_installs_one_bounded_100_hz_irq_source() {
        let source = "fn ticks()->u32 uses Timer{return Time.ticks()} fn main(){var first:u32=ticks() var current:u32=first while current==first{current=ticks()} print(\"timer active\")}";
        let program = check_source(source).unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];

        let mut timer_gate = vec![0xbf];
        timer_gate.extend_from_slice(&(IDT_ORIGIN + EXCEPTION_VECTORS * 16).to_le_bytes());
        timer_gate.push(0xb8);
        let gate = stage
            .windows(timer_gate.len())
            .rposition(|bytes| bytes == timer_gate)
            .unwrap();
        let handler = u32::from_le_bytes(
            stage[gate + timer_gate.len()..gate + timer_gate.len() + 4]
                .try_into()
                .unwrap(),
        );
        let handler = usize::try_from(handler - u32::from(STAGE_ORIGIN)).unwrap();
        let mut handler_bytes = vec![0x50, 0xff, 0x04, 0x25];
        handler_bytes.extend_from_slice(&TIMER_TICKS.to_le_bytes());
        handler_bytes.extend_from_slice(&[0xb0, 0x20, 0xe6, 0x20, 0x58, 0x48, 0xcf]);
        assert_eq!(
            &stage[handler..handler + handler_bytes.len()],
            handler_bytes
        );

        let pit = [
            0xb0, 0x36, 0xe6, 0x43, 0xe6, 0x80, 0xb0, 0x9c, 0xe6, 0x40, 0xe6, 0x80, 0xb0, 0x2e,
            0xe6, 0x40, 0xe6, 0x80,
        ];
        assert!(stage.windows(pit.len()).any(|bytes| bytes == pit));
        let mut clear_and_unmask = vec![0xc7, 0x04, 0x25];
        clear_and_unmask.extend_from_slice(&TIMER_TICKS.to_le_bytes());
        clear_and_unmask.extend_from_slice(&[
            0, 0, 0, 0, 0xb0, 0xfe, 0xe6, 0x21, 0xe6, 0x80, 0xb0, 0xff, 0xe6, 0xa1, 0xe6, 0x80,
            0xfb,
        ]);
        assert!(
            stage
                .windows(clear_and_unmask.len())
                .any(|bytes| bytes == clear_and_unmask)
        );
        let mut read_ticks = vec![0x8b, 0x04, 0x25];
        read_ticks.extend_from_slice(&TIMER_TICKS.to_le_bytes());
        assert!(
            stage
                .windows(read_ticks.len())
                .any(|bytes| bytes == read_ticks)
        );

        let inferred = check_source(
            "fn ticks()->u32{return Time.ticks()} fn main(){var value:u32=ticks() print(value)}",
        )
        .unwrap();
        let error = compile_x86_64(&inferred).unwrap_err();
        assert!(error.message.contains("explicit `uses Timer`"));
    }

    #[test]
    fn x86_64_checked_scalars_use_bounded_absolute_locals_and_long_mode_stack_encodings() {
        let program = check_source(
            "fn main() { var total: u32 = 0 var next: u32 = 1 while next <= 10 { total += next next += 1 } var exact: bool = total == 55 && next == 11 var signed: i32 = -84 signed /= 2 var byte: u8 = 250 byte += 5 print(total) print(exact) print(signed) print(byte) }",
        )
        .unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];
        assert!(
            stage
                .windows(7)
                .any(|bytes| bytes == [0x89, 0x04, 0x25, 0x00, 0x50, 0x10, 0x00])
        );
        assert!(
            stage
                .windows(8)
                .any(|bytes| bytes == [0x0f, 0xb6, 0x04, 0x25, 0x10, 0x50, 0x10, 0x00])
        );
        assert!(
            stage
                .windows(9)
                .any(|bytes| bytes == [0x50, 0xb8, 10, 0, 0, 0, 0x89, 0xc3, 0x58])
        );
        assert!(stage.windows(2).any(|bytes| bytes == [0xff, 0xc1]));
        assert!(stage.windows(2).any(|bytes| bytes == [0xff, 0xc9]));
        assert!(
            stage
                .windows(26)
                .any(|bytes| bytes == b"x86-64 arithmetic failure\0")
        );

        let inferred = check_source("fn main() { var value = 1 print(value) }").unwrap();
        let error = compile_x86_64(&inferred).unwrap_err();
        assert!(error.message.contains("explicit"));

        let mut assembler = Assembler::new(Vec::new());
        let print_string = assembler.label();
        let print_unsigned = assembler.label();
        let print_signed = assembler.label();
        let newline = assembler.label();
        let arithmetic_failure = assembler.label();
        let stack_failure = assembler.label();
        let bounds_failure = assembler.label();
        let halt = assembler.label();
        let mut data = Vec::new();
        let empty = check_source("fn main() {}").unwrap();
        let mut compiler = ScalarCompiler64::new(
            &mut assembler,
            &mut data,
            &empty,
            print_string,
            print_unsigned,
            print_signed,
            newline,
            arithmetic_failure,
            stack_failure,
            bounds_failure,
            halt,
        )
        .unwrap();
        for _ in 0..(LOCALS_BYTES / ScalarKind64::Bool.bytes()) {
            compiler
                .allocate_local(ScalarKind64::Bool, Span::point(1, 1))
                .unwrap();
        }
        let error = compiler
            .allocate_local(ScalarKind64::U8, Span::point(1, 1))
            .unwrap_err();
        assert!(error.message.contains("4096-byte page"));
    }

    #[test]
    fn x86_64_functions_snapshot_guard_and_restore_recursive_scalar_frames() {
        let program = check_source(
            "fn add(left:u32,right:u32)->u32{return left+right} fn nested(value:u32)->u32{return add(value,add(2,3))} fn factorial(value:u16)->u16{if value<=1{return 1} var previous:u16=value-1 var partial:u16=factorial(previous) return value*partial} fn even(value:u8)->bool{if value==0{return true} return odd(value-1)} fn odd(value:u8)->bool{if value==0{return false} return even(value-1)} fn main(){print(nested(10)) print(factorial(6)) print(even(10))}",
        )
        .unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(3).any(|bytes| bytes == [0x48, 0x81, 0xfc]));
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0x89, 0xc1, 0x58, 0x89, 0x04, 0x25])
        );
        assert!(stage.windows(2).any(|bytes| bytes == [0x89, 0xc8]));
        assert!(
            stage
                .windows(28)
                .any(|bytes| bytes == b"x86-64 stack limit exceeded\0")
        );

        let runaway = check_source(
            "fn recurse(value:u8)->u8{return recurse(value)} fn main(){print(recurse(1))}",
        )
        .unwrap();
        let image = compile_x86_64(&runaway).unwrap();
        assert_eq!(
            image[BOOT_SECTOR_BYTES..]
                .windows(28)
                .filter(|bytes| *bytes == b"x86-64 stack limit exceeded\0")
                .count(),
            1
        );
    }

    #[test]
    fn x86_64_fixed_arrays_use_exact_storage_checked_indices_and_recursive_frames() {
        let program = check_source(
            "fn pick(seed:u8,depth:u8)->u8{var bytes:[u8;4]=[seed,u8(2),u8(3),u8(4)] bytes[1]+=5 if depth==0{return bytes[1]} return pick(bytes[0],depth-1)} fn main(){var values:[u32;4]=[u32(10),u32(20),u32(30),u32(40)] var index:u32=2 values[index]+=5 print(values[index]) print(pick(9,2))}",
        )
        .unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];
        assert!(stage.windows(2).any(|bytes| bytes == [0x0f, 0x83]));
        assert!(stage.windows(3).any(|bytes| bytes == [0xc1, 0xe0, 0x02]));
        assert!(stage.windows(3).any(|bytes| bytes == [0x0f, 0xb6, 0x80]));
        assert!(stage.windows(2).any(|bytes| bytes == [0x89, 0x99]));
        assert!(
            stage
                .windows(27)
                .any(|bytes| bytes == b"x86-64 index out of bounds\0")
        );

        let bounds = check_source(
            "fn main(){var bytes:[u8;2]=[u8(10),u8(20)] var index:u32=2 print(bytes[index])}",
        )
        .unwrap();
        let image = compile_x86_64(&bounds).unwrap();
        assert_eq!(
            image[BOOT_SECTOR_BYTES..]
                .windows(27)
                .filter(|bytes| *bytes == b"x86-64 index out of bounds\0")
                .count(),
            1
        );
    }

    #[test]
    fn x86_64_device_io_requires_explicit_authority_and_emits_exact_port_instructions() {
        let program = check_source(
            "fn probe()->u8 uses DeviceIo{var status:u8=0 unsafe uses DeviceIo{status=Port.read_u8(u16(146)) Port.write_u8(u16(233),u8(80))} return status} fn main(){var ignored:u8=probe() print(\"ort I/O authorized in x86-64\")}",
        )
        .unwrap();
        let first = compile_x86_64(&program).unwrap();
        let second = compile_x86_64(&program).unwrap();
        assert_eq!(first, second);
        let stage = &first[BOOT_SECTOR_BYTES..];
        assert!(
            stage
                .windows(6)
                .any(|bytes| bytes == [0x89, 0xc2, 0xec, 0x0f, 0xb6, 0xc0])
        );
        assert!(
            stage
                .windows(4)
                .any(|bytes| bytes == [0x89, 0xd8, 0xee, 0x0f])
        );

        let outside = check_source("fn main(){var byte:u8=Port.read_u8(u16(146))}").unwrap_err();
        assert!(outside.message.contains("requires an `unsafe` block"));
        let implicit =
            check_source("fn main(){unsafe\n{var byte:u8=Port.read_u8(u16(146))}}").unwrap_err();
        assert!(implicit.message.contains("explicit `unsafe uses DeviceIo`"));
    }
}
