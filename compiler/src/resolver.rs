use crate::ast::{
    BindingKind, Block, Expr, Expression, Pattern, Program, Statement, TypeName, TypeQualifier,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolKind {
    Builtin,
    Function,
    Variant { enum_name: String },
    Parameter,
    Local { mutable: bool, constant: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub declaration: Span,
}

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub symbols: Vec<Symbol>,
    pub declarations: HashMap<Span, SymbolId>,
    pub uses: HashMap<Span, SymbolId>,
}

pub struct Resolver {
    resolution: Resolution,
    globals: HashMap<String, SymbolId>,
    types: HashMap<String, (Span, usize)>,
    generic_types: Vec<HashMap<String, Span>>,
    traits: HashMap<String, (Span, usize)>,
    variants: HashMap<String, Vec<(String, SymbolId)>>,
    scopes: Vec<HashMap<String, SymbolId>>,
    loop_depth: usize,
}

impl Resolver {
    pub fn new() -> Self {
        let mut resolver = Self {
            resolution: Resolution::default(),
            globals: HashMap::new(),
            types: HashMap::new(),
            generic_types: Vec::new(),
            traits: HashMap::new(),
            variants: HashMap::new(),
            scopes: Vec::new(),
            loop_depth: 0,
        };
        for builtin in [
            "print", "Some", "None", "Ok", "Err", "i8", "i16", "i32", "i64", "i128", "u8", "u16",
            "u32", "u64", "u128", "int", "uint", "f32", "f64", "CString", "Memory",
        ] {
            let id = resolver.add_symbol(builtin, SymbolKind::Builtin, Span::point(1, 1));
            resolver.globals.insert(builtin.into(), id);
        }
        resolver
    }

    pub fn resolve(mut self, program: &Program) -> Result<Resolution, Diagnostic> {
        for declaration in &program.traits {
            if self
                .traits
                .insert(
                    declaration.name.clone(),
                    (declaration.name_span, declaration.generics.len()),
                )
                .is_some()
                || self.types.contains_key(&declaration.name)
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Resolve,
                    format!("duplicate trait `{}`", declaration.name),
                    declaration.name_span,
                ));
            }
        }
        self.traits
            .entry("Copy".into())
            .or_insert((Span::point(1, 1), 0));
        for declaration in &program.structs {
            self.declare_type(
                &declaration.name,
                declaration.name_span,
                declaration.generics.len(),
            )?;
            self.validate_unique_fields(
                &declaration
                    .fields
                    .iter()
                    .map(|field| (&field.name, field.name_span))
                    .collect::<Vec<_>>(),
            )?;
        }
        for declaration in &program.enums {
            self.declare_type(
                &declaration.name,
                declaration.name_span,
                declaration.generics.len(),
            )?;
            self.validate_unique_fields(
                &declaration
                    .variants
                    .iter()
                    .map(|variant| (&variant.name, variant.name_span))
                    .collect::<Vec<_>>(),
            )?;
            for variant in &declaration.variants {
                let id = self.add_symbol(
                    variant.name.clone(),
                    SymbolKind::Variant {
                        enum_name: declaration.name.clone(),
                    },
                    variant.name_span,
                );
                self.variants
                    .entry(variant.name.clone())
                    .or_default()
                    .push((declaration.name.clone(), id));
                self.resolution.declarations.insert(variant.name_span, id);
            }
        }

        for declaration in &program.structs {
            self.begin_generics(&declaration.generics)?;
            for field in &declaration.fields {
                self.resolve_type_name(&field.ty)?;
            }
            self.generic_types.pop();
        }
        for declaration in &program.enums {
            self.begin_generics(&declaration.generics)?;
            for variant in &declaration.variants {
                for ty in &variant.payload {
                    self.resolve_type_name(ty)?;
                }
            }
            self.generic_types.pop();
        }
        for function in &program.functions {
            if self.globals.contains_key(&function.name) || self.types.contains_key(&function.name)
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Resolve,
                    format!("duplicate function `{}`", function.name),
                    function.name_span,
                )
                .with_help("function names must be unique and cannot replace builtins"));
            }
            let id = self.add_symbol(
                function.name.clone(),
                SymbolKind::Function,
                function.name_span,
            );
            self.globals.insert(function.name.clone(), id);
            self.resolution.declarations.insert(function.name_span, id);
        }

        for function in &program.functions {
            self.begin_generics(&function.generics)?;
            for parameter in &function.parameters {
                self.resolve_type_name(&parameter.ty)?;
            }
            if let Some(return_type) = &function.return_type {
                self.resolve_type_name(return_type)?;
            }
            self.begin_scope();
            for parameter in &function.parameters {
                self.declare_local(&parameter.name, parameter.name_span, SymbolKind::Parameter)?;
            }
            self.resolve_block_contents(&function.body)?;
            self.end_scope();
            self.generic_types.pop();
        }
        for declaration in &program.traits {
            self.begin_generics(&declaration.generics)?;
            self.generic_types
                .last_mut()
                .unwrap()
                .insert("Self".into(), declaration.name_span);
            for method in &declaration.methods {
                for parameter in &method.parameters {
                    self.resolve_type_name(&parameter.ty)?;
                }
                if let Some(result) = &method.return_type {
                    self.resolve_type_name(result)?;
                }
            }
            self.generic_types.pop();
        }
        for implementation in &program.implementations {
            self.begin_generics(&implementation.generics)?;
            if let Some(trait_name) = &implementation.trait_name {
                self.resolve_trait_name(trait_name)?;
            }
            self.resolve_type_name(&implementation.target)?;
            for (_, ty, _) in &implementation.associated_types {
                self.resolve_type_name(ty)?;
            }
            for method in &implementation.methods {
                self.generic_types
                    .last_mut()
                    .unwrap()
                    .insert("Self".into(), method.name_span);
                for parameter in &method.parameters {
                    self.resolve_type_name(&parameter.ty)?;
                }
                if let Some(result) = &method.return_type {
                    self.resolve_type_name(result)?;
                }
                self.begin_scope();
                for parameter in &method.parameters {
                    self.declare_local(
                        &parameter.name,
                        parameter.name_span,
                        SymbolKind::Parameter,
                    )?;
                }
                self.resolve_block_contents(&method.body)?;
                self.end_scope();
            }
            self.generic_types.pop();
        }
        Ok(self.resolution)
    }

    fn resolve_block(&mut self, block: &Block) -> Result<(), Diagnostic> {
        self.begin_scope();
        let result = self.resolve_block_contents(block);
        self.end_scope();
        result
    }

    fn resolve_block_contents(&mut self, block: &Block) -> Result<(), Diagnostic> {
        for statement in &block.statements {
            self.resolve_statement(&statement.node, statement.span)?;
        }
        Ok(())
    }

    fn resolve_statement(&mut self, statement: &Statement, span: Span) -> Result<(), Diagnostic> {
        match statement {
            Statement::Binding {
                kind,
                name,
                name_span,
                annotation,
                value,
            } => {
                if let Some(annotation) = annotation {
                    self.resolve_type_name(annotation)?;
                }
                if let Some(value) = value {
                    self.resolve_expression(value)?;
                }
                let symbol_kind = SymbolKind::Local {
                    mutable: *kind == BindingKind::Var,
                    constant: *kind == BindingKind::Const,
                };
                self.declare_local(name, *name_span, symbol_kind)
            }
            Statement::Assignment {
                name,
                name_span,
                value,
                operator,
            } => {
                let Some(id) = self.lookup(name) else {
                    if *operator != crate::ast::AssignmentOperator::Assign {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Resolve,
                            format!("unknown name `{name}`"),
                            *name_span,
                        ));
                    }
                    self.resolve_expression(value)?;
                    return self.declare_local(
                        name,
                        *name_span,
                        SymbolKind::Local {
                            mutable: true,
                            constant: false,
                        },
                    );
                };
                let symbol = &self.resolution.symbols[id.0];
                let mutable = matches!(symbol.kind, SymbolKind::Local { mutable: true, .. });
                if !mutable {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Resolve,
                        format!("cannot assign to immutable binding `{name}`"),
                        *name_span,
                    )
                    .with_help("declare the binding with `var` if mutation is required"));
                }
                self.resolution.uses.insert(*name_span, id);
                self.resolve_expression(value)
            }
            Statement::PlaceAssignment { target, value, .. } => {
                self.resolve_expression(target)?;
                self.resolve_expression(value)
            }
            Statement::Expression(expression) | Statement::Return(Some(expression)) => {
                self.resolve_expression(expression)
            }
            Statement::Return(None) => Ok(()),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.resolve_expression(condition)?;
                self.resolve_block(then_branch)?;
                if let Some(branch) = else_branch {
                    self.resolve_block(branch)?;
                }
                Ok(())
            }
            Statement::While { condition, body } => {
                self.resolve_expression(condition)?;
                self.with_loop(|resolver| resolver.resolve_block(body))
            }
            Statement::For {
                name,
                name_span,
                start,
                end,
                body,
                ..
            } => {
                self.resolve_expression(start)?;
                self.resolve_expression(end)?;
                self.with_loop(|resolver| {
                    resolver.begin_scope();
                    let result = resolver
                        .declare_local(
                            name,
                            *name_span,
                            SymbolKind::Local {
                                mutable: false,
                                constant: false,
                            },
                        )
                        .and_then(|()| resolver.resolve_block_contents(body));
                    resolver.end_scope();
                    result
                })
            }
            Statement::ForEach {
                name,
                name_span,
                iterable,
                body,
            } => {
                self.resolve_expression(iterable)?;
                self.with_loop(|resolver| {
                    resolver.begin_scope();
                    let result = resolver
                        .declare_local(
                            name,
                            *name_span,
                            SymbolKind::Local {
                                mutable: false,
                                constant: false,
                            },
                        )
                        .and_then(|()| resolver.resolve_block_contents(body));
                    resolver.end_scope();
                    result
                })
            }
            Statement::Loop(body) => self.with_loop(|resolver| resolver.resolve_block(body)),
            Statement::Unsafe(body) => self.resolve_block(body),
            Statement::Break | Statement::Continue if self.loop_depth == 0 => Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                "`break` and `continue` are only valid inside loops",
                span,
            )),
            Statement::Break | Statement::Continue => Ok(()),
        }
    }

    fn resolve_expression(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        match &expression.node {
            Expression::Array(values) => {
                for value in values {
                    self.resolve_expression(value)?;
                }
                Ok(())
            }
            Expression::Closure {
                parameters,
                return_type,
                body,
                ..
            } => {
                for parameter in parameters {
                    self.resolve_type_name(&parameter.ty)?;
                }
                if let Some(return_type) = return_type {
                    self.resolve_type_name(return_type)?;
                }
                let outer_loop_depth = std::mem::replace(&mut self.loop_depth, 0);
                self.begin_scope();
                let result = (|| {
                    for parameter in parameters {
                        self.declare_local(
                            &parameter.name,
                            parameter.name_span,
                            SymbolKind::Local {
                                mutable: false,
                                constant: false,
                            },
                        )?;
                    }
                    match body {
                        crate::ast::ClosureBody::Expression(expression) => {
                            self.resolve_expression(expression)
                        }
                        crate::ast::ClosureBody::Block(block) => self.resolve_block_contents(block),
                    }
                })();
                self.end_scope();
                self.loop_depth = outer_loop_depth;
                result
            }
            Expression::Index { object, index } => {
                self.resolve_expression(object)?;
                self.resolve_expression(index)
            }
            Expression::Subslice { object, start, end } => {
                self.resolve_expression(object)?;
                self.resolve_expression(start)?;
                self.resolve_expression(end)
            }
            Expression::Identifier(name) => {
                if matches!(
                    name.as_str(),
                    "String"
                        | "List"
                        | "Map"
                        | "Set"
                        | "Path"
                        | "File"
                        | "Directory"
                        | "Time"
                        | "Duration"
                        | "Async"
                ) {
                    return Ok(());
                }
                let id = if let Some(id) = self.lookup(name) {
                    id
                } else if let Some(candidates) = self.variants.get(name) {
                    if candidates.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Resolve,
                            format!("ambiguous enum variant `{name}`"),
                            expression.span,
                        )
                        .with_help("qualify the variant as `EnumName.Variant`"));
                    }
                    candidates[0].1
                } else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Resolve,
                        format!("unknown name `{name}`"),
                        expression.span,
                    ));
                };
                self.resolution.uses.insert(expression.span, id);
                Ok(())
            }
            Expression::StructConstruct { name, fields, .. } => {
                if !self.types.contains_key(name) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Resolve,
                        format!("unknown struct type `{name}`"),
                        expression.span,
                    ));
                }
                for field in fields {
                    self.resolve_expression(&field.value)?;
                }
                Ok(())
            }
            Expression::FieldAccess {
                object,
                field,
                field_span,
            } => {
                if matches!(&object.node, Expression::Identifier(name) if matches!(name.as_str(), "String" | "List" | "Map" | "Set" | "Mutex" | "AtomicInt" | "Path" | "File" | "Directory" | "Time" | "Duration" | "Async"))
                {
                    return Ok(());
                }
                if let Expression::Identifier(type_name) = &object.node
                    && self.types.contains_key(type_name)
                {
                    let candidates = self.variants.get(field).ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Resolve,
                            format!("unknown enum variant `{type_name}.{field}`"),
                            *field_span,
                        )
                    })?;
                    if let Some((_, id)) = candidates.iter().find(|(owner, _)| owner == type_name) {
                        self.resolution.uses.insert(*field_span, *id);
                        return Ok(());
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Resolve,
                        format!("unknown enum variant `{type_name}.{field}`"),
                        *field_span,
                    ));
                }
                self.resolve_expression(object)
            }
            Expression::Match { value, arms } => {
                self.resolve_expression(value)?;
                for arm in arms {
                    self.begin_scope();
                    let result = self
                        .resolve_pattern(&arm.pattern.node, arm.pattern.span)
                        .and_then(|()| self.resolve_expression(&arm.value));
                    self.end_scope();
                    result?;
                }
                Ok(())
            }
            Expression::Try(operand)
            | Expression::Await(operand)
            | Expression::Spawn(operand)
            | Expression::Move(operand)
            | Expression::Dereference(operand) => self.resolve_expression(operand),
            Expression::Borrow { target, .. } => self.resolve_expression(target),
            Expression::Unary { operand, .. } => self.resolve_expression(operand),
            Expression::Binary { left, right, .. } => {
                self.resolve_expression(left)?;
                self.resolve_expression(right)
            }
            Expression::Call { callee, arguments } => {
                self.resolve_expression(callee)?;
                for argument in arguments {
                    self.resolve_expression(argument)?;
                }
                Ok(())
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => Ok(()),
        }
    }

    fn resolve_pattern(&mut self, pattern: &Pattern, span: Span) -> Result<(), Diagnostic> {
        match pattern {
            Pattern::Binding(name) => self.declare_local(
                name,
                span,
                SymbolKind::Local {
                    mutable: false,
                    constant: false,
                },
            ),
            Pattern::Variant {
                type_name,
                variant,
                arguments,
            } => {
                let candidates = self.variants.get(variant);
                if !matches!(variant.as_str(), "Some" | "None" | "Ok" | "Err") {
                    let valid = candidates.is_some_and(|values| {
                        type_name.as_ref().map_or(!values.is_empty(), |owner| {
                            values.iter().any(|(candidate, _)| candidate == owner)
                        })
                    });
                    if !valid {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Resolve,
                            format!("unknown enum variant `{variant}`"),
                            span,
                        ));
                    }
                    if type_name.is_none() && candidates.is_some_and(|values| values.len() > 1) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Resolve,
                            format!("ambiguous enum variant `{variant}`"),
                            span,
                        )
                        .with_help("qualify the pattern as `EnumName.Variant`"));
                    }
                }
                for argument in arguments {
                    self.resolve_pattern(&argument.node, argument.span)?;
                }
                Ok(())
            }
            Pattern::Wildcard
            | Pattern::Integer(_)
            | Pattern::String(_)
            | Pattern::Character(_)
            | Pattern::Bool(_) => Ok(()),
        }
    }

    fn declare_local(
        &mut self,
        name: &str,
        span: Span,
        kind: SymbolKind,
    ) -> Result<(), Diagnostic> {
        if self
            .scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name))
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!("duplicate name `{name}` in this scope"),
                span,
            )
            .with_help("rename this declaration or remove the earlier declaration"));
        }
        let id = self.add_symbol(name, kind, span);
        self.scopes
            .last_mut()
            .expect("resolver always has a local scope while resolving a function")
            .insert(name.into(), id);
        self.resolution.declarations.insert(span, id);
        Ok(())
    }

    fn declare_type(&mut self, name: &str, span: Span, arity: usize) -> Result<(), Diagnostic> {
        if self.types.contains_key(name) || self.globals.contains_key(name) {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!("duplicate type `{name}`"),
                span,
            ));
        }
        self.types.insert(name.into(), (span, arity));
        Ok(())
    }

    fn begin_generics(
        &mut self,
        parameters: &[crate::ast::GenericParameter],
    ) -> Result<(), Diagnostic> {
        let mut scope = HashMap::new();
        for parameter in parameters {
            if scope
                .insert(parameter.name.clone(), parameter.name_span)
                .is_some()
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Resolve,
                    format!("duplicate generic parameter `{}`", parameter.name),
                    parameter.name_span,
                ));
            }
        }
        self.generic_types.push(scope);
        for parameter in parameters {
            for constraint in &parameter.constraints {
                self.resolve_trait_name(constraint)?;
            }
        }
        Ok(())
    }

    fn validate_unique_fields(&self, names: &[(&String, Span)]) -> Result<(), Diagnostic> {
        let mut seen = HashMap::new();
        for (name, span) in names {
            if seen.insert(name.as_str(), *span).is_some() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Resolve,
                    format!("duplicate declaration `{name}`"),
                    *span,
                ));
            }
        }
        Ok(())
    }

    fn resolve_type_name(&self, ty: &TypeName) -> Result<(), Diagnostic> {
        if ty.name == "str" {
            if ty.qualifier == TypeQualifier::SharedReference && ty.arguments.is_empty() {
                return Ok(());
            }
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                "`str` is an immutable borrowed type and must be written as `&str`",
                ty.span,
            ));
        }
        if ty.qualifier != TypeQualifier::Owned {
            return match ty.qualifier {
                TypeQualifier::SharedReference | TypeQualifier::MutableReference => {
                    let mut owned = ty.clone();
                    owned.qualifier = TypeQualifier::Owned;
                    self.resolve_type_name(&owned)
                }
                TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer
                    if ty.name == "ptr" && ty.arguments.len() == 1 =>
                {
                    self.resolve_type_name(&ty.arguments[0])
                }
                TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer => {
                    Err(Diagnostic::new(
                        DiagnosticKind::Resolve,
                        "raw pointer type must be `ptr<T>` or `mut ptr<T>`",
                        ty.span,
                    ))
                }
                TypeQualifier::Owned => unreachable!(),
            };
        }
        let expected_arguments = match ty.name.as_str() {
            "fn" => {
                if ty.arguments.is_empty() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Resolve,
                        "function type is missing its result type",
                        ty.span,
                    ));
                }
                for argument in &ty.arguments {
                    self.resolve_type_name(argument)?;
                }
                return Ok(());
            }
            "Option" => Some(1),
            "Result" => Some(2),
            "List" => Some(1),
            "Map" => Some(2),
            "Set" => Some(1),
            "Thread" | "Future" => Some(1),
            "Mutex" | "MutexGuard" => Some(1),
            "AtomicInt" => Some(0),
            "CString" | "CStr" | "Memory" | "CInt" | "CUInt" | "CSize" | "CSSize" | "CChar"
            | "CUChar" | "CShort" | "CUShort" | "CLongLong" | "CULongLong" | "CFloat"
            | "CDouble" => Some(0),
            "[]" => Some(1),
            name if name.starts_with("[;") && name.ends_with(']') => Some(1),
            "int" | "f64" | "str" | "String" | "Path" | "Instant" | "Duration" | "IoError"
            | "char" | "bool" | "Unit" | "ConversionError" => Some(0),
            name if self
                .generic_types
                .last()
                .is_some_and(|scope| scope.contains_key(name)) =>
            {
                Some(0)
            }
            name if self.types.contains_key(name) => Some(self.types[name].1),
            "i8" | "i16" | "i32" | "i64" | "i128" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "uint" | "f32" => Some(0),
            _ => None,
        }
        .ok_or_else(|| {
            Diagnostic::new(
                DiagnosticKind::Resolve,
                format!("unknown type `{}`", ty.name),
                ty.span,
            )
        })?;
        if ty.arguments.len() != expected_arguments {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!(
                    "type `{}` expects {expected_arguments} type arguments, found {}",
                    ty.name,
                    ty.arguments.len()
                ),
                ty.span,
            ));
        }
        for argument in &ty.arguments {
            self.resolve_type_name(argument)?;
        }
        Ok(())
    }

    fn resolve_trait_name(&self, ty: &TypeName) -> Result<(), Diagnostic> {
        let Some((_, arity)) = self.traits.get(&ty.name) else {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!("unknown trait `{}`", ty.name),
                ty.span,
            ));
        };
        if ty.arguments.len() != *arity {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!(
                    "trait `{}` expects {arity} type arguments, found {}",
                    ty.name,
                    ty.arguments.len()
                ),
                ty.span,
            ));
        }
        for argument in &ty.arguments {
            self.resolve_type_name(argument)?;
        }
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<SymbolId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.globals.get(name).copied())
    }

    fn add_symbol(&mut self, name: impl Into<String>, kind: SymbolKind, span: Span) -> SymbolId {
        let id = SymbolId(self.resolution.symbols.len());
        self.resolution.symbols.push(Symbol {
            id,
            name: name.into(),
            kind,
            declaration: span,
        });
        id
    }

    fn begin_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
    }

    fn with_loop<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        self.loop_depth += 1;
        let result = operation(self);
        self.loop_depth -= 1;
        result
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer::Lexer, parser::Parser};

    fn resolve(source: &str) -> Result<Resolution, Diagnostic> {
        let program = Parser::new(Lexer::new(source).tokenize()?).parse()?;
        Resolver::new().resolve(&program)
    }

    #[test]
    fn resolves_forward_functions_and_lexical_shadowing() {
        assert!(resolve(
            "fn main() { let x = 1 if true { let x = 2 print(x) } print(helper(x)) } fn helper(value: i32) -> i32 { return value }"
        )
        .is_ok());
    }

    #[test]
    fn rejects_unknown_and_duplicate_names() {
        assert!(resolve("fn main() { print(missing) }").is_err());
        assert!(resolve("fn main() { let x = 1 let x = 2 }").is_err());
        assert!(resolve("fn main() {} fn main() {}").is_err());
    }

    #[test]
    fn enforces_mutability_and_loop_context() {
        assert!(resolve("fn main() { let x = 1 x = 2 }").is_err());
        assert!(resolve("fn main() { var x = 1 x = 2 }").is_ok());
        assert!(resolve("fn main() { break }").is_err());
    }
}
