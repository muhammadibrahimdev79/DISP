//! Direct AArch64 QEMU `virt-8.2` Image generation.

use crate::{
    ast::{
        AssignmentOperator, BinaryOperator, Block, Capability, Expr, Expression, Function, Program,
        Statement, TypeQualifier, UnaryOperator,
    },
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    freestanding::transactional_write,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const IMAGE_HEADER_BYTES: usize = 64;
const IMAGE_TEXT_OFFSET: u64 = 0x0008_0000;
const IMAGE_FLAGS: u64 = 0x2;
const IMAGE_MAGIC: u32 = 0x644d_5241;
const IMAGE_LOAD_ADDRESS: u64 = 0x4008_0000;
const MAX_DTB_BYTES: u32 = 2 * 1024 * 1024;
const MAX_STATIC_STRING_BYTES: usize = 64 * 1024;
const MAX_IMAGE_BYTES: usize = 256 * 1024;
const MAX_LOCAL_BYTES: u32 = 4 * 1024;
const STACK_BYTES: usize = 16 * 1024;
const EXCEPTION_VECTOR_ALIGNMENT: usize = 2 * 1024;
const EXCEPTION_VECTOR_SLOTS: usize = 16;
const EXCEPTION_VECTOR_SLOT_BYTES: usize = 128;
const EXCEPTION_PROBE_IMAGE_OFFSET: usize = IMAGE_HEADER_BYTES + 16 * 4;
const EXCEPTION_PROBE_INSTRUCTION: u32 = 0xd503_201f; // nop
const PAGE_BYTES: usize = 4 * 1024;
const PAGE_TABLE_ENTRIES: usize = 512;
const PAGE_TABLE_COUNT: usize = 5;
const MMU_PROBE_IMAGE_OFFSET: usize = IMAGE_HEADER_BYTES + 54 * 4;
const MMU_PROBE_INSTRUCTION: u32 = 0xd503_201f; // nop

type Label = usize;

#[derive(Clone, Copy)]
enum FixupKind {
    Branch,
    BranchLink,
    Conditional(u32),
    CompareZero { register: u32, nonzero: bool },
    TestNonzero { register: u32, bit: u32 },
    Address { register: u32 },
}

struct Fixup {
    at: usize,
    target: Label,
    kind: FixupKind,
    span: Span,
}

#[derive(Default)]
struct Assembler {
    bytes: Vec<u8>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
}

impl Assembler {
    fn label(&mut self) -> Label {
        let label = self.labels.len();
        self.labels.push(None);
        label
    }

    fn bind(&mut self, label: Label) {
        assert!(self.labels[label].replace(self.bytes.len()).is_none());
    }

    fn bound(&self, label: Label) -> usize {
        self.labels[label].expect("AArch64 layout label must be bound")
    }

    fn emit(&mut self, instruction: u32) {
        self.bytes.extend_from_slice(&instruction.to_le_bytes());
    }

    fn emit_data(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
    }

    fn align(&mut self, alignment: usize) {
        while !self.bytes.len().is_multiple_of(alignment) {
            self.bytes.push(0);
        }
    }

    fn align_after_prefix(&mut self, prefix: usize, alignment: usize) {
        while !(prefix + self.bytes.len()).is_multiple_of(alignment) {
            self.bytes.push(0);
        }
    }

    fn fixup(&mut self, target: Label, kind: FixupKind, span: Span) {
        let at = self.bytes.len();
        self.emit(0);
        self.fixups.push(Fixup {
            at,
            target,
            kind,
            span,
        });
    }

    fn branch(&mut self, target: Label, span: Span) {
        self.fixup(target, FixupKind::Branch, span);
    }

    fn branch_link(&mut self, target: Label, span: Span) {
        self.fixup(target, FixupKind::BranchLink, span);
    }

    fn conditional(&mut self, condition: u32, target: Label, span: Span) {
        self.fixup(target, FixupKind::Conditional(condition), span);
    }

    fn compare_zero(&mut self, register: u32, nonzero: bool, target: Label, span: Span) {
        self.fixup(target, FixupKind::CompareZero { register, nonzero }, span);
    }

    fn test_nonzero(&mut self, register: u32, bit: u32, target: Label, span: Span) {
        self.fixup(target, FixupKind::TestNonzero { register, bit }, span);
    }

    fn address(&mut self, register: u32, target: Label, span: Span) {
        self.fixup(target, FixupKind::Address { register }, span);
    }

    fn finish(mut self) -> Result<Vec<u8>, Diagnostic> {
        for fixup in self.fixups {
            let target = self.labels[fixup.target]
                .expect("every AArch64 label must be bound before resolution");
            let instruction = match fixup.kind {
                FixupKind::Branch => encode_branch(fixup.at, target, fixup.span)?,
                FixupKind::BranchLink => encode_branch(fixup.at, target, fixup.span)? | 0x8000_0000,
                FixupKind::Conditional(condition) => {
                    let field = signed_scaled_field(
                        fixup.at,
                        target,
                        19,
                        fixup.span,
                        "AArch64 conditional branch",
                    )?;
                    0x5400_0000 | (field << 5) | condition
                }
                FixupKind::CompareZero { register, nonzero } => {
                    let field = signed_scaled_field(
                        fixup.at,
                        target,
                        19,
                        fixup.span,
                        "AArch64 compare-and-branch",
                    )?;
                    (if nonzero { 0x3500_0000 } else { 0x3400_0000 }) | (field << 5) | register
                }
                FixupKind::TestNonzero { register, bit } => {
                    let field = signed_scaled_field(
                        fixup.at,
                        target,
                        14,
                        fixup.span,
                        "AArch64 test-and-branch",
                    )?;
                    0x3700_0000 | (bit << 19) | (field << 5) | register
                }
                FixupKind::Address { register } => {
                    encode_adr(register, fixup.at, target, fixup.span)?
                }
            };
            self.bytes[fixup.at..fixup.at + 4].copy_from_slice(&instruction.to_le_bytes());
        }
        Ok(self.bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScalarKind {
    U8,
    U16,
    U32,
    I32,
    Bool,
}

impl ScalarKind {
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

#[derive(Clone, Copy)]
struct Local {
    offset: u32,
    kind: ScalarKind,
}

#[derive(Clone, Copy)]
struct ArrayLocal {
    offset: u32,
    element: ScalarKind,
    length: usize,
}

impl ArrayLocal {
    fn element(self, index: usize) -> Local {
        Local {
            offset: self.offset + index as u32 * self.element.bytes(),
            kind: self.element,
        }
    }
}

#[derive(Clone, Copy)]
enum LocalValue {
    Scalar(Local),
    Array(ArrayLocal),
}

#[derive(Clone, Copy)]
struct LoopContext {
    continue_target: Label,
    break_target: Label,
}

struct StringData {
    label: Label,
    bytes: Vec<u8>,
}

#[derive(Clone)]
struct FunctionInfo {
    label: Label,
    parameters: Vec<(String, Local)>,
    frame: Vec<Local>,
    return_kind: Option<ScalarKind>,
}

#[derive(Clone, Copy)]
struct PageTableLayout {
    data_start: usize,
    page_tables_start: usize,
    root: usize,
    low_l2: usize,
    low_l3: usize,
    image_l2: usize,
    image_l3: usize,
}

struct ScalarCompiler {
    assembler: Assembler,
    scopes: Vec<HashMap<String, LocalValue>>,
    loops: Vec<LoopContext>,
    strings: Vec<StringData>,
    functions: HashMap<String, FunctionInfo>,
    preallocated: HashMap<(String, Span), LocalValue>,
    next_local: u32,
    string_bytes: usize,
    locals: Label,
    dtb_prelude: Label,
    dtb_failure: Label,
    arithmetic_failure: Label,
    stack_failure: Label,
    bounds_failure: Label,
    device_access_failure: Label,
    exception_level_failure: Label,
    synchronous_exception: Label,
    irq_exception: Label,
    fiq_exception: Label,
    system_error_exception: Label,
    memory_protection_failure: Label,
    exception_vectors: Label,
    data_start: Label,
    page_tables_start: Label,
    root_table: Label,
    low_l2_table: Label,
    low_l3_table: Label,
    image_l2_table: Label,
    image_l3_table: Label,
    image_end: Label,
    halt: Label,
    digit_buffer_end: Label,
    stack_floor: Label,
    stack_top: Label,
    current_function: String,
    current_return: Option<ScalarKind>,
    current_is_main: bool,
    device_io_depth: usize,
    main_span: Span,
}

impl ScalarCompiler {
    fn new(program: &Program, main_span: Span) -> Result<Self, Diagnostic> {
        let mut assembler = Assembler::default();
        let locals = assembler.label();
        let dtb_prelude = assembler.label();
        let dtb_failure = assembler.label();
        let arithmetic_failure = assembler.label();
        let stack_failure = assembler.label();
        let bounds_failure = assembler.label();
        let device_access_failure = assembler.label();
        let exception_level_failure = assembler.label();
        let synchronous_exception = assembler.label();
        let irq_exception = assembler.label();
        let fiq_exception = assembler.label();
        let system_error_exception = assembler.label();
        let memory_protection_failure = assembler.label();
        let exception_vectors = assembler.label();
        let data_start = assembler.label();
        let page_tables_start = assembler.label();
        let root_table = assembler.label();
        let low_l2_table = assembler.label();
        let low_l3_table = assembler.label();
        let image_l2_table = assembler.label();
        let image_l3_table = assembler.label();
        let image_end = assembler.label();
        let halt = assembler.label();
        let digit_buffer_end = assembler.label();
        let stack_floor = assembler.label();
        let stack_top = assembler.label();
        let mut compiler = Self {
            assembler,
            scopes: Vec::new(),
            loops: Vec::new(),
            strings: Vec::new(),
            functions: HashMap::new(),
            preallocated: HashMap::new(),
            next_local: 0,
            string_bytes: 0,
            locals,
            dtb_prelude,
            dtb_failure,
            arithmetic_failure,
            stack_failure,
            bounds_failure,
            device_access_failure,
            exception_level_failure,
            synchronous_exception,
            irq_exception,
            fiq_exception,
            system_error_exception,
            memory_protection_failure,
            exception_vectors,
            data_start,
            page_tables_start,
            root_table,
            low_l2_table,
            low_l3_table,
            image_l2_table,
            image_l3_table,
            image_end,
            halt,
            digit_buffer_end,
            stack_floor,
            stack_top,
            current_function: String::new(),
            current_return: None,
            current_is_main: true,
            device_io_depth: 0,
            main_span,
        };
        for function in &program.functions {
            let label = compiler.assembler.label();
            let mut parameters = Vec::new();
            let mut frame = Vec::new();
            for parameter in &function.parameters {
                let kind = scalar_type(&parameter.ty, "AArch64 function parameter")?;
                let local = Local {
                    offset: compiler.allocate_local(kind, parameter.ty.span)?,
                    kind,
                };
                parameters.push((parameter.name.clone(), local));
                frame.push(local);
            }
            compiler.preallocate_block(&function.name, &function.body, &mut frame)?;
            let return_kind = function
                .return_type
                .as_ref()
                .map(|return_type| scalar_type(return_type, "AArch64 function return"))
                .transpose()?;
            compiler.functions.insert(
                function.name.clone(),
                FunctionInfo {
                    label,
                    parameters,
                    frame,
                    return_kind,
                },
            );
        }
        Ok(compiler)
    }

    fn compile(mut self, program: &Program, main: &Function) -> Result<Vec<u8>, Diagnostic> {
        let code_start = self.assembler.label();
        let entry_after_dtb = self.assembler.label();
        self.assembler.bind(code_start);
        self.assembler.emit(0xd503_4fdf); // msr daifset,#0xf
        self.assembler.branch(self.dtb_prelude, self.main_span);
        self.assembler.bind(entry_after_dtb);
        self.assembler.emit(0xd503_201f); // nop; DTB prelude installed x19/x20
        self.assembler.address(21, self.stack_top, self.main_span);
        self.assembler.emit(0x9100_02bf); // mov sp,x21
        self.assembler.address(22, self.stack_floor, self.main_span);
        self.assembler
            .address(23, self.exception_vectors, self.main_span);
        self.assembler.emit(0xd538_4258); // mrs x24,CurrentEL
        self.assembler.emit(0xf100_131f); // cmp x24,#4 (EL1)
        let install_el1 = self.assembler.label();
        let vectors_installed = self.assembler.label();
        self.assembler.conditional(0, install_el1, self.main_span); // b.eq
        self.assembler.emit(0xf100_231f); // cmp x24,#8 (EL2)
        self.assembler
            .conditional(1, self.exception_level_failure, self.main_span); // b.ne
        self.assembler.emit(0xd51c_c017); // msr VBAR_EL2,x23
        self.assembler.branch(vectors_installed, self.main_span);
        self.assembler.bind(install_el1);
        self.assembler.emit(0xd518_c017); // msr VBAR_EL1,x23
        self.assembler.bind(vectors_installed);
        self.assembler.emit(0xd503_3fdf); // isb
        debug_assert_eq!(
            IMAGE_HEADER_BYTES + self.assembler.bytes.len(),
            EXCEPTION_PROBE_IMAGE_OFFSET
        );
        self.assembler.emit(EXCEPTION_PROBE_INSTRUCTION);

        self.assembler.address(28, code_start, self.main_span);
        self.assembler.address(25, self.root_table, self.main_span);
        self.load_immediate(0xff, 26); // MAIR Attr0: normal WBWA; Attr1 remains device nGnRnE
        self.assembler.emit(0xf100_131f); // cmp x24,#4 (EL1)
        let configure_el1 = self.assembler.label();
        let mmu_enabled = self.assembler.label();
        self.assembler.conditional(0, configure_el1, self.main_span); // b.eq

        self.assembler.emit(0xd51c_a21a); // msr MAIR_EL2,x26
        self.load_immediate(0x3520, 26); // 4 KiB, 32-bit VA/PA, inner-shareable WBWA
        self.assembler.emit(0xd51c_205a); // msr TCR_EL2,x26
        self.assembler.emit(0xd51c_2019); // msr TTBR0_EL2,x25
        self.assembler.emit(0xd503_3f9f); // dsb sy
        self.assembler.emit(0xd503_3fdf); // isb
        self.assembler.emit(0xd50c_871f); // tlbi alle2
        self.assembler.emit(0xd503_3f9f); // dsb sy
        self.assembler.emit(0xd503_3fdf); // isb
        self.assembler.emit(0xd53c_101a); // mrs x26,SCTLR_EL2
        self.load_immediate(0x0008_1005, 27); // WXN|I|C|M
        self.assembler.emit(0xaa1b_035a); // orr x26,x26,x27
        self.assembler.emit(0xd51c_101a); // msr SCTLR_EL2,x26
        self.assembler.emit(0xd503_3fdf); // isb
        self.assembler.branch(mmu_enabled, self.main_span);

        self.assembler.bind(configure_el1);
        self.assembler.emit(0xd518_a21a); // msr MAIR_EL1,x26
        self.load_immediate(0x0080_3520, 26); // EPD1 plus 4 KiB 32-bit TTBR0 regime
        self.assembler.emit(0xd518_205a); // msr TCR_EL1,x26
        self.assembler.emit(0xd518_2019); // msr TTBR0_EL1,x25
        self.assembler.emit(0xd503_3f9f); // dsb sy
        self.assembler.emit(0xd503_3fdf); // isb
        self.assembler.emit(0xd508_871f); // tlbi vmalle1
        self.assembler.emit(0xd503_3f9f); // dsb sy
        self.assembler.emit(0xd503_3fdf); // isb
        self.assembler.emit(0xd538_101a); // mrs x26,SCTLR_EL1
        self.load_immediate(0x0008_1005, 27); // WXN|I|C|M
        self.assembler.emit(0xaa1b_035a); // orr x26,x26,x27
        self.assembler.emit(0xd518_101a); // msr SCTLR_EL1,x26
        self.assembler.emit(0xd503_3fdf); // isb

        self.assembler.bind(mmu_enabled);
        debug_assert_eq!(
            IMAGE_HEADER_BYTES + self.assembler.bytes.len(),
            MMU_PROBE_IMAGE_OFFSET
        );
        self.assembler.emit(MMU_PROBE_INSTRUCTION);
        self.compile_function(main, true)?;
        self.assembler.branch(self.halt, self.main_span);
        for function in &program.functions {
            if function.name != "main" {
                self.compile_function(function, false)?;
            }
        }

        self.assembler.bind(self.arithmetic_failure);
        self.emit_print_literal(b"[DISP arithmetic fault]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.stack_failure);
        self.emit_print_literal(b"[DISP stack exhausted]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.bounds_failure);
        self.emit_print_literal(b"[DISP index out of bounds]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.device_access_failure);
        self.emit_print_literal(b"[DISP device access fault]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.exception_level_failure);
        self.emit_print_literal(b"[DISP unsupported exception level]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.synchronous_exception);
        self.assembler.emit(0xf100_131f); // cmp x24,#4 (EL1)
        let read_el1_syndrome = self.assembler.label();
        let classify_syndrome = self.assembler.label();
        self.assembler
            .conditional(0, read_el1_syndrome, self.main_span); // b.eq
        self.assembler.emit(0xd53c_5200); // mrs x0,ESR_EL2
        self.assembler.branch(classify_syndrome, self.main_span);
        self.assembler.bind(read_el1_syndrome);
        self.assembler.emit(0xd538_5200); // mrs x0,ESR_EL1
        self.assembler.bind(classify_syndrome);
        self.assembler.emit(0xd35a_fc00); // lsr x0,x0,#26 (exception class)
        self.assembler.emit(0xf100_941f); // cmp x0,#0x25 (current-EL data abort)
        self.assembler
            .conditional(0, self.memory_protection_failure, self.main_span); // b.eq
        self.emit_print_literal(b"[DISP synchronous exception]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.irq_exception);
        self.emit_print_literal(b"[DISP IRQ exception]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.fiq_exception);
        self.emit_print_literal(b"[DISP FIQ exception]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.system_error_exception);
        self.emit_print_literal(b"[DISP system error exception]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.memory_protection_failure);
        self.emit_print_literal(b"[DISP memory protection fault]\r\n", self.main_span)?;
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.halt);
        self.assembler.emit(0xd503_207f); // wfi
        self.assembler.branch(self.halt, self.main_span);

        self.assembler.bind(self.dtb_prelude);
        self.emit_dtb_prelude(entry_after_dtb, code_start)?;
        self.assembler.bind(self.dtb_failure);
        self.assembler.emit(0xd503_207f); // fail closed without an unverified device
        self.assembler.branch(self.dtb_failure, self.main_span);

        self.assembler
            .align_after_prefix(IMAGE_HEADER_BYTES, EXCEPTION_VECTOR_ALIGNMENT);
        self.assembler.bind(self.exception_vectors);
        let handlers = [
            self.synchronous_exception,
            self.irq_exception,
            self.fiq_exception,
            self.system_error_exception,
        ];
        for slot in 0..EXCEPTION_VECTOR_SLOTS {
            self.assembler
                .branch(handlers[slot % handlers.len()], self.main_span);
            self.assembler
                .emit_data(&[0; EXCEPTION_VECTOR_SLOT_BYTES - 4]);
        }

        self.assembler
            .align_after_prefix(IMAGE_HEADER_BYTES, PAGE_BYTES);
        self.assembler.bind(self.data_start);
        for string in self.strings {
            self.assembler.bind(string.label);
            self.assembler.emit_data(&string.bytes);
            self.assembler.emit_data(&[0]);
        }
        self.assembler.align(16);
        self.assembler.emit_data(&[0; 16]);
        self.assembler.bind(self.digit_buffer_end);
        self.assembler.align(4);
        self.assembler.bind(self.locals);
        self.assembler.emit_data(&vec![0; self.next_local as usize]);
        self.assembler.align(16);
        self.assembler.bind(self.stack_floor);
        self.assembler.emit_data(&vec![0; STACK_BYTES]);
        self.assembler.bind(self.stack_top);
        self.assembler
            .align_after_prefix(IMAGE_HEADER_BYTES, PAGE_BYTES);
        self.assembler.bind(self.page_tables_start);
        for table in [
            self.root_table,
            self.low_l2_table,
            self.low_l3_table,
            self.image_l2_table,
            self.image_l3_table,
        ] {
            self.assembler.bind(table);
            self.assembler.emit_data(&[0; PAGE_BYTES]);
        }
        self.assembler.bind(self.image_end);
        let layout = PageTableLayout {
            data_start: self.assembler.bound(self.data_start),
            page_tables_start: self.assembler.bound(self.page_tables_start),
            root: self.assembler.bound(self.root_table),
            low_l2: self.assembler.bound(self.low_l2_table),
            low_l3: self.assembler.bound(self.low_l3_table),
            image_l2: self.assembler.bound(self.image_l2_table),
            image_l3: self.assembler.bound(self.image_l3_table),
        };
        let mut payload = self.assembler.finish()?;
        install_page_tables(&mut payload, layout, self.main_span)?;
        Ok(payload)
    }

    fn emit_ldr_w(&mut self, target: u32, base: u32, offset: u32) {
        debug_assert!(offset.is_multiple_of(4) && offset / 4 < 4096);
        self.assembler
            .emit(0xb940_0000 | ((offset / 4) << 10) | (base << 5) | target);
    }

    fn emit_ldrb(&mut self, target: u32, base: u32, offset: u32) {
        debug_assert!(offset < 4096);
        self.assembler
            .emit(0x3940_0000 | (offset << 10) | (base << 5) | target);
    }

    fn emit_ldrb_post_increment(&mut self, target: u32, base: u32) {
        self.assembler.emit(0x3840_1400 | (base << 5) | target);
    }

    fn emit_reverse_w(&mut self, target: u32, source: u32) {
        self.assembler.emit(0x5ac0_0800 | (source << 5) | target);
    }

    fn emit_add_x_register(&mut self, target: u32, left: u32, right: u32) {
        self.assembler
            .emit(0x8b00_0000 | (right << 16) | (left << 5) | target);
    }

    fn emit_add_x_immediate(&mut self, target: u32, source: u32, value: u32) {
        debug_assert!(value < 4096);
        self.assembler
            .emit(0x9100_0000 | (value << 10) | (source << 5) | target);
    }

    fn emit_sub_x_immediate(&mut self, target: u32, source: u32, value: u32) {
        debug_assert!(value < 4096);
        self.assembler
            .emit(0xd100_0000 | (value << 10) | (source << 5) | target);
    }

    fn emit_add_w_immediate(&mut self, target: u32, source: u32, value: u32) {
        debug_assert!(value < 4096);
        self.assembler
            .emit(0x1100_0000 | (value << 10) | (source << 5) | target);
    }

    fn emit_sub_w_immediate(&mut self, target: u32, source: u32, value: u32) {
        debug_assert!(value < 4096);
        self.assembler
            .emit(0x5100_0000 | (value << 10) | (source << 5) | target);
    }

    fn emit_compare_x_registers(&mut self, left: u32, right: u32) {
        self.assembler
            .emit(0xeb00_001f | (right << 16) | (left << 5));
    }

    fn emit_compare_w_registers(&mut self, left: u32, right: u32) {
        self.assembler
            .emit(0x6b00_001f | (right << 16) | (left << 5));
    }

    fn emit_compare_x_immediate(&mut self, register: u32, value: u32) {
        debug_assert!(value < 4096);
        self.assembler
            .emit(0xf100_001f | (value << 10) | (register << 5));
    }

    fn emit_compare_w_immediate(&mut self, register: u32, value: u32) {
        debug_assert!(value < 4096);
        self.assembler
            .emit(0x7100_001f | (value << 10) | (register << 5));
    }

    fn emit_move_x(&mut self, target: u32, source: u32) {
        self.assembler.emit(0xaa00_03e0 | (source << 16) | target);
    }

    fn emit_move_w(&mut self, target: u32, source: u32) {
        self.assembler.emit(0x2a00_03e0 | (source << 16) | target);
    }

    fn emit_orr_x(&mut self, target: u32, left: u32, right: u32) {
        self.assembler
            .emit(0xaa00_0000 | (right << 16) | (left << 5) | target);
    }

    fn emit_orr_w(&mut self, target: u32, left: u32, right: u32) {
        self.assembler
            .emit(0x2a00_0000 | (right << 16) | (left << 5) | target);
    }

    fn emit_extract_x(&mut self, target: u32, source: u32, least_bit: u32, width: u32) {
        debug_assert!(width > 0 && least_bit + width <= 64);
        self.assembler.emit(
            0xd340_0000
                | (least_bit << 16)
                | ((least_bit + width - 1) << 10)
                | (source << 5)
                | target,
        );
    }

    fn emit_shift_right_x(&mut self, target: u32, source: u32, shift: u32) {
        self.assembler
            .emit(0xd340_0000 | (shift << 16) | (63 << 10) | (source << 5) | target);
    }

    fn emit_shift_left_x(&mut self, target: u32, source: u32, shift: u32) {
        let rotate = (64 - shift) & 63;
        self.assembler
            .emit(0xd340_0000 | (rotate << 16) | ((63 - shift) << 10) | (source << 5) | target);
    }

    fn emit_store_x_indexed(&mut self, source: u32, base: u32, index: u32) {
        self.assembler
            .emit(0xf820_7800 | (index << 16) | (base << 5) | source);
    }

    fn load_immediate_u64(&mut self, value: u64, register: u32) {
        self.assembler
            .emit(0xd280_0000 | (((value as u32) & 0xffff) << 5) | register);
        for half in 1..4 {
            let part = ((value >> (half * 16)) & 0xffff) as u32;
            if part != 0 {
                self.assembler
                    .emit(0xf280_0000 | (half << 21) | (part << 5) | register);
            }
        }
    }

    fn branch_if_c_string_equals(&mut self, pointer: u32, end: u32, value: &[u8], equal: Label) {
        debug_assert!(value.last() == Some(&0) && value.len() < 4096);
        let mismatch = self.assembler.label();
        self.emit_add_x_immediate(15, pointer, value.len() as u32);
        self.emit_compare_x_registers(15, pointer);
        self.assembler.conditional(3, mismatch, self.main_span); // b.lo
        self.emit_compare_x_registers(15, end);
        self.assembler.conditional(8, mismatch, self.main_span); // b.hi
        for (offset, byte) in value.iter().copied().enumerate() {
            self.emit_ldrb(15, pointer, offset as u32);
            self.emit_compare_w_immediate(15, byte.into());
            self.assembler.conditional(1, mismatch, self.main_span); // b.ne
        }
        self.assembler.branch(equal, self.main_span);
        self.assembler.bind(mismatch);
    }

    fn set_dtb_node_flag(&mut self, bit: u32) {
        self.load_immediate(1 << bit, 14);
        self.emit_orr_w(9, 9, 14);
    }

    fn emit_dtb_prelude(&mut self, resume: Label, code_start: Label) -> Result<(), Diagnostic> {
        let token_loop = self.assembler.label();
        let begin_node = self.assembler.label();
        let end_node = self.assembler.label();
        let property = self.assembler.label();
        let nop = self.assembler.label();
        let finish = self.assembler.label();

        self.emit_compare_x_immediate(0, 0);
        self.assembler
            .conditional(0, self.dtb_failure, self.main_span);
        self.assembler.emit(0xf240_081f); // tst x0,#7
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);

        self.emit_ldr_w(14, 0, 0);
        self.emit_reverse_w(14, 14);
        self.load_immediate(0xd00d_feed, 15);
        self.emit_compare_w_registers(14, 15);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_ldr_w(14, 0, 4); // total size
        self.emit_reverse_w(14, 14);
        self.emit_compare_w_immediate(14, 40);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span); // b.lo
        self.load_immediate(MAX_DTB_BYTES, 15);
        self.emit_compare_w_registers(14, 15);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span); // b.hi
        self.emit_add_x_register(1, 0, 14); // complete DTB end
        self.emit_compare_x_registers(1, 0);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);

        self.emit_ldr_w(14, 0, 20); // version
        self.emit_reverse_w(14, 14);
        self.emit_compare_w_immediate(14, 17);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_ldr_w(14, 0, 24); // last compatible version
        self.emit_reverse_w(14, 14);
        self.emit_compare_w_immediate(14, 17);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);

        self.emit_ldr_w(14, 0, 8); // structure offset
        self.emit_reverse_w(14, 14);
        self.emit_compare_w_immediate(14, 40);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_add_x_register(2, 0, 14);
        self.emit_compare_x_registers(2, 0);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_ldr_w(15, 0, 36); // structure size
        self.emit_reverse_w(15, 15);
        self.emit_compare_w_immediate(15, 4);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_add_x_register(3, 2, 15);
        self.emit_compare_x_registers(3, 2);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_compare_x_registers(3, 1);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);

        self.emit_ldr_w(14, 0, 12); // strings offset
        self.emit_reverse_w(14, 14);
        self.emit_compare_w_immediate(14, 40);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_add_x_register(4, 0, 14);
        self.emit_compare_x_registers(4, 0);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_ldr_w(15, 0, 32); // strings size
        self.emit_reverse_w(15, 15);
        self.emit_add_x_register(5, 4, 15);
        self.emit_compare_x_registers(5, 4);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_compare_x_registers(5, 1);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);

        self.load_immediate(u32::MAX, 6); // depth before the root node
        self.load_immediate(0, 7); // root #address-cells
        self.load_immediate(0, 8); // root #size-cells
        self.load_immediate(0, 9); // current direct-child flags
        self.load_immediate_u64(0, 10); // current reg base
        self.load_immediate_u64(0, 11); // current reg size
        self.load_immediate_u64(0, 12); // discovered UART
        self.load_immediate(0, 13); // image-containing RAM node seen

        self.assembler.bind(token_loop);
        self.emit_add_x_immediate(16, 2, 4);
        self.emit_compare_x_registers(16, 2);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_compare_x_registers(16, 3);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        self.emit_ldr_w(14, 2, 0);
        self.emit_reverse_w(14, 14);
        self.emit_add_x_immediate(2, 2, 4);
        for (token, target) in [
            (1, begin_node),
            (2, end_node),
            (3, property),
            (4, nop),
            (9, finish),
        ] {
            self.emit_compare_w_immediate(14, token);
            self.assembler.conditional(0, target, self.main_span);
        }
        self.assembler.branch(self.dtb_failure, self.main_span);

        self.assembler.bind(begin_node);
        self.emit_add_w_immediate(6, 6, 1);
        self.emit_compare_w_immediate(6, 64);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        let keep_node_state = self.assembler.label();
        self.emit_compare_w_immediate(6, 1);
        self.assembler
            .conditional(1, keep_node_state, self.main_span);
        self.load_immediate(0, 9);
        self.load_immediate_u64(0, 10);
        self.load_immediate_u64(0, 11);
        self.assembler.bind(keep_node_state);
        let scan_node_name = self.assembler.label();
        self.assembler.bind(scan_node_name);
        self.emit_compare_x_registers(2, 3);
        self.assembler
            .conditional(2, self.dtb_failure, self.main_span);
        self.emit_ldrb_post_increment(14, 2);
        self.assembler
            .compare_zero(14, true, scan_node_name, self.main_span);
        self.emit_add_x_immediate(2, 2, 3);
        self.emit_shift_right_x(2, 2, 2);
        self.emit_shift_left_x(2, 2, 2);
        self.emit_compare_x_registers(2, 3);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        self.assembler.branch(token_loop, self.main_span);

        self.assembler.bind(end_node);
        self.emit_compare_w_immediate(6, 64);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        let decrement_depth = self.assembler.label();
        self.emit_compare_w_immediate(6, 1);
        self.assembler
            .conditional(1, decrement_depth, self.main_span);

        let skip_uart_commit = self.assembler.label();
        let commit_uart = self.assembler.label();
        self.assembler
            .test_nonzero(9, 0, commit_uart, self.main_span);
        self.assembler.branch(skip_uart_commit, self.main_span);
        self.assembler.bind(commit_uart);
        let uart_has_reg = self.assembler.label();
        self.assembler
            .test_nonzero(9, 2, uart_has_reg, self.main_span);
        self.assembler.branch(skip_uart_commit, self.main_span);
        self.assembler.bind(uart_has_reg);
        self.emit_compare_x_immediate(12, 0);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span); // ambiguous duplicate
        self.emit_compare_x_immediate(10, 0);
        self.assembler
            .conditional(0, self.dtb_failure, self.main_span);
        self.assembler.emit(0xf240_2d5f); // tst x10,#0xfff
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.load_immediate(0xffff_ffff, 15);
        self.emit_compare_x_registers(10, 15);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        self.load_immediate(0x1000, 15);
        self.emit_compare_x_registers(11, 15);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_extract_x(14, 10, 30, 9);
        self.emit_compare_x_immediate(14, 1); // image root entry is reserved
        self.assembler
            .conditional(0, self.dtb_failure, self.main_span);
        self.emit_move_x(12, 10);
        self.assembler.bind(skip_uart_commit);

        let skip_memory_commit = self.assembler.label();
        let commit_memory = self.assembler.label();
        self.assembler
            .test_nonzero(9, 1, commit_memory, self.main_span);
        self.assembler.branch(skip_memory_commit, self.main_span);
        self.assembler.bind(commit_memory);
        let memory_has_reg = self.assembler.label();
        self.assembler
            .test_nonzero(9, 2, memory_has_reg, self.main_span);
        self.assembler.branch(skip_memory_commit, self.main_span);
        self.assembler.bind(memory_has_reg);
        self.assembler.address(14, code_start, self.main_span);
        self.emit_sub_x_immediate(14, 14, IMAGE_HEADER_BYTES as u32);
        self.emit_compare_x_registers(14, 10);
        self.assembler
            .conditional(3, skip_memory_commit, self.main_span); // image starts below node
        self.emit_add_x_register(16, 10, 11);
        self.emit_compare_x_registers(16, 10);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.assembler.address(15, self.image_end, self.main_span);
        self.emit_compare_x_registers(15, 16);
        self.assembler
            .conditional(8, skip_memory_commit, self.main_span);
        self.load_immediate(1, 13);
        self.assembler.bind(skip_memory_commit);

        self.assembler.bind(decrement_depth);
        self.emit_sub_w_immediate(6, 6, 1);
        self.assembler.branch(token_loop, self.main_span);

        self.assembler.bind(property);
        self.emit_add_x_immediate(16, 2, 8);
        self.emit_compare_x_registers(16, 2);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_compare_x_registers(16, 3);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        self.emit_ldr_w(14, 2, 0); // value length
        self.emit_reverse_w(14, 14);
        self.emit_ldr_w(15, 2, 4); // string-table name offset
        self.emit_reverse_w(15, 15);
        self.emit_add_x_register(18, 16, 14); // unaligned value end
        self.emit_compare_x_registers(18, 16);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_add_x_immediate(17, 18, 3);
        self.emit_compare_x_registers(17, 18);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_shift_right_x(17, 17, 2);
        self.emit_shift_left_x(17, 17, 2);
        self.emit_compare_x_registers(17, 3);
        self.assembler
            .conditional(8, self.dtb_failure, self.main_span);
        self.emit_add_x_register(0, 4, 15); // property-name pointer
        self.emit_compare_x_registers(0, 4);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        self.emit_compare_x_registers(0, 5);
        self.assembler
            .conditional(2, self.dtb_failure, self.main_span);
        self.emit_move_x(1, 0);
        let scan_property_name = self.assembler.label();
        self.assembler.bind(scan_property_name);
        self.emit_compare_x_registers(1, 5);
        self.assembler
            .conditional(2, self.dtb_failure, self.main_span);
        self.emit_ldrb_post_increment(15, 1);
        self.assembler
            .compare_zero(15, true, scan_property_name, self.main_span);

        let address_cells = self.assembler.label();
        let size_cells = self.assembler.label();
        let compatible = self.assembler.label();
        let device_type = self.assembler.label();
        let reg = self.assembler.label();
        let property_done = self.assembler.label();
        self.branch_if_c_string_equals(0, 5, b"#address-cells\0", address_cells);
        self.branch_if_c_string_equals(0, 5, b"#size-cells\0", size_cells);
        self.branch_if_c_string_equals(0, 5, b"compatible\0", compatible);
        self.branch_if_c_string_equals(0, 5, b"device_type\0", device_type);
        self.branch_if_c_string_equals(0, 5, b"reg\0", reg);
        self.assembler.branch(property_done, self.main_span);

        self.assembler.bind(address_cells);
        self.emit_compare_w_immediate(6, 0);
        self.assembler.conditional(1, property_done, self.main_span);
        self.emit_compare_w_immediate(14, 4);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_ldr_w(15, 16, 0);
        self.emit_reverse_w(15, 15);
        self.emit_compare_w_immediate(15, 2);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_move_w(7, 15);
        self.assembler.branch(property_done, self.main_span);

        self.assembler.bind(size_cells);
        self.emit_compare_w_immediate(6, 0);
        self.assembler.conditional(1, property_done, self.main_span);
        self.emit_compare_w_immediate(14, 4);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_ldr_w(15, 16, 0);
        self.emit_reverse_w(15, 15);
        self.emit_compare_w_immediate(15, 2);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_move_w(8, 15);
        self.assembler.branch(property_done, self.main_span);

        self.assembler.bind(compatible);
        self.emit_compare_w_immediate(6, 1);
        self.assembler.conditional(1, property_done, self.main_span);
        self.emit_move_x(1, 16);
        let compatible_loop = self.assembler.label();
        let compatible_scan = self.assembler.label();
        let compatible_match = self.assembler.label();
        self.assembler.bind(compatible_loop);
        self.emit_compare_x_registers(1, 18);
        self.assembler.conditional(2, property_done, self.main_span);
        self.branch_if_c_string_equals(1, 18, b"arm,pl011\0", compatible_match);
        self.assembler.bind(compatible_scan);
        self.emit_compare_x_registers(1, 18);
        self.assembler
            .conditional(2, self.dtb_failure, self.main_span);
        self.emit_ldrb_post_increment(15, 1);
        self.assembler
            .compare_zero(15, true, compatible_scan, self.main_span);
        self.assembler.branch(compatible_loop, self.main_span);
        self.assembler.bind(compatible_match);
        self.set_dtb_node_flag(0);
        self.assembler.branch(property_done, self.main_span);

        self.assembler.bind(device_type);
        self.emit_compare_w_immediate(6, 1);
        self.assembler.conditional(1, property_done, self.main_span);
        self.emit_compare_w_immediate(14, 7);
        self.assembler.conditional(1, property_done, self.main_span);
        let memory_match = self.assembler.label();
        self.branch_if_c_string_equals(16, 18, b"memory\0", memory_match);
        self.assembler.branch(property_done, self.main_span);
        self.assembler.bind(memory_match);
        self.set_dtb_node_flag(1);
        self.assembler.branch(property_done, self.main_span);

        self.assembler.bind(reg);
        self.emit_compare_w_immediate(6, 1);
        self.assembler.conditional(1, property_done, self.main_span);
        self.emit_compare_w_immediate(7, 2);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_compare_w_immediate(8, 2);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_compare_w_immediate(14, 16);
        self.assembler
            .conditional(3, self.dtb_failure, self.main_span);
        // FDT property payloads are only four-byte aligned. Reconstruct each
        // two-cell value from aligned word loads so SCTLR alignment checking
        // cannot fault on a valid `reg` property.
        self.emit_ldr_w(10, 16, 0);
        self.emit_reverse_w(10, 10);
        self.emit_shift_left_x(10, 10, 32);
        self.emit_ldr_w(15, 16, 4);
        self.emit_reverse_w(15, 15);
        self.emit_orr_x(10, 10, 15);
        self.emit_ldr_w(11, 16, 8);
        self.emit_reverse_w(11, 11);
        self.emit_shift_left_x(11, 11, 32);
        self.emit_ldr_w(15, 16, 12);
        self.emit_reverse_w(15, 15);
        self.emit_orr_x(11, 11, 15);
        self.set_dtb_node_flag(2);
        self.assembler.branch(property_done, self.main_span);

        self.assembler.bind(property_done);
        self.emit_move_x(2, 17);
        self.assembler.branch(token_loop, self.main_span);

        self.assembler.bind(nop);
        self.assembler.branch(token_loop, self.main_span);

        self.assembler.bind(finish);
        self.emit_add_w_immediate(14, 6, 1);
        self.emit_compare_w_immediate(14, 0);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_compare_w_immediate(7, 2);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_compare_w_immediate(8, 2);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);
        self.emit_compare_x_immediate(12, 0);
        self.assembler
            .conditional(0, self.dtb_failure, self.main_span);
        self.emit_compare_w_immediate(13, 1);
        self.assembler
            .conditional(1, self.dtb_failure, self.main_span);

        self.emit_move_x(20, 12);
        self.assembler.address(25, self.root_table, self.main_span);
        self.assembler
            .address(26, self.low_l2_table, self.main_span);
        self.assembler
            .address(27, self.low_l3_table, self.main_span);
        self.emit_extract_x(14, 20, 30, 9);
        self.load_immediate_u64(3, 15);
        self.emit_orr_x(15, 26, 15);
        self.emit_store_x_indexed(15, 25, 14);
        self.emit_extract_x(14, 20, 21, 9);
        self.load_immediate_u64(3, 15);
        self.emit_orr_x(15, 27, 15);
        self.emit_store_x_indexed(15, 26, 14);
        self.emit_extract_x(14, 20, 12, 9);
        self.load_immediate_u64((1 << 53) | (1 << 54) | 0x607, 15);
        self.emit_orr_x(15, 20, 15);
        self.emit_store_x_indexed(15, 27, 14);
        self.assembler.address(19, self.locals, self.main_span);
        self.assembler.branch(resume, self.main_span);
        Ok(())
    }

    fn compile_function(&mut self, function: &Function, main: bool) -> Result<(), Diagnostic> {
        let info = self
            .functions
            .get(&function.name)
            .expect("validated AArch64 function is registered")
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
        if !main {
            self.push_register(30, function.span);
        }
        self.compile_block(&function.body)?;
        self.scopes.pop();
        if !main {
            if self.current_return.is_some() {
                self.assembler
                    .branch(self.arithmetic_failure, function.span);
            } else {
                self.emit_return();
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
                        "AArch64 locals require an explicit exact scalar or fixed-array annotation",
                    )
                })?;
                let value = value
                    .as_ref()
                    .ok_or_else(|| profile_error(span, "AArch64 locals must be initialized"))?;
                let local = *self
                    .preallocated
                    .get(&(self.current_function.clone(), span))
                    .expect("AArch64 local was preallocated");
                match local {
                    LocalValue::Scalar(local) => {
                        self.compile_expression(value, Some(local.kind))?;
                        self.store(local, 0);
                    }
                    LocalValue::Array(array) => {
                        let Expression::Array(values) = &value.node else {
                            return Err(profile_error(
                                value.span,
                                "AArch64 fixed arrays require an array-literal initializer",
                            ));
                        };
                        if values.len() != array.length {
                            return Err(profile_error(
                                value.span,
                                "AArch64 fixed-array initializer length does not match its annotation",
                            ));
                        }
                        for (index, value) in values.iter().enumerate() {
                            self.compile_expression(value, Some(array.element))?;
                            self.store(array.element(index), 0);
                        }
                    }
                }
                self.scopes
                    .last_mut()
                    .expect("AArch64 block scope exists")
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
                self.compile_expression(value, Some(local.kind))?;
                if *operator != AssignmentOperator::Assign {
                    if !local.kind.numeric() {
                        return Err(profile_error(
                            span,
                            "AArch64 boolean compound assignment is not permitted",
                        ));
                    }
                    self.load(local, 1);
                    let binary = match operator {
                        AssignmentOperator::Add => BinaryOperator::Add,
                        AssignmentOperator::Subtract => BinaryOperator::Subtract,
                        AssignmentOperator::Multiply => BinaryOperator::Multiply,
                        AssignmentOperator::Divide => BinaryOperator::Divide,
                        AssignmentOperator::Assign => unreachable!(),
                    };
                    self.emit_arithmetic(binary, local.kind, span)?;
                }
                self.store(local, 0);
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
                self.compile_expression(condition, Some(ScalarKind::Bool))?;
                let alternate = self.assembler.label();
                let end = self.assembler.label();
                self.assembler
                    .compare_zero(0, false, alternate, condition.span);
                self.compile_block(then_branch)?;
                self.assembler.branch(end, span);
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
                self.compile_expression(condition, Some(ScalarKind::Bool))?;
                self.assembler.compare_zero(0, false, end, condition.span);
                self.loops.push(LoopContext {
                    continue_target: start,
                    break_target: end,
                });
                self.compile_block(body)?;
                self.loops.pop();
                self.assembler.branch(start, span);
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
                self.assembler.branch(start, span);
                self.assembler.bind(end);
                Ok(())
            }
            Statement::Break => {
                let context = self.loops.last().copied().ok_or_else(|| {
                    profile_error(span, "AArch64 `break` requires an enclosing loop")
                })?;
                self.assembler.branch(context.break_target, span);
                Ok(())
            }
            Statement::Continue => {
                let context = self.loops.last().copied().ok_or_else(|| {
                    profile_error(span, "AArch64 `continue` requires an enclosing loop")
                })?;
                self.assembler.branch(context.continue_target, span);
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
                        "AArch64 unsafe regions require the explicit supported `DeviceIo` contract",
                    ));
                }
                self.device_io_depth += 1;
                let result = self.compile_block(body);
                self.device_io_depth -= 1;
                result
            }
            _ => Err(profile_error(
                span,
                "this statement is not yet available in the AArch64 scalar profile",
            )),
        }
    }

    fn compile_return(&mut self, value: Option<&Expr>, span: Span) -> Result<(), Diagnostic> {
        if self.current_is_main {
            if value.is_some() {
                return Err(profile_error(span, "AArch64 `main` cannot return a value"));
            }
            self.assembler.branch(self.halt, span);
            return Ok(());
        }
        match (self.current_return, value) {
            (Some(expected), Some(value)) => {
                self.compile_expression(value, Some(expected))?;
                self.emit_return();
            }
            (None, None) => self.emit_return(),
            (Some(_), None) => {
                return Err(profile_error(
                    span,
                    "AArch64 scalar function must return a value",
                ));
            }
            (None, Some(_)) => {
                return Err(profile_error(
                    span,
                    "AArch64 `Unit` function cannot return a value",
                ));
            }
        }
        Ok(())
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
                "AArch64 place assignment requires a fixed-array element",
            ));
        };
        let array = self.direct_array(object)?;
        self.compile_array_index(array, index)?;
        self.push_register(0, index.span);
        self.compile_expression(value, Some(array.element))?;
        self.pop_register(3);
        if operator != AssignmentOperator::Assign {
            if !array.element.numeric() {
                return Err(profile_error(
                    span,
                    "AArch64 boolean compound assignment is not permitted",
                ));
            }
            self.push_register(3, span);
            self.emit_array_address(array, 3);
            self.load_indexed(array.element, 1);
            let operation = match operator {
                AssignmentOperator::Add => BinaryOperator::Add,
                AssignmentOperator::Subtract => BinaryOperator::Subtract,
                AssignmentOperator::Multiply => BinaryOperator::Multiply,
                AssignmentOperator::Divide => BinaryOperator::Divide,
                AssignmentOperator::Assign => unreachable!(),
            };
            self.emit_arithmetic(operation, array.element, span)?;
            self.pop_register(3);
        }
        self.emit_array_address(array, 3);
        self.store_indexed(array.element, 0);
        Ok(())
    }

    fn compile_array_index(&mut self, array: ArrayLocal, index: &Expr) -> Result<(), Diagnostic> {
        let kind = self.compile_expression(index, None)?;
        if !kind.numeric() {
            return Err(profile_error(
                index.span,
                "AArch64 fixed-array index must be an integer",
            ));
        }
        let length = u32::try_from(array.length)
            .map_err(|_| profile_error(index.span, "AArch64 fixed-array length exceeds u32"))?;
        self.load_immediate(length, 2);
        self.assembler.emit(0x6b02_001f); // cmp w0,w2
        self.assembler
            .conditional(2, self.bounds_failure, index.span); // b.hs
        Ok(())
    }

    fn emit_array_address(&mut self, array: ArrayLocal, index_register: u32) {
        self.assembler
            .emit(0x9100_0000 | (array.offset << 10) | (19 << 5) | 2); // add x2,x19,#offset
        let shift = array.element.bytes().trailing_zeros();
        self.assembler
            .emit(0x8b00_0000 | (index_register << 16) | (shift << 10) | (2 << 5) | 2); // add x2,x2,xN,lsl #width
    }

    fn load_indexed(&mut self, kind: ScalarKind, register: u32) {
        let base = match kind {
            ScalarKind::U8 => 0x3940_0000,
            ScalarKind::U16 => 0x7940_0000,
            ScalarKind::U32 | ScalarKind::I32 | ScalarKind::Bool => 0xb940_0000,
        };
        self.assembler.emit(base | (2 << 5) | register);
    }

    fn store_indexed(&mut self, kind: ScalarKind, register: u32) {
        let base = match kind {
            ScalarKind::U8 => 0x3900_0000,
            ScalarKind::U16 => 0x7900_0000,
            ScalarKind::U32 | ScalarKind::I32 | ScalarKind::Bool => 0xb900_0000,
        };
        self.assembler.emit(base | (2 << 5) | register);
    }

    fn require_device_io(&self, span: Span) -> Result<(), Diagnostic> {
        if self.device_io_depth == 0 {
            Err(profile_error(
                span,
                "AArch64 memory-mapped access requires `unsafe uses DeviceIo { ... }`",
            ))
        } else {
            Ok(())
        }
    }

    fn mmio_kind(field: &str) -> Option<ScalarKind> {
        match field.strip_prefix("read_") {
            Some("u8") => Some(ScalarKind::U8),
            Some("u16") => Some(ScalarKind::U16),
            Some("u32") => Some(ScalarKind::U32),
            _ => None,
        }
    }

    fn mmio_write_kind(field: &str) -> Option<ScalarKind> {
        match field.strip_prefix("write_") {
            Some("u8") => Some(ScalarKind::U8),
            Some("u16") => Some(ScalarKind::U16),
            Some("u32") => Some(ScalarKind::U32),
            _ => None,
        }
    }

    fn validate_mmio_offset(&mut self, register: u32, kind: ScalarKind, span: Span) {
        let maximum = PAGE_BYTES as u32 - kind.bytes();
        self.load_immediate(maximum, 2);
        self.emit_compare_w_registers(register, 2);
        self.assembler
            .conditional(8, self.device_access_failure, span); // b.hi
        if kind.bytes() >= 2 {
            self.assembler
                .test_nonzero(register, 0, self.device_access_failure, span);
        }
        if kind.bytes() == 4 {
            self.assembler
                .test_nonzero(register, 1, self.device_access_failure, span);
        }
    }

    fn compile_mmio_read(
        &mut self,
        field: &str,
        arguments: &[Expr],
        expected: Option<ScalarKind>,
        span: Span,
    ) -> Result<ScalarKind, Diagnostic> {
        self.require_device_io(span)?;
        let kind = Self::mmio_kind(field).ok_or_else(|| {
            profile_error(
                span,
                format!("no bounded AArch64 MMIO input `Mmio.{field}`"),
            )
        })?;
        if arguments.len() != 1 {
            return Err(profile_error(
                span,
                format!("AArch64 `Mmio.{field}` requires exactly one `u16` offset argument"),
            ));
        }
        self.compile_expression(&arguments[0], Some(ScalarKind::U16))?;
        self.validate_mmio_offset(0, kind, arguments[0].span);
        self.assembler.emit(0x8b00_0282); // add x2,x20,x0
        self.assembler.emit(0xd503_33bf); // dmb osh
        self.assembler.emit(match kind {
            ScalarKind::U8 => 0x3940_0040,  // ldrb w0,[x2]
            ScalarKind::U16 => 0x7940_0040, // ldrh w0,[x2]
            ScalarKind::U32 => 0xb940_0040, // ldr w0,[x2]
            ScalarKind::I32 | ScalarKind::Bool => unreachable!(),
        });
        self.assembler.emit(0xd503_33bf); // dmb osh
        self.require_kind(kind, expected, span)?;
        Ok(kind)
    }

    fn compile_mmio_write(
        &mut self,
        field: &str,
        arguments: &[Expr],
        span: Span,
    ) -> Result<(), Diagnostic> {
        self.require_device_io(span)?;
        let kind = Self::mmio_write_kind(field).ok_or_else(|| {
            profile_error(
                span,
                format!("no bounded AArch64 MMIO output `Mmio.{field}`"),
            )
        })?;
        if arguments.len() != 2 {
            return Err(profile_error(
                span,
                format!(
                    "AArch64 `Mmio.{field}` requires one `u16` offset and one `{}` value",
                    kind_name(kind)
                ),
            ));
        }
        self.compile_expression(&arguments[0], Some(ScalarKind::U16))?;
        self.validate_mmio_offset(0, kind, arguments[0].span);
        self.push_register(0, arguments[0].span);
        self.compile_expression(&arguments[1], Some(kind))?;
        self.pop_register(1);
        self.assembler.emit(0x8b01_0282); // add x2,x20,x1
        self.assembler.emit(0xd503_33bf); // dmb osh
        self.assembler.emit(match kind {
            ScalarKind::U8 => 0x3900_0040,  // strb w0,[x2]
            ScalarKind::U16 => 0x7900_0040, // strh w0,[x2]
            ScalarKind::U32 => 0xb900_0040, // str w0,[x2]
            ScalarKind::I32 | ScalarKind::Bool => unreachable!(),
        });
        self.assembler.emit(0xd503_33bf); // dmb osh
        Ok(())
    }

    fn compile_call_statement(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let Expression::Call { callee, arguments } = &expression.node else {
            return Err(profile_error(
                expression.span,
                "AArch64 expression statements require a direct call",
            ));
        };
        if let Expression::FieldAccess { object, field, .. } = &callee.node
            && matches!(&object.node, Expression::Identifier(owner) if owner == "Mmio")
        {
            return self.compile_mmio_write(field, arguments, expression.span);
        }
        let Expression::Identifier(name) = &callee.node else {
            return Err(profile_error(
                expression.span,
                "AArch64 calls require a direct function name",
            ));
        };
        if name == "print" {
            return self.compile_print(expression);
        }
        self.compile_user_call(name, arguments, None, expression.span)?;
        Ok(())
    }

    fn compile_print(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let Expression::Call { callee, arguments } = &expression.node else {
            return Err(profile_error(
                expression.span,
                "AArch64 expression statements must be `print(\"text\")` calls",
            ));
        };
        if !matches!(&callee.node, Expression::Identifier(name) if name == "print")
            || arguments.len() != 1
        {
            return Err(profile_error(
                expression.span,
                "AArch64 output requires one string argument",
            ));
        }
        if let Expression::String(text) = &arguments[0].node {
            if text.as_bytes().contains(&0) {
                return Err(profile_error(
                    arguments[0].span,
                    "AArch64 output strings cannot contain NUL",
                ));
            }
            let mut bytes = text.as_bytes().to_vec();
            bytes.extend_from_slice(b"\r\n");
            self.emit_print_literal(&bytes, expression.span)
        } else {
            let kind = self.compile_expression(&arguments[0], None)?;
            self.emit_print_scalar(kind, arguments[0].span)
        }
    }

    fn emit_print_scalar(&mut self, kind: ScalarKind, span: Span) -> Result<(), Diagnostic> {
        if kind == ScalarKind::Bool {
            let false_value = self.assembler.label();
            let end = self.assembler.label();
            self.assembler.compare_zero(0, false, false_value, span);
            self.emit_print_literal(b"true\r\n", span)?;
            self.assembler.branch(end, span);
            self.assembler.bind(false_value);
            self.emit_print_literal(b"false\r\n", span)?;
            self.assembler.bind(end);
            return Ok(());
        }

        self.assembler.emit(0x2a00_03e3); // mov w3,w0
        if kind.signed() {
            let magnitude = self.assembler.label();
            self.assembler.test_nonzero(3, 31, magnitude, span);
            let ready = self.assembler.label();
            self.assembler.branch(ready, span);
            self.assembler.bind(magnitude);
            self.emit_uart_immediate(b'-', span);
            self.assembler.emit(0x4b03_03e3); // neg w3,w3 (MIN becomes 2147483648)
            self.assembler.bind(ready);
        }

        self.assembler.address(2, self.digit_buffer_end, span);
        self.assembler.address(7, self.digit_buffer_end, span);
        let convert = self.assembler.label();
        self.assembler.bind(convert);
        self.assembler.emit(0x5280_0144); // mov w4,#10
        self.assembler.emit(0x1ac4_0865); // udiv w5,w3,w4
        self.assembler.emit(0x1b04_8ca6); // msub w6,w5,w4,w3
        self.assembler.emit(0x1100_c0c6); // add w6,w6,#'0'
        self.assembler.emit(0xd100_0442); // sub x2,x2,#1
        self.assembler.emit(0x3900_0046); // strb w6,[x2]
        self.assembler.emit(0x2a05_03e3); // mov w3,w5
        self.assembler.compare_zero(3, true, convert, span);

        let output = self.assembler.label();
        let newline = self.assembler.label();
        self.assembler.bind(output);
        self.assembler.emit(0xeb07_005f); // cmp x2,x7
        self.assembler.conditional(0, newline, span); // b.eq
        self.assembler.emit(0x3840_1440); // ldrb w0,[x2],#1
        self.emit_uart_register(span);
        self.assembler.branch(output, span);
        self.assembler.bind(newline);
        self.emit_uart_immediate(b'\r', span);
        self.emit_uart_immediate(b'\n', span);
        Ok(())
    }

    fn emit_uart_immediate(&mut self, value: u8, span: Span) {
        self.load_immediate(value.into(), 0);
        self.emit_uart_register(span);
    }

    fn emit_uart_register(&mut self, span: Span) {
        let wait = self.assembler.label();
        self.assembler.bind(wait);
        self.assembler.emit(0xb940_1a81); // ldr w1,[x20,#0x18]
        self.assembler.test_nonzero(1, 5, wait, span);
        self.assembler.emit(0xb900_0280); // str w0,[x20]
    }

    fn emit_print_literal(&mut self, bytes: &[u8], span: Span) -> Result<(), Diagnostic> {
        self.string_bytes = self
            .string_bytes
            .checked_add(bytes.len() + 1)
            .ok_or_else(|| profile_error(span, "AArch64 static string size overflow"))?;
        if self.string_bytes > MAX_STATIC_STRING_BYTES {
            return Err(profile_error(
                span,
                format!(
                    "AArch64 static strings exceed the {MAX_STATIC_STRING_BYTES}-byte image limit"
                ),
            ));
        }
        let string = self.assembler.label();
        self.strings.push(StringData {
            label: string,
            bytes: bytes.to_vec(),
        });
        let loop_start = self.assembler.label();
        let wait = self.assembler.label();
        let end = self.assembler.label();
        self.assembler.address(2, string, span);
        self.assembler.bind(loop_start);
        self.assembler.emit(0x3840_1440); // ldrb w0,[x2],#1
        self.assembler.compare_zero(0, false, end, span);
        self.assembler.bind(wait);
        self.assembler.emit(0xb940_1a81); // ldr w1,[x20,#0x18]
        self.assembler.test_nonzero(1, 5, wait, span);
        self.assembler.emit(0xb900_0280); // str w0,[x20]
        self.assembler.branch(loop_start, span);
        self.assembler.bind(end);
        Ok(())
    }

    fn compile_expression(
        &mut self,
        expression: &Expr,
        expected: Option<ScalarKind>,
    ) -> Result<ScalarKind, Diagnostic> {
        match &expression.node {
            Expression::Integer(value) => {
                let kind = expected.unwrap_or(ScalarKind::U32);
                if !kind.numeric() {
                    return Err(profile_error(
                        expression.span,
                        "an integer literal cannot initialize an AArch64 boolean",
                    ));
                }
                let value = match kind {
                    ScalarKind::U8 => u8::try_from(*value).map(u32::from).map_err(|_| {
                        profile_error(expression.span, "AArch64 `u8` literal exceeds 255")
                    })?,
                    ScalarKind::U16 => u16::try_from(*value).map(u32::from).map_err(|_| {
                        profile_error(expression.span, "AArch64 `u16` literal exceeds 65535")
                    })?,
                    ScalarKind::U32 => u32::try_from(*value).map_err(|_| {
                        profile_error(expression.span, "AArch64 `u32` literal exceeds 4294967295")
                    })?,
                    ScalarKind::I32 => {
                        i32::try_from(*value)
                            .map(|value| value as u32)
                            .map_err(|_| {
                                profile_error(
                                    expression.span,
                                    "positive AArch64 `i32` literal exceeds 2147483647",
                                )
                            })?
                    }
                    ScalarKind::Bool => unreachable!(),
                };
                self.load_immediate(value, 0);
                Ok(kind)
            }
            Expression::Bool(value) => {
                self.require_kind(ScalarKind::Bool, expected, expression.span)?;
                self.load_immediate(u32::from(*value), 0);
                Ok(ScalarKind::Bool)
            }
            Expression::Identifier(name) => {
                let local = self.lookup(name, expression.span)?;
                self.require_kind(local.kind, expected, expression.span)?;
                self.load(local, 0);
                Ok(local.kind)
            }
            Expression::Index { object, index } => {
                let array = self.direct_array(object)?;
                self.compile_array_index(array, index)?;
                self.emit_array_address(array, 0);
                self.load_indexed(array.element, 0);
                self.require_kind(array.element, expected, expression.span)?;
                Ok(array.element)
            }
            Expression::Unary {
                operator: UnaryOperator::Not,
                operand,
            } => {
                self.compile_expression(operand, Some(ScalarKind::Bool))?;
                self.assembler.emit(0x7100_001f); // cmp w0,#0
                self.assembler.emit(0x1a9f_17e0); // cset w0,eq
                self.require_kind(ScalarKind::Bool, expected, expression.span)?;
                Ok(ScalarKind::Bool)
            }
            Expression::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => {
                if expected.is_some_and(|kind| kind != ScalarKind::I32) {
                    return Err(profile_error(
                        expression.span,
                        "AArch64 negation requires an `i32` value",
                    ));
                }
                if let Expression::Integer(value) = operand.node
                    && value == (i32::MAX as u128) + 1
                {
                    self.load_immediate(0x8000_0000, 0);
                    return Ok(ScalarKind::I32);
                }
                self.compile_expression(operand, Some(ScalarKind::I32))?;
                self.assembler.emit(0x6b00_03e0); // negs w0,w0
                self.assembler
                    .conditional(6, self.arithmetic_failure, expression.span); // b.vs
                Ok(ScalarKind::I32)
            }
            Expression::Binary {
                left,
                operator,
                right,
            } if matches!(operator, BinaryOperator::And | BinaryOperator::Or) => {
                self.compile_short_circuit(left, *operator, right, expression.span)?;
                self.require_kind(ScalarKind::Bool, expected, expression.span)?;
                Ok(ScalarKind::Bool)
            }
            Expression::Binary {
                left,
                operator,
                right,
            } => self.compile_binary(left, *operator, right, expected, expression.span),
            Expression::Call { callee, arguments } => {
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(owner) if owner == "Mmio")
                {
                    return self.compile_mmio_read(field, arguments, expected, expression.span);
                }
                let Expression::Identifier(name) = &callee.node else {
                    return Err(profile_error(
                        expression.span,
                        "AArch64 scalar constructors require a direct type name",
                    ));
                };
                if let Some(kind) = ScalarKind::from_name(name) {
                    if arguments.len() != 1 {
                        return Err(profile_error(
                            expression.span,
                            "AArch64 scalar constructors require exactly one argument",
                        ));
                    }
                    self.require_kind(kind, expected, expression.span)?;
                    self.compile_expression(&arguments[0], Some(kind))?;
                    return Ok(kind);
                }
                self.compile_user_call(name, arguments, expected, expression.span)?
                    .ok_or_else(|| {
                        profile_error(
                            expression.span,
                            format!("AArch64 `Unit` function `{name}` has no value"),
                        )
                    })
            }
            _ => Err(profile_error(
                expression.span,
                "this expression is not yet available in the AArch64 scalar profile",
            )),
        }
    }

    fn compile_binary(
        &mut self,
        left: &Expr,
        operator: BinaryOperator,
        right: &Expr,
        expected: Option<ScalarKind>,
        span: Span,
    ) -> Result<ScalarKind, Diagnostic> {
        let comparison = matches!(
            operator,
            BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
        );
        let operand_kind =
            self.infer_binary_kind(left, right, if comparison { None } else { expected }, span)?;
        if comparison {
            self.require_kind(ScalarKind::Bool, expected, span)?;
            if operand_kind == ScalarKind::Bool
                && !matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual)
            {
                return Err(profile_error(
                    span,
                    "AArch64 booleans support equality only",
                ));
            }
        } else if !operand_kind.numeric() {
            return Err(profile_error(
                span,
                "AArch64 boolean arithmetic is unavailable",
            ));
        }
        self.compile_expression(left, Some(operand_kind))?;
        self.push_register(0, left.span);
        self.compile_expression(right, Some(operand_kind))?;
        self.pop_register(1);
        if comparison {
            self.assembler.emit(0x6b00_003f); // cmp w1,w0
            let instruction = match (operator, operand_kind.signed()) {
                (BinaryOperator::Equal, _) => 0x1a9f_17e0,
                (BinaryOperator::NotEqual, _) => 0x1a9f_07e0,
                (BinaryOperator::Less, false) => 0x1a9f_27e0,
                (BinaryOperator::LessEqual, false) => 0x1a9f_87e0,
                (BinaryOperator::Greater, false) => 0x1a9f_97e0,
                (BinaryOperator::GreaterEqual, false) => 0x1a9f_37e0,
                (BinaryOperator::Less, true) => 0x1a9f_a7e0,
                (BinaryOperator::LessEqual, true) => 0x1a9f_c7e0,
                (BinaryOperator::Greater, true) => 0x1a9f_d7e0,
                (BinaryOperator::GreaterEqual, true) => 0x1a9f_b7e0,
                _ => unreachable!(),
            };
            self.assembler.emit(instruction);
            Ok(ScalarKind::Bool)
        } else {
            self.emit_arithmetic(operator, operand_kind, span)?;
            Ok(operand_kind)
        }
    }

    fn compile_short_circuit(
        &mut self,
        left: &Expr,
        operator: BinaryOperator,
        right: &Expr,
        span: Span,
    ) -> Result<(), Diagnostic> {
        self.compile_expression(left, Some(ScalarKind::Bool))?;
        let shortcut = self.assembler.label();
        let end = self.assembler.label();
        self.assembler
            .compare_zero(0, operator == BinaryOperator::Or, shortcut, left.span);
        self.compile_expression(right, Some(ScalarKind::Bool))?;
        self.assembler.branch(end, span);
        self.assembler.bind(shortcut);
        self.load_immediate(u32::from(operator == BinaryOperator::Or), 0);
        self.assembler.bind(end);
        Ok(())
    }

    fn emit_arithmetic(
        &mut self,
        operator: BinaryOperator,
        kind: ScalarKind,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if kind.signed() {
            return self.emit_signed_arithmetic(operator, span);
        }
        match operator {
            BinaryOperator::Add => {
                self.assembler.emit(0x2b00_0020); // adds w0,w1,w0
                self.assembler.conditional(2, self.arithmetic_failure, span); // b.cs
                self.check_narrow_maximum(kind, span);
            }
            BinaryOperator::Subtract => {
                self.assembler.emit(0x6b00_0020); // subs w0,w1,w0
                self.assembler.conditional(3, self.arithmetic_failure, span); // b.cc
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(0x9ba0_7c22); // umull x2,w1,w0
                self.assembler.emit(0xd360_fc43); // lsr x3,x2,#32
                self.assembler
                    .compare_zero(3, true, self.arithmetic_failure, span);
                self.assembler.emit(0x2a02_03e0); // mov w0,w2
                self.check_narrow_maximum(kind, span);
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler
                    .compare_zero(0, false, self.arithmetic_failure, span);
                if operator == BinaryOperator::Divide {
                    self.assembler.emit(0x1ac0_0820); // udiv w0,w1,w0
                } else {
                    self.assembler.emit(0x1ac0_0822); // udiv w2,w1,w0
                    self.assembler.emit(0x1b00_8440); // msub w0,w2,w0,w1
                }
            }
            _ => {
                return Err(profile_error(
                    span,
                    "operator is not AArch64 unsigned arithmetic",
                ));
            }
        }
        Ok(())
    }

    fn emit_signed_arithmetic(
        &mut self,
        operator: BinaryOperator,
        span: Span,
    ) -> Result<(), Diagnostic> {
        match operator {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                self.assembler.emit(if operator == BinaryOperator::Add {
                    0x2b00_0020 // adds w0,w1,w0
                } else {
                    0x6b00_0020 // subs w0,w1,w0
                });
                self.assembler.conditional(6, self.arithmetic_failure, span); // b.vs
            }
            BinaryOperator::Multiply => {
                self.assembler.emit(0x9b20_7c22); // smull x2,w1,w0
                self.assembler.emit(0x9340_7c43); // sxtw x3,w2
                self.assembler.emit(0xeb03_005f); // cmp x2,x3
                self.assembler.conditional(1, self.arithmetic_failure, span); // b.ne
                self.assembler.emit(0x2a02_03e0); // mov w0,w2
            }
            BinaryOperator::Divide | BinaryOperator::Remainder => {
                self.assembler
                    .compare_zero(0, false, self.arithmetic_failure, span);
                let safe = self.assembler.label();
                self.load_immediate(0x8000_0000, 2);
                self.assembler.emit(0x6b02_003f); // cmp w1,w2
                self.assembler.conditional(1, safe, span); // b.ne
                self.load_immediate(u32::MAX, 2);
                self.assembler.emit(0x6b02_001f); // cmp w0,w2
                self.assembler.conditional(0, self.arithmetic_failure, span); // b.eq
                self.assembler.bind(safe);
                if operator == BinaryOperator::Divide {
                    self.assembler.emit(0x1ac0_0c20); // sdiv w0,w1,w0
                } else {
                    self.assembler.emit(0x1ac0_0c22); // sdiv w2,w1,w0
                    self.assembler.emit(0x1b00_8440); // msub w0,w2,w0,w1
                }
            }
            _ => {
                return Err(profile_error(
                    span,
                    "operator is not AArch64 signed arithmetic",
                ));
            }
        }
        Ok(())
    }

    fn check_narrow_maximum(&mut self, kind: ScalarKind, span: Span) {
        let maximum = match kind {
            ScalarKind::U8 => Some(u8::MAX.into()),
            ScalarKind::U16 => Some(u16::MAX.into()),
            ScalarKind::U32 => None,
            ScalarKind::I32 | ScalarKind::Bool => unreachable!(),
        };
        if let Some(maximum) = maximum {
            self.load_immediate(maximum, 2);
            self.assembler.emit(0x6b02_001f); // cmp w0,w2
            self.assembler.conditional(8, self.arithmetic_failure, span); // b.hi
        }
    }

    fn infer_binary_kind(
        &self,
        left: &Expr,
        right: &Expr,
        expected: Option<ScalarKind>,
        span: Span,
    ) -> Result<ScalarKind, Diagnostic> {
        let left_kind = self.known_kind(left)?;
        let right_kind = self.known_kind(right)?;
        let mut selected = expected;
        for hint in [left_kind, right_kind].into_iter().flatten() {
            if selected.is_some_and(|selected| selected != hint) {
                return Err(profile_error(
                    span,
                    "AArch64 binary operands have different exact scalar types",
                ));
            }
            selected = Some(hint);
        }
        Ok(selected.unwrap_or(ScalarKind::U32))
    }

    fn known_kind(&self, expression: &Expr) -> Result<Option<ScalarKind>, Diagnostic> {
        match &expression.node {
            Expression::Bool(_) => Ok(Some(ScalarKind::Bool)),
            Expression::Integer(_) => Ok(None),
            Expression::Identifier(name) => Ok(Some(self.lookup(name, expression.span)?.kind)),
            Expression::Index { object, .. } => Ok(Some(self.direct_array(object)?.element)),
            Expression::Unary {
                operator: UnaryOperator::Not,
                ..
            } => Ok(Some(ScalarKind::Bool)),
            Expression::Unary {
                operator: UnaryOperator::Negate,
                ..
            } => Ok(Some(ScalarKind::I32)),
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
            } => Ok(Some(ScalarKind::Bool)),
            Expression::Binary { left, right, .. } => {
                let left = self.known_kind(left)?;
                let right = self.known_kind(right)?;
                match (left, right) {
                    (Some(left), Some(right)) if left != right => Err(profile_error(
                        expression.span,
                        "AArch64 expression mixes exact scalar types",
                    )),
                    (Some(kind), _) | (_, Some(kind)) => Ok(Some(kind)),
                    _ => Ok(None),
                }
            }
            Expression::Call { callee, .. } => match &callee.node {
                Expression::Identifier(name) => Ok(ScalarKind::from_name(name).or_else(|| {
                    self.functions
                        .get(name)
                        .and_then(|function| function.return_kind)
                })),
                Expression::FieldAccess { object, field, .. } if matches!(&object.node, Expression::Identifier(owner) if owner == "Mmio") => {
                    Ok(Self::mmio_kind(field))
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    fn compile_user_call(
        &mut self,
        name: &str,
        arguments: &[Expr],
        expected: Option<ScalarKind>,
        span: Span,
    ) -> Result<Option<ScalarKind>, Diagnostic> {
        let info =
            self.functions.get(name).cloned().ok_or_else(|| {
                profile_error(span, format!("`{name}` is not an AArch64 function"))
            })?;
        if name == "main" {
            return Err(profile_error(
                span,
                "AArch64 `main` is an entry point and cannot be called",
            ));
        }
        if arguments.len() != info.parameters.len() {
            return Err(profile_error(
                span,
                format!(
                    "AArch64 function `{name}` expects {} arguments but received {}",
                    info.parameters.len(),
                    arguments.len()
                ),
            ));
        }

        for local in &info.frame {
            self.load(*local, 0);
            self.push_register(0, span);
        }
        for (argument, (_, parameter)) in arguments.iter().zip(&info.parameters) {
            self.compile_expression(argument, Some(parameter.kind))?;
            self.push_register(0, argument.span);
        }
        for (_, parameter) in info.parameters.iter().rev() {
            self.pop_register(0);
            self.store(*parameter, 0);
        }
        self.assembler.branch_link(info.label, span);
        if info.return_kind.is_some() {
            self.assembler.emit(0x2a00_03e8); // mov w8,w0
        }
        for local in info.frame.iter().rev() {
            self.pop_register(1);
            self.store(*local, 1);
        }
        if info.return_kind.is_some() {
            self.assembler.emit(0x2a08_03e0); // mov w0,w8
        }
        if let (Some(expected), Some(actual)) = (expected, info.return_kind) {
            self.require_kind(actual, Some(expected), span)?;
        }
        Ok(info.return_kind)
    }

    fn push_register(&mut self, register: u32, span: Span) {
        self.assembler.emit(0xeb36_63ff); // cmp sp,x22
        self.assembler.conditional(9, self.stack_failure, span); // b.ls
        self.assembler.emit(0xf81f_0fe0 | register); // str xN,[sp,#-16]!
    }

    fn pop_register(&mut self, register: u32) {
        self.assembler.emit(0xf841_07e0 | register); // ldr xN,[sp],#16
    }

    fn emit_return(&mut self) {
        self.pop_register(30);
        self.assembler.emit(0xd65f_03c0); // ret
    }

    fn preallocate_block(
        &mut self,
        function: &str,
        block: &Block,
        frame: &mut Vec<Local>,
    ) -> Result<(), Diagnostic> {
        for statement in &block.statements {
            match &statement.node {
                Statement::Binding { annotation, .. } => {
                    let annotation = annotation.as_ref().ok_or_else(|| {
                        profile_error(
                            statement.span,
                            "AArch64 locals require an explicit exact scalar annotation",
                        )
                    })?;
                    let local = if let Some((element, length)) = array_annotation(annotation)? {
                        let mut first = None;
                        for _ in 0..length {
                            let local = Local {
                                offset: self.allocate_local(element, statement.span)?,
                                kind: element,
                            };
                            first.get_or_insert(local.offset);
                            frame.push(local);
                        }
                        LocalValue::Array(ArrayLocal {
                            offset: first.unwrap_or(self.next_local),
                            element,
                            length,
                        })
                    } else {
                        let kind = scalar_type(annotation, "AArch64 local")?;
                        let local = Local {
                            offset: self.allocate_local(kind, statement.span)?,
                            kind,
                        };
                        frame.push(local);
                        LocalValue::Scalar(local)
                    };
                    self.preallocated
                        .insert((function.to_owned(), statement.span), local);
                }
                Statement::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.preallocate_block(function, then_branch, frame)?;
                    if let Some(else_branch) = else_branch {
                        self.preallocate_block(function, else_branch, frame)?;
                    }
                }
                Statement::While { body, .. } | Statement::Loop(body) => {
                    self.preallocate_block(function, body, frame)?;
                }
                Statement::Unsafe { body, .. } => {
                    self.preallocate_block(function, body, frame)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn require_kind(
        &self,
        actual: ScalarKind,
        expected: Option<ScalarKind>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        if let Some(expected) = expected
            && actual != expected
        {
            return Err(profile_error(
                span,
                format!(
                    "AArch64 scalar type mismatch: expected `{}`, found `{}`",
                    kind_name(expected),
                    kind_name(actual)
                ),
            ));
        }
        Ok(())
    }

    fn lookup_value(&self, name: &str, span: Span) -> Result<LocalValue, Diagnostic> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .ok_or_else(|| profile_error(span, format!("unknown AArch64 local `{name}`")))
    }

    fn lookup(&self, name: &str, span: Span) -> Result<Local, Diagnostic> {
        match self.lookup_value(name, span)? {
            LocalValue::Scalar(local) => Ok(local),
            LocalValue::Array(_) => Err(profile_error(
                span,
                format!("AArch64 fixed array `{name}` requires an index"),
            )),
        }
    }

    fn direct_array(&self, object: &Expr) -> Result<ArrayLocal, Diagnostic> {
        let Expression::Identifier(name) = &object.node else {
            return Err(profile_error(
                object.span,
                "AArch64 indexing requires a direct fixed-array local",
            ));
        };
        match self.lookup_value(name, object.span)? {
            LocalValue::Array(array) => Ok(array),
            LocalValue::Scalar(_) => Err(profile_error(
                object.span,
                format!("AArch64 scalar `{name}` cannot be indexed"),
            )),
        }
    }

    fn allocate_local(&mut self, kind: ScalarKind, span: Span) -> Result<u32, Diagnostic> {
        let alignment = kind.bytes();
        let aligned = self
            .next_local
            .checked_add(alignment - 1)
            .map(|value| value / alignment * alignment)
            .ok_or_else(|| profile_error(span, "AArch64 local alignment overflow"))?;
        let end = aligned
            .checked_add(kind.bytes())
            .ok_or_else(|| profile_error(span, "AArch64 local storage overflow"))?;
        if end > MAX_LOCAL_BYTES {
            return Err(profile_error(
                span,
                format!("AArch64 scalar storage exceeds {MAX_LOCAL_BYTES} bytes"),
            ));
        }
        self.next_local = end;
        Ok(aligned)
    }

    fn load(&mut self, local: Local, register: u32) {
        let (base, immediate) = match local.kind {
            ScalarKind::U8 => (0x3940_0000, local.offset),
            ScalarKind::U16 => (0x7940_0000, local.offset / 2),
            ScalarKind::U32 | ScalarKind::I32 | ScalarKind::Bool => (0xb940_0000, local.offset / 4),
        };
        self.assembler
            .emit(base | (immediate << 10) | (19 << 5) | register);
    }

    fn store(&mut self, local: Local, register: u32) {
        let (base, immediate) = match local.kind {
            ScalarKind::U8 => (0x3900_0000, local.offset),
            ScalarKind::U16 => (0x7900_0000, local.offset / 2),
            ScalarKind::U32 | ScalarKind::I32 | ScalarKind::Bool => (0xb900_0000, local.offset / 4),
        };
        self.assembler
            .emit(base | (immediate << 10) | (19 << 5) | register);
    }

    fn load_immediate(&mut self, value: u32, register: u32) {
        self.assembler
            .emit(0x5280_0000 | ((value & 0xffff) << 5) | register);
        if value > u16::MAX.into() {
            self.assembler
                .emit(0x72a0_0000 | ((value >> 16) << 5) | register);
        }
    }
}

fn kind_name(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::U8 => "u8",
        ScalarKind::U16 => "u16",
        ScalarKind::U32 => "u32",
        ScalarKind::I32 => "i32",
        ScalarKind::Bool => "bool",
    }
}

fn install_page_tables(
    payload: &mut [u8],
    layout: PageTableLayout,
    span: Span,
) -> Result<(), Diagnostic> {
    const TABLE_DESCRIPTOR: u64 = 0x3;
    const NORMAL_RW_PAGE: u64 = 0x703; // Attr0, privileged RW, inner-shareable, AF, page
    const NORMAL_RO_PAGE: u64 = 0x783; // Attr0, privileged RO, inner-shareable, AF, page
    const EXECUTE_NEVER: u64 = (1 << 53) | (1 << 54);

    let image_size = IMAGE_HEADER_BYTES
        .checked_add(payload.len())
        .ok_or_else(|| profile_error(span, "AArch64 paged image size overflow"))?;
    if !image_size.is_multiple_of(PAGE_BYTES) {
        return Err(profile_error(
            span,
            "AArch64 paged image must end on a 4 KiB boundary",
        ));
    }
    let full_offset = |payload_offset: usize| {
        IMAGE_HEADER_BYTES
            .checked_add(payload_offset)
            .ok_or_else(|| profile_error(span, "AArch64 page-table offset overflow"))
    };
    let physical = |payload_offset: usize| {
        let offset = full_offset(payload_offset)?;
        if !offset.is_multiple_of(PAGE_BYTES) {
            return Err(profile_error(
                span,
                "AArch64 translation tables must be image-page aligned",
            ));
        }
        IMAGE_LOAD_ADDRESS
            .checked_add(offset as u64)
            .ok_or_else(|| profile_error(span, "AArch64 translation-table address overflow"))
    };
    let data_start = full_offset(layout.data_start)?;
    let page_tables_start = full_offset(layout.page_tables_start)?;
    if !data_start.is_multiple_of(PAGE_BYTES)
        || !page_tables_start.is_multiple_of(PAGE_BYTES)
        || data_start >= page_tables_start
    {
        return Err(profile_error(
            span,
            "AArch64 executable/data/page-table regions are not page separated",
        ));
    }
    if image_size - page_tables_start != PAGE_TABLE_COUNT * PAGE_BYTES {
        return Err(profile_error(
            span,
            "AArch64 translation-table region has an unexpected size",
        ));
    }

    let _root = physical(layout.root)?;
    let _low_l2 = physical(layout.low_l2)?;
    let _low_l3 = physical(layout.low_l3)?;
    let image_l2 = physical(layout.image_l2)?;
    let image_l3 = physical(layout.image_l3)?;
    let mut write = |table: usize, index: usize, descriptor: u64| -> Result<(), Diagnostic> {
        if index >= PAGE_TABLE_ENTRIES {
            return Err(profile_error(span, "AArch64 page-table index overflow"));
        }
        let at = table
            .checked_add(index * 8)
            .ok_or_else(|| profile_error(span, "AArch64 page-table write overflow"))?;
        let destination = payload
            .get_mut(at..at + 8)
            .ok_or_else(|| profile_error(span, "AArch64 page-table write exceeds the image"))?;
        destination.copy_from_slice(&descriptor.to_le_bytes());
        Ok(())
    };

    let root_index = |address: u64| ((address >> 30) & 0x1ff) as usize;
    let l2_index = |address: u64| ((address >> 21) & 0x1ff) as usize;
    let l3_index = |address: u64| ((address >> 12) & 0x1ff) as usize;
    write(
        layout.root,
        root_index(IMAGE_LOAD_ADDRESS),
        image_l2 | TABLE_DESCRIPTOR,
    )?;
    write(
        layout.image_l2,
        l2_index(IMAGE_LOAD_ADDRESS),
        image_l3 | TABLE_DESCRIPTOR,
    )?;
    for image_offset in (0..image_size).step_by(PAGE_BYTES) {
        let address = IMAGE_LOAD_ADDRESS
            .checked_add(image_offset as u64)
            .ok_or_else(|| profile_error(span, "AArch64 image-page address overflow"))?;
        if l2_index(address) != l2_index(IMAGE_LOAD_ADDRESS) {
            return Err(profile_error(
                span,
                "AArch64 image exceeds its bounded 2 MiB translation window",
            ));
        }
        let attributes = if image_offset < data_start {
            NORMAL_RO_PAGE
        } else if image_offset < page_tables_start {
            NORMAL_RW_PAGE | EXECUTE_NEVER
        } else {
            NORMAL_RO_PAGE | EXECUTE_NEVER
        };
        write(layout.image_l3, l3_index(address), address | attributes)?;
    }
    Ok(())
}

/// Builds a deterministic AArch64 Linux-Image-compatible payload for QEMU `virt-8.2`.
pub fn build_aarch64_virt(program: &Program, source_path: &Path) -> Result<PathBuf, Diagnostic> {
    if !source_path.is_file()
        || source_path.extension().and_then(|value| value.to_str()) != Some("disp")
    {
        return Err(error_at(
            source_path,
            Span::point(1, 1),
            "the AArch64 virt target requires one `.disp` source file",
        ));
    }
    let image = compile_aarch64_virt(program).map_err(|error| {
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
                "the AArch64 source filename must be valid UTF-8",
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
            format!("could not create AArch64 build directory: {cause}"),
        )
    })?;
    let destination = build.join(format!("{stem}-aarch64-virt-8.2.img"));
    transactional_write(&destination, &image).map_err(|cause| {
        error_at(
            source_path,
            Span::point(1, 1),
            format!("could not write AArch64 image safely: {cause}"),
        )
    })?;
    Ok(destination)
}

/// Compiles bounded AArch64 programs with guarded functions, arrays, exceptions, and sparse W^X.
pub fn compile_aarch64_virt(program: &Program) -> Result<Vec<u8>, Diagnostic> {
    let main = validate_program(program)?;
    let payload = ScalarCompiler::new(program, main.body.span)?.compile(program, main)?;
    let image_size = IMAGE_HEADER_BYTES
        .checked_add(payload.len())
        .ok_or_else(|| profile_error(main.body.span, "AArch64 image size overflow"))?;
    if image_size > MAX_IMAGE_BYTES {
        return Err(profile_error(
            main.body.span,
            format!("AArch64 image exceeds the {MAX_IMAGE_BYTES}-byte profile limit"),
        ));
    }
    let mut image = vec![0; IMAGE_HEADER_BYTES];
    image[0..4]
        .copy_from_slice(&encode_branch(0, IMAGE_HEADER_BYTES, main.body.span)?.to_le_bytes());
    image[8..16].copy_from_slice(&IMAGE_TEXT_OFFSET.to_le_bytes());
    image[16..24].copy_from_slice(&(image_size as u64).to_le_bytes());
    image[24..32].copy_from_slice(&IMAGE_FLAGS.to_le_bytes());
    image[56..60].copy_from_slice(&IMAGE_MAGIC.to_le_bytes());
    image.extend_from_slice(&payload);
    Ok(image)
}

fn validate_program(program: &Program) -> Result<&crate::ast::Function, Diagnostic> {
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
            "AArch64 virt does not yet accept modules, imports, or user-defined types",
        ));
    }
    let Some(main) = program
        .functions
        .iter()
        .find(|function| function.name == "main")
    else {
        return Err(profile_error(
            Span::point(1, 1),
            "AArch64 virt requires exactly one `fn main()` entry point",
        ));
    };
    if !main.parameters.is_empty()
        || main.return_type.is_some()
        || main.asynchronous
        || !main.generics.is_empty()
        || main.capabilities.is_some()
        || main.external.is_some()
    {
        return Err(profile_error(
            main.span,
            "AArch64 virt requires plain `fn main()` without parameters, result, generics, capabilities, or external ABI",
        ));
    }
    for function in &program.functions {
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
                "AArch64 functions cannot be async, generic, external, or carry capabilities other than `DeviceIo`",
            ));
        }
        for parameter in &function.parameters {
            scalar_type(&parameter.ty, "AArch64 function parameter")?;
        }
        if let Some(return_type) = &function.return_type {
            scalar_type(return_type, "AArch64 function return")?;
        }
    }
    Ok(main)
}

fn array_annotation(
    annotation: &crate::ast::TypeName,
) -> Result<Option<(ScalarKind, usize)>, Diagnostic> {
    let Some(length) = annotation
        .name
        .strip_prefix("[;")
        .and_then(|name| name.strip_suffix(']'))
    else {
        return Ok(None);
    };
    if annotation.qualifier != TypeQualifier::Owned || annotation.arguments.len() != 1 {
        return Err(profile_error(
            annotation.span,
            "AArch64 fixed arrays require one owned exact-scalar element type",
        ));
    }
    let length = length
        .parse::<usize>()
        .map_err(|_| profile_error(annotation.span, "invalid AArch64 fixed-array length"))?;
    let element_type = &annotation.arguments[0];
    if element_type.qualifier != TypeQualifier::Owned || !element_type.arguments.is_empty() {
        return Err(profile_error(
            element_type.span,
            "AArch64 fixed-array elements require an owned exact scalar",
        ));
    }
    let element = ScalarKind::from_name(&element_type.name).ok_or_else(|| {
        profile_error(
            element_type.span,
            "AArch64 fixed arrays support only `u8`, `u16`, `u32`, `i32`, and `bool` elements",
        )
    })?;
    Ok(Some((element, length)))
}

fn scalar_type(ty: &crate::ast::TypeName, context: &str) -> Result<ScalarKind, Diagnostic> {
    if ty.qualifier != TypeQualifier::Owned || !ty.arguments.is_empty() {
        return Err(profile_error(
            ty.span,
            format!("{context} requires an owned, non-generic exact scalar"),
        ));
    }
    ScalarKind::from_name(&ty.name).ok_or_else(|| {
        profile_error(
            ty.span,
            format!("{context} supports only `u8`, `u16`, `u32`, `i32`, and `bool`"),
        )
    })
}

fn encode_branch(from: usize, to: usize, span: Span) -> Result<u32, Diagnostic> {
    let field = signed_scaled_field(from, to, 26, span, "AArch64 branch")?;
    Ok(0x1400_0000 | field)
}

fn encode_adr(register: u32, from: usize, to: usize, span: Span) -> Result<u32, Diagnostic> {
    let delta = signed_delta(from, to, span, "AArch64 ADR")?;
    let minimum = -(1i64 << 20);
    let maximum = (1i64 << 20) - 1;
    if !(minimum..=maximum).contains(&delta) {
        return Err(profile_error(
            span,
            "AArch64 ADR target exceeds 21-bit reach",
        ));
    }
    let field = (delta as u64 & 0x1f_ffff) as u32;
    Ok(0x1000_0000 | ((field & 3) << 29) | ((field >> 2) << 5) | register)
}

fn signed_scaled_field(
    from: usize,
    to: usize,
    bits: u32,
    span: Span,
    operation: &str,
) -> Result<u32, Diagnostic> {
    let delta = signed_delta(from, to, span, operation)?;
    if delta % 4 != 0 {
        return Err(profile_error(
            span,
            format!("{operation} target is not aligned"),
        ));
    }
    let scaled = delta / 4;
    let minimum = -(1i64 << (bits - 1));
    let maximum = (1i64 << (bits - 1)) - 1;
    if !(minimum..=maximum).contains(&scaled) {
        return Err(profile_error(
            span,
            format!("{operation} target exceeds {bits}-bit reach"),
        ));
    }
    Ok((scaled as u64 & ((1u64 << bits) - 1)) as u32)
}

fn signed_delta(from: usize, to: usize, span: Span, operation: &str) -> Result<i64, Diagnostic> {
    let from = i64::try_from(from)
        .map_err(|_| profile_error(span, format!("{operation} origin exceeds i64")))?;
    let to = i64::try_from(to)
        .map_err(|_| profile_error(span, format!("{operation} target exceeds i64")))?;
    to.checked_sub(from)
        .ok_or_else(|| profile_error(span, format!("{operation} displacement overflow")))
}

fn profile_error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, span).with_help(
        "use the bounded AArch64 exact-scalar, checked arithmetic/control-flow, and scalar-output profile",
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
    fn aarch64_image_has_checked_scalar_control_and_deterministic_utf8_data() {
        let source = r#"
fn main() {
    var value: u32 = 6
    value *= 7
    while value > 40 {
        value -= 1
    }
    let ready: bool = value == 40
    if ready && true {
        print("AArch64 scalar ✓")
    } else {
        print("wrong")
    }
}
"#;
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            u32::from_le_bytes(first[56..60].try_into().unwrap()),
            IMAGE_MAGIC
        );
        assert_eq!(
            u64::from_le_bytes(first[8..16].try_into().unwrap()),
            IMAGE_TEXT_OFFSET
        );
        assert_eq!(
            u64::from_le_bytes(first[24..32].try_into().unwrap()),
            IMAGE_FLAGS
        );
        assert_eq!(
            u64::from_le_bytes(first[16..24].try_into().unwrap()),
            first.len() as u64
        );
        assert_eq!(
            u32::from_le_bytes(first[0..4].try_into().unwrap()),
            0x1400_0010
        );
        for instruction in [
            0x9ba0_7c22u32,
            0xd360_fc43,
            0x6b00_0020,
            0x6b00_003f,
            0x1a9f_97e0,
            0x1a9f_17e0,
        ] {
            assert!(
                first
                    .windows(4)
                    .any(|bytes| bytes == instruction.to_le_bytes())
            );
        }
        let text = "AArch64 scalar ✓\r\n\0".as_bytes();
        assert!(first.windows(text.len()).any(|bytes| bytes == text));
        let fault = b"[DISP arithmetic fault]\r\n\0";
        assert!(first.windows(fault.len()).any(|bytes| bytes == fault));
    }

    #[test]
    fn aarch64_scalar_profile_rejects_unsupported_or_unbounded_programs() {
        let float = check_source("fn main(){let value:f64=1.0 print(\"x\")}").unwrap();
        assert!(
            compile_aarch64_virt(&float)
                .unwrap_err()
                .message
                .contains("supports only `u8`")
        );

        let text = check_source("fn main(){let value:String=\"x\" print(\"x\")}").unwrap();
        assert!(
            compile_aarch64_virt(&text)
                .unwrap_err()
                .message
                .contains("supports only `u8`")
        );

        let nul = check_source("fn main(){print(\"a\\0b\")}").unwrap();
        assert!(
            compile_aarch64_virt(&nul)
                .unwrap_err()
                .message
                .contains("NUL")
        );

        let text = "x".repeat(MAX_STATIC_STRING_BYTES);
        let program = check_source(&format!("fn main(){{print(\"{text}\")}}")).unwrap();
        assert!(
            compile_aarch64_virt(&program)
                .unwrap_err()
                .message
                .contains("static strings")
        );
    }

    #[test]
    fn aarch64_exact_widths_use_compact_storage_checked_signed_math_and_scalar_output() {
        let source = include_str!("../examples/freestanding_aarch64_exact_scalars.disp");
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        let words = first[IMAGE_HEADER_BYTES..]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert!(words.iter().any(|word| word & 0xffc0_0000 == 0x3900_0000));
        assert!(words.iter().any(|word| word & 0xffc0_0000 == 0x3940_0000));
        assert!(words.iter().any(|word| word & 0xffc0_0000 == 0x7900_0000));
        assert!(words.iter().any(|word| word & 0xffc0_0000 == 0x7940_0000));
        for instruction in [
            0x9b20_7c22u32, // smull x2,w1,w0
            0x9340_7c43,    // sxtw x3,w2
            0x1ac0_0c20,    // sdiv w0,w1,w0
            0x1ac4_0865,    // decimal conversion udiv
            0x1b04_8ca6,    // decimal digit remainder
            0x3900_0046,    // compact digit store
        ] {
            assert!(
                words.contains(&instruction),
                "missing A64 instruction {instruction:08x}"
            );
        }
        assert!(first.windows(7).any(|bytes| bytes == b"true\r\n\0"));
        assert!(first.windows(8).any(|bytes| bytes == b"false\r\n\0"));
    }

    #[test]
    fn aarch64_functions_preserve_recursive_exact_frames_and_guard_the_stack() {
        let functions = include_str!("../examples/freestanding_aarch64_functions.disp");
        let program = check_source(functions).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        let words = first[IMAGE_HEADER_BYTES..]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert!(words.iter().any(|word| word & 0xfc00_0000 == 0x9400_0000));
        for instruction in [
            0x9100_02bfu32, // mov sp,x21
            0xeb36_63ff,    // cmp sp,x22
            0xf81f_0ffe,    // push x30 in a 16-byte slot
            0xf841_07fe,    // pop x30
            0xd65f_03c0,    // ret
        ] {
            assert!(words.contains(&instruction));
        }
        let fault = b"[DISP stack exhausted]\r\n\0";
        assert!(first.windows(fault.len()).any(|bytes| bytes == fault));

        let stack = include_str!("../examples/freestanding_aarch64_stack.disp");
        let stack_program = check_source(stack).unwrap();
        let stack_image = compile_aarch64_virt(&stack_program).unwrap();
        assert!(stack_image.windows(fault.len()).any(|bytes| bytes == fault));
    }

    #[test]
    fn aarch64_fixed_arrays_use_exact_storage_checked_indices_and_recursive_frames() {
        let source = include_str!("../examples/freestanding_aarch64_arrays.disp");
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        let words = first[IMAGE_HEADER_BYTES..]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        for instruction in [
            0x6b02_001fu32, // cmp index,length
            0x3940_0040,    // ldrb w0,[x2]
            0x3900_0040,    // strb w0,[x2]
            0x7940_0040,    // ldrh w0,[x2]
            0x7900_0040,    // strh w0,[x2]
            0xb940_0040,    // ldr w0,[x2]
            0xb900_0040,    // str w0,[x2]
        ] {
            assert!(words.contains(&instruction));
        }
        assert!(words.iter().any(|word| word & 0xff00_001f == 0x5400_0002));
        let fault = b"[DISP index out of bounds]\r\n\0";
        assert!(first.windows(fault.len()).any(|bytes| bytes == fault));

        let bounds = include_str!("../examples/freestanding_aarch64_bounds.disp");
        let bounds_program = check_source(bounds).unwrap();
        let bounds_image = compile_aarch64_virt(&bounds_program).unwrap();
        assert!(
            bounds_image
                .windows(fault.len())
                .any(|bytes| bytes == fault)
        );
    }

    #[test]
    fn aarch64_exception_vectors_cover_current_el_classes_and_fail_closed() {
        let source = include_str!("../examples/freestanding_aarch64_exceptions.disp");
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);

        assert_eq!(
            u32::from_le_bytes(
                first[IMAGE_HEADER_BYTES..IMAGE_HEADER_BYTES + 4]
                    .try_into()
                    .unwrap()
            ),
            0xd503_4fdf // msr daifset,#0xf
        );
        for (offset, instruction) in [
            (IMAGE_HEADER_BYTES + 7 * 4, 0xd538_4258u32), // mrs x24,CurrentEL
            (IMAGE_HEADER_BYTES + 12 * 4, 0xd51c_c017),   // msr VBAR_EL2,x23
            (IMAGE_HEADER_BYTES + 14 * 4, 0xd518_c017),   // msr VBAR_EL1,x23
            (IMAGE_HEADER_BYTES + 15 * 4, 0xd503_3fdf),   // isb
            (EXCEPTION_PROBE_IMAGE_OFFSET, EXCEPTION_PROBE_INSTRUCTION),
        ] {
            assert_eq!(
                u32::from_le_bytes(first[offset..offset + 4].try_into().unwrap()),
                instruction
            );
        }

        let tables = (EXCEPTION_VECTOR_ALIGNMENT..first.len())
            .step_by(EXCEPTION_VECTOR_ALIGNMENT)
            .filter(|offset| {
                offset + EXCEPTION_VECTOR_ALIGNMENT <= first.len()
                    && (0..EXCEPTION_VECTOR_SLOTS).all(|slot| {
                        let entry = offset + slot * EXCEPTION_VECTOR_SLOT_BYTES;
                        let branch =
                            u32::from_le_bytes(first[entry..entry + 4].try_into().unwrap());
                        branch & 0xfc00_0000 == 0x1400_0000
                            && first[entry + 4..entry + EXCEPTION_VECTOR_SLOT_BYTES]
                                .iter()
                                .all(|byte| *byte == 0)
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(tables.len(), 1);
        let table = tables[0];
        assert_eq!(table % EXCEPTION_VECTOR_ALIGNMENT, 0);

        let vector_adr_at = IMAGE_HEADER_BYTES + 6 * 4;
        let vector_adr =
            u32::from_le_bytes(first[vector_adr_at..vector_adr_at + 4].try_into().unwrap());
        assert_eq!(vector_adr & 0x9f00_001f, 0x1000_0017); // adr x23,exception_vectors
        let immediate = ((((vector_adr >> 5) & 0x7ffff) << 2) | ((vector_adr >> 29) & 0x3)) as i32;
        let signed_immediate = (immediate << 11) >> 11;
        assert_eq!(
            (vector_adr_at as isize + signed_immediate as isize) as usize,
            table
        );

        let targets = (0..4)
            .map(|slot| {
                let at = table + slot * EXCEPTION_VECTOR_SLOT_BYTES;
                let word = u32::from_le_bytes(first[at..at + 4].try_into().unwrap());
                let field = (word & 0x03ff_ffff) as i32;
                let signed = (field << 6) >> 6;
                (at as isize + (signed as isize * 4)) as usize
            })
            .collect::<Vec<_>>();
        let mut distinct = targets.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 4);
        for slot in 0..EXCEPTION_VECTOR_SLOTS {
            let at = table + slot * EXCEPTION_VECTOR_SLOT_BYTES;
            let word = u32::from_le_bytes(first[at..at + 4].try_into().unwrap());
            let field = (word & 0x03ff_ffff) as i32;
            let signed = (field << 6) >> 6;
            assert_eq!(
                (at as isize + signed as isize * 4) as usize,
                targets[slot % 4]
            );
        }

        for diagnostic in [
            b"[DISP unsupported exception level]\r\n\0".as_slice(),
            b"[DISP synchronous exception]\r\n\0".as_slice(),
            b"[DISP IRQ exception]\r\n\0".as_slice(),
            b"[DISP FIQ exception]\r\n\0".as_slice(),
            b"[DISP system error exception]\r\n\0".as_slice(),
        ] {
            assert!(
                first
                    .windows(diagnostic.len())
                    .any(|bytes| bytes == diagnostic)
            );
        }
    }

    #[test]
    fn aarch64_sparse_page_tables_enforce_wx_and_protect_translation_state() {
        const ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;
        const ATTRIBUTE_MASK: u64 = !ADDRESS_MASK;
        const EXECUTE_NEVER: u64 = (1 << 53) | (1 << 54);
        let source = include_str!("../examples/freestanding_aarch64_mmu.disp");
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        assert!(first.len().is_multiple_of(PAGE_BYTES));

        for instruction in [
            0xd51c_a21au32, // msr MAIR_EL2,x26
            0xd51c_205a,    // msr TCR_EL2,x26
            0xd51c_2019,    // msr TTBR0_EL2,x25
            0xd50c_871f,    // tlbi alle2
            0xd51c_101a,    // msr SCTLR_EL2,x26
            0xd518_a21a,    // msr MAIR_EL1,x26
            0xd518_205a,    // msr TCR_EL1,x26
            0xd518_2019,    // msr TTBR0_EL1,x25
            0xd508_871f,    // tlbi vmalle1
            0xd518_101a,    // msr SCTLR_EL1,x26
            0xaa1b_035a,    // orr x26,x26,x27 (WXN|I|C|M)
        ] {
            assert!(
                first
                    .windows(4)
                    .any(|bytes| bytes == instruction.to_le_bytes()),
                "missing A64 MMU instruction {instruction:08x}"
            );
        }
        assert_eq!(
            u32::from_le_bytes(
                first[MMU_PROBE_IMAGE_OFFSET..MMU_PROBE_IMAGE_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            MMU_PROBE_INSTRUCTION
        );

        let entry_words = first[IMAGE_HEADER_BYTES..]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        let movz_w =
            |register: u32, immediate: u32| 0x5280_0000 | ((immediate & 0xffff) << 5) | register;
        let movk_w16 =
            |register: u32, immediate: u32| 0x72a0_0000 | ((immediate & 0xffff) << 5) | register;
        assert_eq!(entry_words[19], movz_w(26, 0xff)); // MAIR Attr0/Attr1
        assert_eq!(entry_words[23], movz_w(26, 0x3520)); // TCR_EL2
        assert_eq!(entry_words[32], movz_w(27, 0x1005));
        assert_eq!(entry_words[33], movk_w16(27, 0x8)); // EL2 WXN|I|C|M
        assert_eq!(entry_words[39], movz_w(26, 0x3520));
        assert_eq!(entry_words[40], movk_w16(26, 0x80)); // TCR_EL1 EPD1
        assert_eq!(entry_words[49], movz_w(27, 0x1005));
        assert_eq!(entry_words[50], movk_w16(27, 0x8)); // EL1 WXN|I|C|M

        let decode_adr = |at: usize, instruction: u32| {
            let immediate =
                ((((instruction >> 5) & 0x7ffff) << 2) | ((instruction >> 29) & 0x3)) as i32;
            (at as isize + ((immediate << 11) >> 11) as isize) as usize
        };
        let probe_adr_at = IMAGE_HEADER_BYTES + 17 * 4;
        assert_eq!(entry_words[17] & 0x9f00_001f, 0x1000_001c); // adr x28,code_start
        assert_eq!(
            decode_adr(probe_adr_at, entry_words[17]),
            IMAGE_HEADER_BYTES
        );

        let root_adr_at = IMAGE_HEADER_BYTES + 18 * 4;
        let root_adr = u32::from_le_bytes(first[root_adr_at..root_adr_at + 4].try_into().unwrap());
        assert_eq!(root_adr & 0x9f00_001f, 0x1000_0019); // adr x25,root_table
        let root_offset = decode_adr(root_adr_at, root_adr);
        let page_tables_start = first.len() - PAGE_TABLE_COUNT * PAGE_BYTES;
        assert_eq!(root_offset, page_tables_start);
        assert!(root_offset.is_multiple_of(PAGE_BYTES));

        let descriptor = |table: usize, index: usize| {
            let at = table + index * 8;
            u64::from_le_bytes(first[at..at + 8].try_into().unwrap())
        };
        let file_offset =
            |entry: u64| (entry & ADDRESS_MASK) as usize - IMAGE_LOAD_ADDRESS as usize;
        let root_entries = (0..PAGE_TABLE_ENTRIES)
            .filter(|index| descriptor(root_offset, *index) != 0)
            .collect::<Vec<_>>();
        assert_eq!(root_entries, [1]);
        let low_l2 = root_offset + PAGE_BYTES;
        let low_l3 = low_l2 + PAGE_BYTES;
        let image_l2 = file_offset(descriptor(root_offset, 1));
        assert_eq!(descriptor(root_offset, 1) & ATTRIBUTE_MASK, 0x3);
        assert!((0..PAGE_TABLE_ENTRIES).all(|index| descriptor(low_l2, index) == 0));
        assert!((0..PAGE_TABLE_ENTRIES).all(|index| descriptor(low_l3, index) == 0));
        let image_l3 = file_offset(descriptor(
            image_l2,
            ((IMAGE_LOAD_ADDRESS >> 21) & 0x1ff) as usize,
        ));

        let page_count = first.len() / PAGE_BYTES;
        let image_index = ((IMAGE_LOAD_ADDRESS >> 12) & 0x1ff) as usize;
        let image_entries = (0..PAGE_TABLE_ENTRIES)
            .filter(|index| descriptor(image_l3, *index) != 0)
            .collect::<Vec<_>>();
        assert_eq!(
            image_entries,
            (image_index..image_index + page_count).collect::<Vec<_>>()
        );
        let attributes = (0..page_count)
            .map(|page| descriptor(image_l3, image_index + page) & ATTRIBUTE_MASK)
            .collect::<Vec<_>>();
        let first_writable = attributes
            .iter()
            .position(|attributes| *attributes == EXECUTE_NEVER | 0x703)
            .expect("one writable non-executable data region");
        assert!(first_writable > 0);
        assert!(
            attributes[..first_writable]
                .iter()
                .all(|attributes| *attributes == 0x783)
        );
        let table_page = page_count - PAGE_TABLE_COUNT;
        assert!(
            attributes[first_writable..table_page]
                .iter()
                .all(|attributes| *attributes == EXECUTE_NEVER | 0x703)
        );
        assert!(
            attributes[table_page..]
                .iter()
                .all(|attributes| *attributes == EXECUTE_NEVER | 0x783)
        );

        let diagnostic = b"[DISP memory protection fault]\r\n\0";
        assert!(
            first
                .windows(diagnostic.len())
                .any(|bytes| bytes == diagnostic)
        );
        let mut injected = first;
        injected[MMU_PROBE_IMAGE_OFFSET..MMU_PROBE_IMAGE_OFFSET + 4]
            .copy_from_slice(&0xb900_039fu32.to_le_bytes()); // str wzr,[x28]
        assert_eq!(
            u32::from_le_bytes(
                injected[MMU_PROBE_IMAGE_OFFSET..MMU_PROBE_IMAGE_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            0xb900_039f
        );
    }

    #[test]
    fn aarch64_dtb_prelude_discovers_pl011_validates_ram_and_patches_sparse_tables() {
        let source = include_str!("../examples/freestanding_aarch64_dtb.disp");
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        assert!(first.len().is_multiple_of(PAGE_BYTES));

        assert_eq!(
            u32::from_le_bytes(
                first[IMAGE_HEADER_BYTES..IMAGE_HEADER_BYTES + 4]
                    .try_into()
                    .unwrap()
            ),
            0xd503_4fdf // msr daifset,#0xf
        );
        assert_eq!(
            u32::from_le_bytes(
                first[IMAGE_HEADER_BYTES + 8..IMAGE_HEADER_BYTES + 12]
                    .try_into()
                    .unwrap()
            ),
            0xd503_201f // reserved entry checkpoint remains a NOP
        );
        assert_eq!(
            u32::from_le_bytes(
                first[EXCEPTION_PROBE_IMAGE_OFFSET..EXCEPTION_PROBE_IMAGE_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            EXCEPTION_PROBE_INSTRUCTION
        );
        assert_eq!(
            u32::from_le_bytes(
                first[MMU_PROBE_IMAGE_OFFSET..MMU_PROBE_IMAGE_OFFSET + 4]
                    .try_into()
                    .unwrap()
            ),
            MMU_PROBE_INSTRUCTION
        );

        let entry_branch = u32::from_le_bytes(
            first[IMAGE_HEADER_BYTES + 4..IMAGE_HEADER_BYTES + 8]
                .try_into()
                .unwrap(),
        );
        assert_eq!(entry_branch & 0xfc00_0000, 0x1400_0000);
        assert!(
            !first
                .windows(8)
                .any(|bytes| bytes == 0x0900_0000u64.to_le_bytes()),
            "the target image must not embed QEMU's historical UART base"
        );

        for instruction in [
            0xd35e_9a8eu32, // ubfx x14,x20,#30,#9 (root index)
            0xd355_768e,    // ubfx x14,x20,#21,#9 (L2 index)
            0xd34c_528e,    // ubfx x14,x20,#12,#9 (L3 index)
            0xf82e_7b2f,    // str x15,[x25,x14,lsl #3]
            0xf82e_7b4f,    // str x15,[x26,x14,lsl #3]
            0xf82e_7b6f,    // str x15,[x27,x14,lsl #3]
            0xf2e0_0c0f,    // movk x15,#0x60,lsl #48 (PXN|UXN)
        ] {
            assert!(
                first
                    .windows(4)
                    .any(|bytes| bytes == instruction.to_le_bytes()),
                "missing dynamic DTB/MMU instruction {instruction:08x}"
            );
        }

        let aligned_reg_load = [
            0xb940_020au32, // ldr w10,[x16]
            0x5ac0_094a,    // rev w10,w10
            0xd360_7d4a,    // lsl x10,x10,#32
            0xb940_060f,    // ldr w15,[x16,#4]
            0x5ac0_09ef,    // rev w15,w15
            0xaa0f_014a,    // orr x10,x10,x15
            0xb940_0a0b,    // ldr w11,[x16,#8]
            0x5ac0_096b,    // rev w11,w11
            0xd360_7d6b,    // lsl x11,x11,#32
            0xb940_0e0f,    // ldr w15,[x16,#12]
            0x5ac0_09ef,    // rev w15,w15
            0xaa0f_016b,    // orr x11,x11,x15
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
        assert!(
            first
                .windows(aligned_reg_load.len())
                .any(|bytes| bytes == aligned_reg_load),
            "FDT two-cell values must use four-byte-aligned word loads"
        );
        assert!(
            !first
                .windows(4)
                .any(|bytes| bytes == 0xf940_020au32.to_le_bytes()),
            "FDT property payloads must not use alignment-sensitive xword loads"
        );

        let words = first
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        let device_type_check = words
            .windows(2)
            .position(|pair| pair[0] == 0x7100_1ddf && pair[1] & 0xff00_001f == 0x5400_0001)
            .expect("the DTB parser must check the memory device_type length");
        let branch_index = device_type_check + 1;
        let immediate = ((words[branch_index] >> 5) & 0x7ffff) as i32;
        let signed_offset = (immediate << 13) >> 13;
        let mismatch_target = (branch_index as isize + signed_offset as isize) as usize;
        assert_eq!(
            words[mismatch_target],
            0xaa11_03e2, // mov x2,x17 at property_done
            "unrelated direct-child device_type properties must be ignored safely"
        );

        let root_offset = first.len() - PAGE_TABLE_COUNT * PAGE_BYTES;
        let descriptor = |table: usize, index: usize| {
            let at = table + index * 8;
            u64::from_le_bytes(first[at..at + 8].try_into().unwrap())
        };
        assert_eq!(
            (0..PAGE_TABLE_ENTRIES)
                .filter(|index| descriptor(root_offset, *index) != 0)
                .collect::<Vec<_>>(),
            [1]
        );
        let low_l2 = root_offset + PAGE_BYTES;
        let low_l3 = low_l2 + PAGE_BYTES;
        assert!((0..PAGE_TABLE_ENTRIES).all(|index| descriptor(low_l2, index) == 0));
        assert!((0..PAGE_TABLE_ENTRIES).all(|index| descriptor(low_l3, index) == 0));

        let text = b"AArch64 DTB discovery active\r\n\0";
        assert!(first.windows(text.len()).any(|bytes| bytes == text));
    }

    #[test]
    fn aarch64_mmio_requires_device_authority_bounds_offsets_and_orders_volatile_access() {
        let source = r#"
fn exercise() uses DeviceIo {
    unsafe uses DeviceIo {
        let byte: u8 = Mmio.read_u8(u16(1))
        let half: u16 = Mmio.read_u16(u16(2))
        let word: u32 = Mmio.read_u32(u16(24))
        Mmio.write_u8(u16(3), byte)
        Mmio.write_u16(u16(4), half)
        Mmio.write_u32(u16(0), word)
    }
}
fn main() { exercise() }
"#;
        let program = check_source(source).unwrap();
        let first = compile_aarch64_virt(&program).unwrap();
        let second = compile_aarch64_virt(&program).unwrap();
        assert_eq!(first, second);
        let words = first[IMAGE_HEADER_BYTES..]
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        for instruction in [
            0x8b00_0282u32, // add x2,x20,x0 (validated read offset)
            0x8b01_0282,    // add x2,x20,x1 (preserved write offset)
            0xd503_33bf,    // dmb osh
            0x3940_0040,    // ldrb w0,[x2]
            0x7940_0040,    // ldrh w0,[x2]
            0xb940_0040,    // ldr w0,[x2]
            0x3900_0040,    // strb w0,[x2]
            0x7900_0040,    // strh w0,[x2]
            0xb900_0040,    // str w0,[x2]
            0x5281_ffe2,    // mov w2,#4095 (byte page bound)
            0x5281_ffc2,    // mov w2,#4094 (halfword page bound)
            0x5281_ff82,    // mov w2,#4092 (word page bound)
        ] {
            assert!(
                words.contains(&instruction),
                "missing bounded volatile MMIO instruction {instruction:08x}"
            );
        }
        for (bound, alignment_bits) in [
            (0x5281_ffe2u32, &[][..]),
            (0x5281_ffc2, &[0][..]),
            (0x5281_ff82, &[0, 1][..]),
        ] {
            let positions = words
                .iter()
                .enumerate()
                .filter_map(|(index, word)| (*word == bound).then_some(index))
                .collect::<Vec<_>>();
            assert_eq!(
                positions.len(),
                2,
                "read and write need the same page bound"
            );
            for position in positions {
                assert_eq!(words[position + 1], 0x6b02_001f); // cmp w0,w2
                assert_eq!(words[position + 2] & 0xff00_001f, 0x5400_0008); // b.hi
                for (index, bit) in alignment_bits.iter().copied().enumerate() {
                    assert_eq!(
                        words[position + 3 + index] & 0xfff8_001f,
                        0x3700_0000 | (bit << 19)
                    ); // tbnz w0,#bit,failure
                }
            }
        }
        assert!(words.contains(&0xf81f_0fe0)); // preserve a validated write offset
        assert!(words.contains(&0xf841_07e1)); // restore that offset into x1
        assert!(
            first
                .windows(8)
                .all(|bytes| bytes != 0x0900_0000u64.to_le_bytes())
        );
        assert!(
            first
                .windows(b"[DISP device access fault]\r\n\0".len())
                .any(|bytes| bytes == b"[DISP device access fault]\r\n\0")
        );

        let outside = check_source("fn main(){let value:u32=Mmio.read_u32(u16(24))}")
            .expect_err("MMIO outside explicit DeviceIo authority must fail");
        assert!(outside.message.contains("requires an `unsafe` block"));

        let wrong = check_source("fn main(){unsafe uses RawMemory{Mmio.write_u32(u16(0),u32(1))}}")
            .expect_err("unrelated unsafe authority must not grant MMIO");
        assert!(
            wrong
                .message
                .contains("does not allow capability `DeviceIo`")
        );

        let offset =
            check_source("fn main(){unsafe uses DeviceIo{let value:u8=Mmio.read_u8(u32(0))}}")
                .expect_err("MMIO offset width is part of the contract");
        assert!(offset.message.contains("memory-mapped device offset"));
    }
}
