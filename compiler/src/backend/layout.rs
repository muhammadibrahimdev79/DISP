use super::target::Target;
use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    hir,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
    pub fields: Vec<u64>,
    pub payload_offset: Option<u64>,
    pub discriminant_size: u64,
}

pub struct LayoutEngine<'a> {
    target: Target,
    program: &'a hir::Program,
    cache: HashMap<hir::Type, Layout>,
    active: HashSet<hir::Type>,
    generic_names: HashSet<String>,
}

impl<'a> LayoutEngine<'a> {
    pub fn new(target: Target, program: &'a hir::Program) -> Self {
        let generic_names = program
            .functions
            .iter()
            .flat_map(|function| function.generic_parameters.iter().cloned())
            .chain(
                program
                    .structs
                    .iter()
                    .flat_map(|declaration| declaration.generic_parameters.iter().cloned()),
            )
            .chain(
                program
                    .enums
                    .iter()
                    .flat_map(|declaration| declaration.generic_parameters.iter().cloned()),
            )
            .collect();
        Self {
            target,
            program,
            cache: HashMap::new(),
            active: HashSet::new(),
            generic_names,
        }
    }
    pub fn layout(&mut self, ty: &hir::Type) -> Result<Layout, Diagnostic> {
        if let Some(layout) = self.cache.get(ty) {
            return Ok(layout.clone());
        }
        if !self.active.insert(ty.clone()) {
            return Err(self.error("recursive by-value type has infinite native layout"));
        }
        let layout = match ty {
            hir::Type::Unit => scalar(0, 1),
            hir::Type::Bool => scalar(1, 1),
            hir::Type::Char => scalar(4, 4),
            hir::Type::String | hir::Type::CString | hir::Type::Path => aggregate(&[
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
            ]),
            hir::Type::Str => {
                aggregate(&[self.target.pointer_alignment, self.target.pointer_alignment])
            }
            hir::Type::CStr => scalar(
                u64::from(self.target.pointer_width) / 8,
                self.target.pointer_alignment,
            ),
            hir::Type::Array(element, length) => {
                let element = self.layout(element)?;
                Layout {
                    size: element
                        .size
                        .checked_mul(*length as u64)
                        .ok_or_else(|| self.error("array layout size overflow"))?,
                    align: element.align,
                    fields: (0..*length)
                        .map(|index| element.size * index as u64)
                        .collect(),
                    payload_offset: None,
                    discriminant_size: 0,
                }
            }
            hir::Type::Slice(_) => {
                aggregate(&[self.target.pointer_alignment, self.target.pointer_alignment])
            }
            hir::Type::List(_) => aggregate(&[
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
            ]),
            hir::Type::Map(_, _) => aggregate(&[
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
            ]),
            hir::Type::Set(_) => aggregate(&[
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
            ]),
            hir::Type::Thread(_) => {
                aggregate(&[self.target.pointer_alignment, self.target.pointer_alignment])
            }
            hir::Type::Mutex(_) | hir::Type::MutexGuard(_) | hir::Type::AtomicInt => {
                aggregate(&[self.target.pointer_alignment])
            }
            hir::Type::Instant | hir::Type::Duration => scalar(8, 8),
            hir::Type::Int { width, .. } => {
                let bytes = u64::from(width.unwrap_or(self.target.pointer_width)) / 8;
                scalar(bytes, bytes.min(16))
            }
            hir::Type::Float { width } => {
                let bytes = u64::from(*width) / 8;
                scalar(bytes, bytes)
            }
            hir::Type::Reference { .. }
            | hir::Type::RawPointer { .. }
            | hir::Type::Function(_, _) => scalar(
                u64::from(self.target.pointer_width) / 8,
                self.target.pointer_alignment,
            ),
            hir::Type::Struct(id, arguments) => {
                let declaration = self
                    .program
                    .structs
                    .get(id.0)
                    .ok_or_else(|| self.error("unknown struct in layout"))?;
                let substitutions = declaration_type_arguments(declaration, arguments);
                let layouts = declaration
                    .fields
                    .iter()
                    .map(|field| self.layout(&substitute(&field.ty, &substitutions)))
                    .collect::<Result<Vec<_>, _>>()?;
                aggregate_layout(&layouts)
            }
            hir::Type::Enum(id, arguments) => {
                let declaration = self
                    .program
                    .enums
                    .get(id.0)
                    .ok_or_else(|| self.error("unknown enum in layout"))?;
                let substitutions = enum_type_arguments(declaration, arguments);
                let variants = declaration
                    .variants
                    .iter()
                    .map(|variant| {
                        variant
                            .payload
                            .iter()
                            .map(|ty| self.layout(&substitute(ty, &substitutions)))
                            .collect::<Result<Vec<_>, _>>()
                            .map(|fields| aggregate_layout(&fields))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                enum_layout(&variants)
            }
            hir::Type::Option(inner) => enum_layout(&[scalar(0, 1), self.layout(inner)?]),
            hir::Type::Result(ok, error) => enum_layout(&[self.layout(ok)?, self.layout(error)?]),
            hir::Type::Generic(name) if self.generic_names.contains(name) => {
                return Err(self.error(&format!(
                    "unresolved generic `{name}` has no concrete layout"
                )));
            }
            hir::Type::Generic(_) => aggregate(&[
                self.target.pointer_alignment,
                self.target.pointer_alignment,
                self.target.pointer_alignment,
            ]),
            hir::Type::Unknown => return Err(self.error("unknown type has no native layout")),
        };
        self.active.remove(ty);
        self.cache.insert(ty.clone(), layout.clone());
        Ok(layout)
    }
    fn error(&self, message: &str) -> Diagnostic {
        Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
    }
}

fn scalar(size: u64, align: u64) -> Layout {
    Layout {
        size,
        align: align.max(1),
        fields: vec![],
        payload_offset: None,
        discriminant_size: 0,
    }
}
fn align_to(value: u64, align: u64) -> u64 {
    value.div_ceil(align) * align
}
fn aggregate(sizes: &[u64]) -> Layout {
    let layouts = sizes
        .iter()
        .map(|size| scalar(*size, *size))
        .collect::<Vec<_>>();
    aggregate_layout(&layouts)
}
fn aggregate_layout(fields: &[Layout]) -> Layout {
    let align = fields.iter().map(|x| x.align).max().unwrap_or(1);
    let mut size = 0;
    let mut offsets = Vec::new();
    for field in fields {
        size = align_to(size, field.align);
        offsets.push(size);
        size += field.size;
    }
    Layout {
        size: align_to(size, align),
        align,
        fields: offsets,
        payload_offset: None,
        discriminant_size: 0,
    }
}
fn enum_layout(variants: &[Layout]) -> Layout {
    let payload_align = variants.iter().map(|x| x.align).max().unwrap_or(1);
    let payload_size = variants.iter().map(|x| x.size).max().unwrap_or(0);
    let discriminant_size = if variants.len() <= 256 {
        1
    } else if variants.len() <= 65_536 {
        2
    } else {
        4
    };
    let payload_offset = align_to(discriminant_size, payload_align);
    let align = payload_align.max(discriminant_size);
    Layout {
        size: align_to(payload_offset + payload_size, align),
        align,
        fields: vec![],
        payload_offset: Some(payload_offset),
        discriminant_size,
    }
}
fn declaration_type_arguments(
    declaration: &hir::Struct,
    arguments: &[hir::Type],
) -> HashMap<String, hir::Type> {
    declaration
        .generic_parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect()
}
fn enum_type_arguments(
    declaration: &hir::Enum,
    arguments: &[hir::Type],
) -> HashMap<String, hir::Type> {
    declaration
        .generic_parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect()
}
pub fn substitute(ty: &hir::Type, substitutions: &HashMap<String, hir::Type>) -> hir::Type {
    match ty {
        hir::Type::Generic(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        hir::Type::Reference { mutable, inner } => hir::Type::Reference {
            mutable: *mutable,
            inner: Box::new(substitute(inner, substitutions)),
        },
        hir::Type::RawPointer { mutable, inner } => hir::Type::RawPointer {
            mutable: *mutable,
            inner: Box::new(substitute(inner, substitutions)),
        },
        hir::Type::Struct(id, args) => hir::Type::Struct(
            *id,
            args.iter().map(|x| substitute(x, substitutions)).collect(),
        ),
        hir::Type::Enum(id, args) => hir::Type::Enum(
            *id,
            args.iter().map(|x| substitute(x, substitutions)).collect(),
        ),
        hir::Type::Option(x) => hir::Type::Option(Box::new(substitute(x, substitutions))),
        hir::Type::Result(a, b) => hir::Type::Result(
            Box::new(substitute(a, substitutions)),
            Box::new(substitute(b, substitutions)),
        ),
        hir::Type::Function(args, result) => hir::Type::Function(
            args.iter().map(|x| substitute(x, substitutions)).collect(),
            Box::new(substitute(result, substitutions)),
        ),
        hir::Type::Array(element, length) => {
            hir::Type::Array(Box::new(substitute(element, substitutions)), *length)
        }
        hir::Type::Slice(element) => hir::Type::Slice(Box::new(substitute(element, substitutions))),
        hir::Type::List(element) => hir::Type::List(Box::new(substitute(element, substitutions))),
        hir::Type::Thread(result) => hir::Type::Thread(Box::new(substitute(result, substitutions))),
        hir::Type::Mutex(value) => hir::Type::Mutex(Box::new(substitute(value, substitutions))),
        hir::Type::MutexGuard(value) => {
            hir::Type::MutexGuard(Box::new(substitute(value, substitutions)))
        }
        _ => ty.clone(),
    }
}
