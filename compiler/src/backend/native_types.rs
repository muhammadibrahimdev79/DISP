use super::{layout, mono, target::Target};
use crate::{diagnostics::Diagnostic, hir};
use std::{collections::HashSet, fmt::Write};

pub fn generate(
    program: &hir::Program,
    mono_program: &mono::MonoProgram,
    target: Target,
) -> Result<String, Diagnostic> {
    let mut output = String::from(
        "/* Concrete monomorphized DISP storage types. */\n\
         typedef struct { unsigned char _zero[0]; } disp_native_unit;\n\
         typedef struct { char *data; size_t len; size_t cap; } disp_native_string;\n\
         typedef struct { const char *data; size_t len; } disp_native_str;\n\
         typedef struct { char *data; size_t len; size_t cap; } disp_native_cstring;\n\
         typedef struct { uint8_t *data; size_t len; size_t align; } disp_native_memory;\n\
         typedef struct { char *data; size_t len; size_t cap; } disp_native_path;\n\
         typedef struct { uint8_t bytes[16]; uint8_t family; } disp_native_ip_address;\n\
         typedef struct { disp_native_ip_address *data; size_t len; size_t cap; } disp_native_ip_list;\n\
         typedef struct { char *host; size_t len; uint16_t port; } disp_native_socket_address;\n\
         typedef struct disp_tcp_state disp_tcp_state;\n\
         typedef struct { disp_tcp_state *state; } disp_native_tcp_stream;\n\
         typedef struct disp_tcp_listener_state disp_tcp_listener_state;\n\
         typedef struct { disp_tcp_listener_state *state; } disp_native_tcp_listener;\n\
         typedef struct disp_udp_socket_state disp_udp_socket_state;\n\
         typedef struct { disp_udp_socket_state *state; } disp_native_udp_socket;\n\
         typedef struct { disp_native_socket_address source; uint8_t *data; size_t len; size_t cap; } disp_native_udp_datagram;\n\
         typedef struct { uint64_t nanos; } disp_native_instant;\n\
         typedef struct { uint64_t nanos; } disp_native_duration;\n\
         typedef struct { uintptr_t handle; void *result; } disp_native_thread;\n\
         typedef struct { void *context; _Bool (*poll)(void *, void *); void (*drop)(void *); } disp_native_future;\n\
         typedef struct disp_task_state disp_task_state;\n\
         typedef struct { disp_task_state *state; } disp_native_task;\n\
         typedef struct disp_mutex_state disp_mutex_state;\n\
         typedef struct { disp_mutex_state *state; } disp_native_mutex;\n\
         typedef struct { disp_mutex_state *state; } disp_native_mutex_guard;\n\
         typedef struct disp_atomic_int_state disp_atomic_int_state;\n\
         typedef struct { disp_atomic_int_state *state; } disp_native_atomic_int;\n\
         typedef struct { void (*code)(void); void *env; void (*drop)(void *); } disp_native_callable;\n",
    );
    for instance in &mono_program.types {
        writeln!(
            output,
            "typedef struct {} {};",
            name(&instance.ty),
            name(&instance.ty)
        )
        .unwrap();
    }
    let mut emitter = Emitter {
        program,
        target,
        emitted: HashSet::new(),
        active: HashSet::new(),
        output,
    };
    for instance in &mono_program.types {
        emitter.emit(&instance.ty)?;
    }
    Ok(emitter.output)
}

struct Emitter<'a> {
    program: &'a hir::Program,
    target: Target,
    emitted: HashSet<hir::Type>,
    active: HashSet<hir::Type>,
    output: String,
}

impl Emitter<'_> {
    fn emit(&mut self, ty: &hir::Type) -> Result<(), Diagnostic> {
        if self.emitted.contains(ty) || !is_aggregate(ty) {
            return Ok(());
        }
        if !self.active.insert(ty.clone()) {
            // References can form cycles, but by-value cycles were rejected by layout validation.
            return Ok(());
        }
        for dependency in dependencies(self.program, ty) {
            self.emit(&dependency)?;
        }
        self.definition(ty)?;
        self.active.remove(ty);
        self.emitted.insert(ty.clone());
        Ok(())
    }

    fn definition(&mut self, ty: &hir::Type) -> Result<(), Diagnostic> {
        let mut engine = layout::LayoutEngine::new(self.target, self.program);
        let concrete_layout = engine.layout(ty)?;
        let type_name = name(ty);
        match ty {
            hir::Type::Array(element, length) => {
                writeln!(
                    self.output,
                    "struct {type_name} {{ {} values[{length}]; }};",
                    c_type(element)
                )
                .unwrap();
            }
            hir::Type::Slice(element) => {
                writeln!(
                    self.output,
                    "struct {type_name} {{ {} *data; size_t len; }};",
                    c_type(element)
                )
                .unwrap();
            }
            hir::Type::List(element) => {
                writeln!(
                    self.output,
                    "struct {type_name} {{ {} *data; size_t len; size_t cap; }};",
                    c_type(element)
                )
                .unwrap();
            }
            hir::Type::Map(key, value) => {
                writeln!(
                    self.output,
                    "struct {type_name} {{ {} *keys; {} *values; uint8_t *states; size_t len; size_t cap; }};",
                    c_type(key), c_type(value)
                )
                .unwrap();
            }
            hir::Type::Set(element) => {
                writeln!(
                    self.output,
                    "struct {type_name} {{ {} *values; uint8_t *states; size_t len; size_t cap; }};",
                    c_type(element)
                )
                .unwrap();
            }
            hir::Type::Struct(id, arguments) => {
                let declaration = &self.program.structs[id.0];
                let substitutions = declaration
                    .generic_parameters
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect();
                writeln!(self.output, "struct {type_name} {{").unwrap();
                if declaration.fields.is_empty() {
                    writeln!(self.output, "unsigned char _zero[0];").unwrap();
                }
                for field in &declaration.fields {
                    let field_ty = layout::substitute(&field.ty, &substitutions);
                    writeln!(self.output, "{} f{};", c_type(&field_ty), field.index).unwrap();
                }
                writeln!(self.output, "}};").unwrap();
                for (index, offset) in concrete_layout.fields.iter().enumerate() {
                    writeln!(
                        self.output,
                        "_Static_assert(offsetof({type_name},f{index})=={offset},\"DISP field layout mismatch\");"
                    )
                    .unwrap();
                }
            }
            hir::Type::Enum(id, arguments) => {
                let declaration = &self.program.enums[id.0];
                let substitutions = declaration
                    .generic_parameters
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect();
                self.enum_definition(
                    &type_name,
                    declaration
                        .variants
                        .iter()
                        .map(|variant| {
                            variant
                                .payload
                                .iter()
                                .map(|payload| layout::substitute(payload, &substitutions))
                                .collect()
                        })
                        .collect(),
                    concrete_layout.discriminant_size,
                    concrete_layout.payload_offset.unwrap_or(0),
                );
            }
            hir::Type::Option(inner) => self.enum_definition(
                &type_name,
                vec![vec![], vec![(**inner).clone()]],
                concrete_layout.discriminant_size,
                concrete_layout.payload_offset.unwrap_or(0),
            ),
            hir::Type::Result(ok, error) => self.enum_definition(
                &type_name,
                vec![vec![(**ok).clone()], vec![(**error).clone()]],
                concrete_layout.discriminant_size,
                concrete_layout.payload_offset.unwrap_or(0),
            ),
            _ => unreachable!(),
        }
        writeln!(
            self.output,
            "_Static_assert(sizeof({type_name})=={},\"DISP type size mismatch\");",
            concrete_layout.size
        )
        .unwrap();
        writeln!(
            self.output,
            "_Static_assert(_Alignof({type_name})=={},\"DISP type alignment mismatch\");",
            concrete_layout.align
        )
        .unwrap();
        Ok(())
    }

    fn enum_definition(
        &mut self,
        type_name: &str,
        variants: Vec<Vec<hir::Type>>,
        discriminant_size: u64,
        payload_offset: u64,
    ) {
        let tag = match discriminant_size {
            1 => "uint8_t",
            2 => "uint16_t",
            _ => "uint32_t",
        };
        writeln!(self.output, "struct {type_name} {{ {tag} tag; union {{").unwrap();
        for (variant, fields) in variants.iter().enumerate() {
            writeln!(self.output, "struct {{").unwrap();
            if fields.is_empty() {
                writeln!(self.output, "unsigned char _zero[0];").unwrap();
            }
            for (index, field) in fields.iter().enumerate() {
                writeln!(self.output, "{} f{index};", c_type(field)).unwrap();
            }
            writeln!(self.output, "}} v{variant};").unwrap();
        }
        writeln!(self.output, "}} payload; }};").unwrap();
        writeln!(
            self.output,
            "_Static_assert(offsetof({type_name},payload)=={payload_offset},\"DISP enum payload offset mismatch\");"
        )
        .unwrap();
    }
}

fn dependencies(program: &hir::Program, ty: &hir::Type) -> Vec<hir::Type> {
    match ty {
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            declaration
                .fields
                .iter()
                .map(|field| layout::substitute(&field.ty, &substitutions))
                .filter(is_aggregate)
                .collect()
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            declaration
                .variants
                .iter()
                .flat_map(|variant| &variant.payload)
                .map(|payload| layout::substitute(payload, &substitutions))
                .filter(is_aggregate)
                .collect()
        }
        hir::Type::Option(inner) => vec![(**inner).clone()],
        hir::Type::Result(ok, error) => vec![(**ok).clone(), (**error).clone()],
        hir::Type::Array(element, _) => vec![(**element).clone()],
        hir::Type::Slice(element) | hir::Type::List(element) | hir::Type::Set(element) => {
            vec![(**element).clone()]
        }
        hir::Type::Map(key, value) => vec![(**key).clone(), (**value).clone()],
        hir::Type::Thread(_)
        | hir::Type::Future(_)
        | hir::Type::Task(_)
        | hir::Type::Mutex(_)
        | hir::Type::MutexGuard(_) => vec![],
        hir::Type::IpAddress
        | hir::Type::SocketAddress
        | hir::Type::TcpStream
        | hir::Type::TcpListener
        | hir::Type::UdpSocket
        | hir::Type::UdpDatagram => vec![],
        _ => vec![],
    }
}

fn is_aggregate(ty: &hir::Type) -> bool {
    matches!(
        ty,
        hir::Type::Struct(_, _)
            | hir::Type::Enum(_, _)
            | hir::Type::Option(_)
            | hir::Type::Result(_, _)
            | hir::Type::Array(_, _)
            | hir::Type::Slice(_)
            | hir::Type::List(_)
            | hir::Type::Map(_, _)
            | hir::Type::Set(_)
    )
}

pub fn name(ty: &hir::Type) -> String {
    format!("disp_t_{}", mono::type_code(ty))
}

pub fn c_type(ty: &hir::Type) -> String {
    match ty {
        hir::Type::Unit => "disp_native_unit".into(),
        hir::Type::Bool => "bool".into(),
        hir::Type::Char => "uint32_t".into(),
        hir::Type::String => "disp_native_string".into(),
        hir::Type::CString => "disp_native_cstring".into(),
        hir::Type::CStr => "const char *".into(),
        hir::Type::Memory => "disp_native_memory".into(),
        hir::Type::Path => "disp_native_path".into(),
        hir::Type::IpAddress => "disp_native_ip_address".into(),
        hir::Type::SocketAddress => "disp_native_socket_address".into(),
        hir::Type::TcpStream => "disp_native_tcp_stream".into(),
        hir::Type::TcpListener => "disp_native_tcp_listener".into(),
        hir::Type::UdpSocket => "disp_native_udp_socket".into(),
        hir::Type::UdpDatagram => "disp_native_udp_datagram".into(),
        hir::Type::Instant => "disp_native_instant".into(),
        hir::Type::Duration => "disp_native_duration".into(),
        hir::Type::Thread(_) => "disp_native_thread".into(),
        hir::Type::Future(_) => "disp_native_future".into(),
        hir::Type::Task(_) => "disp_native_task".into(),
        hir::Type::Mutex(_) => "disp_native_mutex".into(),
        hir::Type::MutexGuard(_) => "disp_native_mutex_guard".into(),
        hir::Type::AtomicInt => "disp_native_atomic_int".into(),
        hir::Type::Str => "disp_native_str".into(),
        hir::Type::Array(_, _) => name(ty),
        hir::Type::Slice(_) => name(ty),
        hir::Type::List(_) => name(ty),
        hir::Type::Map(_, _) | hir::Type::Set(_) => name(ty),
        hir::Type::Int { signed, width } => {
            let width = width.unwrap_or(64);
            if width == 128 {
                if *signed {
                    "__int128"
                } else {
                    "unsigned __int128"
                }
                .into()
            } else {
                format!("{}int{width}_t", if *signed { "" } else { "u" })
            }
        }
        hir::Type::Float { width: 32 } => "float".into(),
        hir::Type::Float { .. } => "double".into(),
        hir::Type::Reference { mutable, inner } | hir::Type::RawPointer { mutable, inner } => {
            format!("{}{}*", if *mutable { "" } else { "const " }, c_type(inner))
        }
        hir::Type::Struct(_, _)
        | hir::Type::Enum(_, _)
        | hir::Type::Option(_)
        | hir::Type::Result(_, _) => name(ty),
        hir::Type::Generic(_) => "disp_native_string".into(),
        hir::Type::Function(_, _) => "disp_native_callable".into(),
        hir::Type::Unknown => "void".into(),
    }
}
