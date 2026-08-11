use crate::ast::{
    AssignmentOperator, BinaryOperator, BindingKind, Block, Expr, Expression, Function, Pattern,
    Program, Statement, TypeName, TypeQualifier, UnaryOperator,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    IntLiteral(u128),
    NegativeIntLiteral(i128),
    UInt,
    Signed(u16),
    Unsigned(u16),
    Float,
    Float32,
    FloatLiteral,
    String,
    Str,
    Path,
    Instant,
    Duration,
    Array(Box<Type>, usize),
    Slice(Box<Type>),
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Char,
    Bool,
    ConversionError,
    IoError,
    Unit,
    Struct(TypeId, Vec<Type>),
    Enum(TypeId, Vec<Type>),
    Generic(String),
    Reference(Box<Type>, bool),
    RawPointer(Box<Type>, bool),
    Option(Box<Type>),
    Result(Box<Type>, Box<Type>),
    Function(Vec<Type>, Box<Type>),
    Infer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(usize);

#[derive(Debug, Clone)]
struct StructInfo {
    name: String,
    generics: Vec<String>,
    constraints: Vec<Vec<String>>,
    fields: HashMap<String, Type>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    owner: TypeId,
    payload: Vec<Type>,
}

#[derive(Debug, Clone)]
struct EnumInfo {
    name: String,
    generics: Vec<String>,
    constraints: Vec<Vec<String>>,
    variants: HashMap<String, VariantInfo>,
}

#[derive(Debug, Clone)]
struct Variable {
    ty: Type,
    constant: bool,
}

#[derive(Debug, Clone)]
struct Signature {
    generics: Vec<String>,
    constraints: HashMap<String, Vec<String>>,
    parameters: Vec<Type>,
    result: Type,
}

#[derive(Debug, Clone)]
struct TraitInfo {
    generics: Vec<String>,
    methods: HashMap<String, Signature>,
    associated_types: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ImplInfo {
    trait_name: String,
    trait_arguments: Vec<Type>,
    target: Type,
}

pub struct TypeChecker {
    functions: HashMap<String, Signature>,
    types: HashMap<String, Type>,
    structs: HashMap<TypeId, StructInfo>,
    enums: HashMap<TypeId, EnumInfo>,
    variants: HashMap<String, Vec<VariantInfo>>,
    scopes: Vec<HashMap<String, Variable>>,
    expected_return: Type,
    generic_types: HashMap<String, Vec<String>>,
    traits: HashMap<String, TraitInfo>,
    implementations: Vec<ImplInfo>,
    unsafe_depth: usize,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            types: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            variants: HashMap::new(),
            scopes: Vec::new(),
            expected_return: Type::Unit,
            generic_types: HashMap::new(),
            traits: HashMap::new(),
            implementations: Vec::new(),
            unsafe_depth: 0,
        }
    }

    pub fn check(&mut self, program: &Program) -> Result<(), Diagnostic> {
        self.functions.clear();
        self.types.clear();
        self.structs.clear();
        self.enums.clear();
        self.variants.clear();
        self.traits.clear();
        self.implementations.clear();
        self.traits.insert(
            "Copy".into(),
            TraitInfo {
                generics: vec![],
                methods: HashMap::new(),
                associated_types: HashSet::new(),
            },
        );

        let mut next_type_id = 0usize;
        for declaration in &program.structs {
            let id = TypeId(next_type_id);
            next_type_id += 1;
            self.types
                .insert(declaration.name.clone(), Type::Struct(id, vec![]));
            self.structs.insert(
                id,
                StructInfo {
                    name: declaration.name.clone(),
                    generics: declaration
                        .generics
                        .iter()
                        .map(|p| p.name.clone())
                        .collect(),
                    constraints: declaration
                        .generics
                        .iter()
                        .map(|parameter| {
                            parameter
                                .constraints
                                .iter()
                                .map(|constraint| constraint.name.clone())
                                .collect()
                        })
                        .collect(),
                    fields: HashMap::new(),
                },
            );
        }
        for declaration in &program.enums {
            let id = TypeId(next_type_id);
            next_type_id += 1;
            self.types
                .insert(declaration.name.clone(), Type::Enum(id, vec![]));
            self.enums.insert(
                id,
                EnumInfo {
                    name: declaration.name.clone(),
                    generics: declaration
                        .generics
                        .iter()
                        .map(|p| p.name.clone())
                        .collect(),
                    constraints: declaration
                        .generics
                        .iter()
                        .map(|parameter| {
                            parameter
                                .constraints
                                .iter()
                                .map(|constraint| constraint.name.clone())
                                .collect()
                        })
                        .collect(),
                    variants: HashMap::new(),
                },
            );
        }
        for declaration in &program.structs {
            let Type::Struct(id, _) = self.types[&declaration.name] else {
                unreachable!();
            };
            self.set_generics(&declaration.generics);
            let fields = declaration
                .fields
                .iter()
                .map(|field| Ok((field.name.clone(), self.resolve_type(&field.ty)?)))
                .collect::<Result<HashMap<_, _>, Diagnostic>>()?;
            self.structs.get_mut(&id).unwrap().fields = fields;
        }
        for declaration in &program.enums {
            let Type::Enum(id, _) = self.types[&declaration.name] else {
                unreachable!();
            };
            self.set_generics(&declaration.generics);
            let mut variants = HashMap::new();
            for variant in &declaration.variants {
                let info = VariantInfo {
                    owner: id,
                    payload: variant
                        .payload
                        .iter()
                        .map(|ty| self.resolve_type(ty))
                        .collect::<Result<Vec<_>, _>>()?,
                };
                self.variants
                    .entry(variant.name.clone())
                    .or_default()
                    .push(info.clone());
                variants.insert(variant.name.clone(), info);
            }
            self.enums.get_mut(&id).unwrap().variants = variants;
        }

        for declaration in &program.traits {
            if self.traits.contains_key(&declaration.name)
                || self.types.contains_key(&declaration.name)
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("duplicate trait `{}`", declaration.name),
                    declaration.name_span,
                ));
            }
            self.set_generics(&declaration.generics);
            self.generic_types.insert("Self".into(), vec![]);
            let mut methods = HashMap::new();
            for method in &declaration.methods {
                let signature = Signature {
                    generics: method.generics.iter().map(|p| p.name.clone()).collect(),
                    constraints: method
                        .generics
                        .iter()
                        .map(|p| {
                            (
                                p.name.clone(),
                                p.constraints.iter().map(|c| c.name.clone()).collect(),
                            )
                        })
                        .collect(),
                    parameters: method
                        .parameters
                        .iter()
                        .map(|p| self.resolve_type(&p.ty))
                        .collect::<Result<_, _>>()?,
                    result: method
                        .return_type
                        .as_ref()
                        .map(|ty| self.resolve_type(ty))
                        .transpose()?
                        .unwrap_or(Type::Unit),
                };
                if methods.insert(method.name.clone(), signature).is_some() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("duplicate trait method `{}`", method.name),
                        method.name_span,
                    ));
                }
            }
            self.traits.insert(
                declaration.name.clone(),
                TraitInfo {
                    generics: declaration
                        .generics
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                    methods,
                    associated_types: declaration
                        .associated_types
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect(),
                },
            );
        }

        for implementation in &program.implementations {
            self.set_generics(&implementation.generics);
            let target = self.resolve_type(&implementation.target)?;
            let trait_name = implementation.trait_name.as_ref().ok_or_else(|| Diagnostic::new(DiagnosticKind::Type, "inherent implementations are not available until constructor-method semantics are finalized", implementation.span))?.name.clone();
            let trait_arguments = implementation
                .trait_name
                .as_ref()
                .unwrap()
                .arguments
                .iter()
                .map(|argument| self.resolve_type(argument))
                .collect::<Result<Vec<_>, _>>()?;
            let trait_info = self.traits.get(&trait_name).cloned().ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("unknown trait `{trait_name}`"),
                    implementation.trait_name.as_ref().unwrap().span,
                )
            })?;
            if self.implementations.iter().any(|existing| {
                existing.trait_name == trait_name && types_overlap(&existing.target, &target)
            }) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "conflicting implementation of `{trait_name}` for {}",
                        self.format_type(&target)
                    ),
                    implementation.span,
                ));
            }
            let associated: HashSet<_> = implementation
                .associated_types
                .iter()
                .map(|(name, _, _)| name.clone())
                .collect();
            if associated != trait_info.associated_types {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "implementation of `{trait_name}` must define exactly its associated types"
                    ),
                    implementation.span,
                ));
            }
            let methods: HashMap<_, _> = implementation
                .methods
                .iter()
                .map(|method| (method.name.clone(), method.clone()))
                .collect();
            if methods.len() != implementation.methods.len()
                || methods.keys().cloned().collect::<HashSet<_>>()
                    != trait_info.methods.keys().cloned().collect()
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "implementation of `{trait_name}` must define exactly its declared methods"
                    ),
                    implementation.span,
                ));
            }
            self.implementations.push(ImplInfo {
                trait_name,
                trait_arguments,
                target,
            });
        }

        for function in &program.functions {
            self.set_generics(&function.generics);
            let parameters = function
                .parameters
                .iter()
                .map(|parameter| self.resolve_type(&parameter.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let result = function
                .return_type
                .as_ref()
                .map(|ty| self.resolve_type(ty))
                .transpose()?
                .unwrap_or(Type::Unit);
            self.functions.insert(
                function.name.clone(),
                Signature {
                    generics: function.generics.iter().map(|p| p.name.clone()).collect(),
                    constraints: function
                        .generics
                        .iter()
                        .map(|p| {
                            (
                                p.name.clone(),
                                p.constraints.iter().map(|c| c.name.clone()).collect(),
                            )
                        })
                        .collect(),
                    parameters,
                    result,
                },
            );
        }

        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Type,
                    "missing `main` function",
                    Span::point(1, 1),
                )
            })?;
        if !main.parameters.is_empty() || main.return_type.is_some() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "`main` must have signature `fn main()` in the current runtime profile",
                main.name_span,
            ));
        }

        for implementation in program.implementations.clone() {
            self.set_generics(&implementation.generics);
            let target = self.resolve_type(&implementation.target)?;
            let trait_name = implementation.trait_name.as_ref().unwrap().name.clone();
            let trait_info = self.traits[&trait_name].clone();
            for method in &implementation.methods {
                let expected = trait_info.methods[&method.name].clone();
                let actual = Signature {
                    generics: method.generics.iter().map(|p| p.name.clone()).collect(),
                    constraints: method
                        .generics
                        .iter()
                        .map(|p| {
                            (
                                p.name.clone(),
                                p.constraints.iter().map(|c| c.name.clone()).collect(),
                            )
                        })
                        .collect(),
                    parameters: method
                        .parameters
                        .iter()
                        .map(|p| self.resolve_type_with_self(&p.ty, &target))
                        .collect::<Result<_, _>>()?,
                    result: method
                        .return_type
                        .as_ref()
                        .map(|ty| self.resolve_type_with_self(ty, &target))
                        .transpose()?
                        .unwrap_or(Type::Unit),
                };
                let mut substitutions = HashMap::new();
                substitutions.insert("Self".into(), target.clone());
                for (name, argument) in trait_info
                    .generics
                    .iter()
                    .zip(implementation.trait_name.as_ref().unwrap().arguments.iter())
                {
                    substitutions.insert(name.clone(), self.resolve_type(argument)?);
                }
                if expected
                    .parameters
                    .iter()
                    .map(|ty| substitute(ty, &substitutions))
                    .collect::<Vec<_>>()
                    != actual.parameters
                    || substitute(&expected.result, &substitutions) != actual.result
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "method `{}` does not match trait `{trait_name}`",
                            method.name
                        ),
                        method.span,
                    ));
                }
                self.check_callable(method, actual)?;
            }
        }

        for function in &program.functions {
            self.check_function(function)?;
        }
        Ok(())
    }

    fn set_generics(&mut self, parameters: &[crate::ast::GenericParameter]) {
        self.generic_types = parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.clone(),
                    parameter
                        .constraints
                        .iter()
                        .map(|constraint| constraint.name.clone())
                        .collect(),
                )
            })
            .collect();
    }

    fn check_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        self.set_generics(&function.generics);
        let signature = self
            .functions
            .get(&function.name)
            .cloned()
            .expect("function signatures are collected before body checking");
        self.check_callable(function, signature)
    }

    fn check_callable(
        &mut self,
        function: &Function,
        signature: Signature,
    ) -> Result<(), Diagnostic> {
        self.expected_return = signature.result.clone();
        self.scopes.clear();
        self.begin_scope();
        for (parameter, ty) in function.parameters.iter().zip(signature.parameters) {
            self.scopes.last_mut().unwrap().insert(
                parameter.name.clone(),
                Variable {
                    ty,
                    constant: false,
                },
            );
        }
        let always_returns = self.check_block_contents(&function.body)?;
        if self.expected_return != Type::Unit && !always_returns {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "function `{}` may finish without returning {:?}",
                    function.name, self.expected_return
                ),
                function.span,
            )
            .with_help("return a value on every control-flow path"));
        }
        self.end_scope();
        Ok(())
    }

    fn resolve_type_with_self(&self, ty: &TypeName, self_type: &Type) -> Result<Type, Diagnostic> {
        if ty.name == "Self" && ty.arguments.is_empty() {
            return Ok(match ty.qualifier {
                TypeQualifier::Owned => self_type.clone(),
                TypeQualifier::SharedReference => {
                    Type::Reference(Box::new(self_type.clone()), false)
                }
                TypeQualifier::MutableReference => {
                    Type::Reference(Box::new(self_type.clone()), true)
                }
                TypeQualifier::RawConstPointer => {
                    Type::RawPointer(Box::new(self_type.clone()), false)
                }
                TypeQualifier::RawMutPointer => Type::RawPointer(Box::new(self_type.clone()), true),
            });
        }
        self.resolve_type(ty)
    }

    fn check_block(&mut self, block: &Block) -> Result<bool, Diagnostic> {
        self.begin_scope();
        let result = self.check_block_contents(block);
        self.end_scope();
        result
    }

    fn check_block_contents(&mut self, block: &Block) -> Result<bool, Diagnostic> {
        let mut always_returns = false;
        for statement in &block.statements {
            if !always_returns {
                always_returns = self.check_statement(&statement.node, statement.span)?;
            }
        }
        Ok(always_returns)
    }

    fn check_statement(&mut self, statement: &Statement, span: Span) -> Result<bool, Diagnostic> {
        match statement {
            Statement::Binding {
                kind,
                name,
                annotation,
                value,
                ..
            } => {
                if value.is_none() && *kind == BindingKind::Const {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("constant `{name}` requires an initializer"),
                        span,
                    ));
                }
                let inferred = value
                    .as_ref()
                    .map(|value| self.check_expression(value))
                    .transpose()?;
                let ty = if let Some(annotation) = annotation {
                    let declared = self.resolve_type(annotation)?;
                    if let (Some(inferred), Some(value)) = (&inferred, value) {
                        self.require_same(&declared, inferred, value.span, "binding initializer")?;
                    }
                    declared
                } else {
                    let inferred =
                        inferred.expect("parser requires an initializer without an annotation");
                    if contains_infer(&inferred) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("cannot infer the complete type of binding `{name}`"),
                            value.as_ref().unwrap().span,
                        )
                        .with_help("add an explicit Option<T> or Result<T, E> annotation"));
                    }
                    let materialized = materialize_literal(inferred);
                    if matches!(
                        materialized,
                        Type::IntLiteral(_) | Type::NegativeIntLiteral(_)
                    ) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "integer literal does not fit the default `int` type",
                            value.as_ref().unwrap().span,
                        )
                        .with_help("add an explicit i128 or u128 annotation"));
                    }
                    materialized
                };
                if ty == Type::Str {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "a `str` view must be held through `&str`",
                        value.as_ref().map_or(span, |value| value.span),
                    )
                    .with_help("borrow the view, for example `let view: &str = &text[0..end]`"));
                }
                if *kind == BindingKind::Const
                    && !self.is_constant_expression(value.as_ref().unwrap())
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("constant `{name}` requires a compile-time expression"),
                        value.as_ref().unwrap().span,
                    ));
                }
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty,
                        constant: *kind == BindingKind::Const,
                    },
                );
                Ok(false)
            }
            Statement::Assignment {
                name,
                operator,
                value,
                name_span,
            } => {
                let Some(target) = self.lookup_variable(name) else {
                    if *operator != AssignmentOperator::Assign {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("unknown variable `{name}`"),
                            *name_span,
                        ));
                    }
                    let inferred = self.check_expression(value)?;
                    if contains_infer(&inferred) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("cannot infer the complete type of binding `{name}`"),
                            value.span,
                        ));
                    }
                    let ty = materialize_literal(inferred);
                    if ty == Type::Str {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "a `str` view must be held through `&str`",
                            value.span,
                        ));
                    }
                    self.scopes.last_mut().unwrap().insert(
                        name.clone(),
                        Variable {
                            ty,
                            constant: false,
                        },
                    );
                    return Ok(false);
                };
                let value_type = self.check_expression(value)?;
                self.require_same(&target.ty, &value_type, value.span, "assignment")?;
                if *operator != AssignmentOperator::Assign && !is_numeric(&target.ty) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "compound arithmetic assignment requires a number, found {:?}",
                            target.ty
                        ),
                        span,
                    ));
                }
                Ok(false)
            }
            Statement::PlaceAssignment {
                target,
                operator,
                value,
            } => {
                let target_type = self.check_expression(target)?;
                let value_type = self.check_expression(value)?;
                if *operator == AssignmentOperator::Assign {
                    self.require_same(&target_type, &value_type, value.span, "assignment")?;
                } else {
                    self.check_binary(
                        match operator {
                            AssignmentOperator::Add => BinaryOperator::Add,
                            AssignmentOperator::Subtract => BinaryOperator::Subtract,
                            AssignmentOperator::Multiply => BinaryOperator::Multiply,
                            AssignmentOperator::Divide => BinaryOperator::Divide,
                            AssignmentOperator::Assign => unreachable!(),
                        },
                        target_type,
                        value_type,
                        span,
                    )?;
                }
                Ok(false)
            }
            Statement::Expression(expression) => {
                self.check_expression(expression)?;
                Ok(false)
            }
            Statement::Return(value) => {
                let actual = value
                    .as_ref()
                    .map(|expression| self.check_expression(expression))
                    .transpose()?
                    .unwrap_or(Type::Unit);
                self.require_same(&self.expected_return, &actual, span, "return value")?;
                Ok(true)
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_type = self.check_expression(condition)?;
                self.require_same(&Type::Bool, &condition_type, condition.span, "if condition")?;
                let then_returns = self.check_block(then_branch)?;
                let else_returns = else_branch
                    .as_ref()
                    .map(|branch| self.check_block(branch))
                    .transpose()?
                    .unwrap_or(false);
                Ok(then_returns && else_returns)
            }
            Statement::While { condition, body } => {
                let condition_type = self.check_expression(condition)?;
                self.require_same(
                    &Type::Bool,
                    &condition_type,
                    condition.span,
                    "while condition",
                )?;
                self.check_block(body)?;
                Ok(false)
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                let start_type = self.check_expression(start)?;
                let end_type = self.check_expression(end)?;
                self.require_same(&Type::Int, &start_type, start.span, "range start")?;
                self.require_same(&Type::Int, &end_type, end.span, "range end")?;
                self.begin_scope();
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty: Type::Int,
                        constant: false,
                    },
                );
                let result = self.check_block_contents(body);
                self.end_scope();
                result?;
                Ok(false)
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                let iterable_ty = self.check_expression(iterable)?;
                let element = match iterable_ty {
                    Type::Array(element, _)
                    | Type::Slice(element)
                    | Type::List(element)
                    | Type::Set(element) => *element,
                    Type::Reference(inner, _) => match *inner {
                        Type::Array(element, _)
                        | Type::Slice(element)
                        | Type::List(element)
                        | Type::Set(element) => *element,
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`for item in value` requires an array, slice, or List",
                                iterable.span,
                            ));
                        }
                    },
                    _ => {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`for item in value` requires an array, slice, or List",
                            iterable.span,
                        ));
                    }
                };
                let item = if self.type_is_copy(&element) {
                    element
                } else {
                    Type::Reference(Box::new(element), false)
                };
                self.begin_scope();
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty: item,
                        constant: false,
                    },
                );
                let result = self.check_block_contents(body);
                self.end_scope();
                result?;
                Ok(false)
            }
            Statement::Loop(body) => {
                self.check_block(body)?;
                Ok(!contains_break_for_current_loop(body))
            }
            Statement::Unsafe(body) => {
                self.unsafe_depth += 1;
                let result = self.check_block(body);
                self.unsafe_depth -= 1;
                result
            }
            Statement::Break | Statement::Continue => Ok(false),
        }
    }

    fn check_expression(&mut self, expression: &Expr) -> Result<Type, Diagnostic> {
        match &expression.node {
            Expression::Integer(value) => Ok(Type::IntLiteral(*value)),
            Expression::Float(_) => Ok(Type::FloatLiteral),
            Expression::String(_) => Ok(Type::String),
            Expression::Array(values) => {
                let Some(first) = values.first() else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "cannot infer the element type of an empty array",
                        expression.span,
                    ));
                };
                let element = match self.check_expression(first)? {
                    Type::IntLiteral(value) if value <= i64::MAX as u128 => Type::Int,
                    Type::FloatLiteral => Type::Float,
                    other => other,
                };
                for value in &values[1..] {
                    let actual = self.check_expression(value)?;
                    self.require_same(&element, &actual, value.span, "array element")?;
                }
                Ok(Type::Array(Box::new(element), values.len()))
            }
            Expression::Index { object, index } => {
                let object_ty = self.check_expression(object)?;
                let index_ty = self.check_expression(index)?;
                if !matches!(
                    index_ty,
                    Type::Int
                        | Type::UInt
                        | Type::Signed(_)
                        | Type::Unsigned(_)
                        | Type::IntLiteral(_)
                ) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "array index must be an integer",
                        index.span,
                    ));
                }
                match object_ty {
                    Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                        Ok(*element)
                    }
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("cannot index {}", self.format_type(&other)),
                        object.span,
                    )),
                }
            }
            Expression::Subslice { object, start, end } => {
                let object_ty = self.check_expression(object)?;
                for bound in [start, end] {
                    let ty = self.check_expression(bound)?;
                    if !matches!(
                        ty,
                        Type::Int
                            | Type::UInt
                            | Type::Signed(_)
                            | Type::Unsigned(_)
                            | Type::IntLiteral(_)
                    ) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "subslice bound must be an integer",
                            bound.span,
                        ));
                    }
                }
                match object_ty {
                    Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                        Ok(Type::Slice(element))
                    }
                    Type::String | Type::Str => Ok(Type::Str),
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("cannot subslice {}", self.format_type(&other)),
                        object.span,
                    )),
                }
            }
            Expression::Character(_) => Ok(Type::Char),
            Expression::Bool(_) => Ok(Type::Bool),
            Expression::StructConstruct { name, fields, .. } => {
                let Some(Type::Struct(id, _)) = self.types.get(name).cloned() else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("`{name}` is not a struct type"),
                        expression.span,
                    ));
                };
                let info = self.structs[&id].clone();
                let mut provided = HashMap::new();
                let mut substitutions = HashMap::new();
                for field in fields {
                    if provided
                        .insert(field.name.clone(), field.name_span)
                        .is_some()
                    {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("field `{}` is provided more than once", field.name),
                            field.name_span,
                        ));
                    }
                    let expected = info.fields.get(&field.name).ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("struct `{}` has no field `{}`", info.name, field.name),
                            field.name_span,
                        )
                    })?;
                    let actual = self.check_expression(&field.value)?;
                    infer_substitutions(expected, &actual, &mut substitutions, field.value.span)?;
                    self.require_same(
                        &substitute(expected, &substitutions),
                        &actual,
                        field.value.span,
                        "struct field",
                    )?;
                }
                let mut missing_fields = info
                    .fields
                    .keys()
                    .filter(|field| !provided.contains_key(*field))
                    .cloned()
                    .collect::<Vec<_>>();
                missing_fields.sort();
                if let Some(missing) = missing_fields.first() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "missing field `{missing}` when constructing `{}`",
                            info.name
                        ),
                        expression.span,
                    ));
                }
                let arguments = info
                    .generics
                    .iter()
                    .map(|name| {
                        substitutions.get(name).cloned().ok_or_else(|| {
                            Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "cannot infer generic argument `{name}` for `{}`",
                                    info.name
                                ),
                                expression.span,
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.require_constraints(&info.constraints, &arguments, expression.span)?;
                Ok(Type::Struct(id, arguments))
            }
            Expression::Identifier(name) => {
                if let Some(variable) = self.lookup_variable(name) {
                    return Ok(variable.ty);
                }
                if let Some(signature) = self.functions.get(name) {
                    return Ok(Type::Function(
                        signature.parameters.clone(),
                        Box::new(signature.result.clone()),
                    ));
                }
                if name == "print" {
                    return Ok(Type::Function(vec![], Box::new(Type::Unit)));
                }
                match name.as_str() {
                    "None" => return Ok(Type::Option(Box::new(Type::Infer))),
                    "Some" => {
                        return Ok(Type::Function(
                            vec![Type::Infer],
                            Box::new(Type::Option(Box::new(Type::Infer))),
                        ));
                    }
                    "Ok" => {
                        return Ok(Type::Function(
                            vec![Type::Infer],
                            Box::new(Type::Result(Box::new(Type::Infer), Box::new(Type::Infer))),
                        ));
                    }
                    "Err" => {
                        return Ok(Type::Function(
                            vec![Type::Infer],
                            Box::new(Type::Result(Box::new(Type::Infer), Box::new(Type::Infer))),
                        ));
                    }
                    _ => {}
                }
                if let Some(candidates) = self.variants.get(name) {
                    if candidates.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("ambiguous enum variant `{name}`"),
                            expression.span,
                        ));
                    }
                    return Ok(self.variant_constructor_type(&candidates[0]));
                }
                Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("unknown name `{name}`"),
                    expression.span,
                ))
            }
            Expression::FieldAccess {
                object,
                field,
                field_span,
            } => {
                if let Expression::Identifier(type_name) = &object.node
                    && let Some(Type::Enum(id, _)) = self.types.get(type_name)
                {
                    let info = &self.enums[id];
                    let variant = info.variants.get(field).ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("enum `{}` has no variant `{field}`", info.name),
                            *field_span,
                        )
                    })?;
                    return Ok(self.variant_constructor_type(variant));
                }
                let object_type = match self.check_expression(object)? {
                    Type::Reference(inner, _) => *inner,
                    other => other,
                };
                let Type::Struct(id, arguments) = object_type else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("field access requires a struct, found {object_type:?}"),
                        object.span,
                    ));
                };
                let substitutions = self.structs[&id]
                    .generics
                    .iter()
                    .cloned()
                    .zip(arguments)
                    .collect();
                self.structs[&id]
                    .fields
                    .get(field)
                    .map(|ty| substitute(ty, &substitutions))
                    .ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("struct `{}` has no field `{field}`", self.structs[&id].name),
                            *field_span,
                        )
                    })
            }
            Expression::Unary { operator, operand } => {
                let ty = self.check_expression(operand)?;
                match operator {
                    UnaryOperator::Negate if is_numeric(&ty) => Ok(match ty {
                        Type::IntLiteral(value) if value <= i128::MAX as u128 => {
                            Type::NegativeIntLiteral(-(value as i128))
                        }
                        Type::IntLiteral(value) if value == (1_u128 << 127) => {
                            Type::NegativeIntLiteral(i128::MIN)
                        }
                        Type::IntLiteral(_) => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "negative integer literal is outside i128 range",
                                expression.span,
                            ));
                        }
                        other => other,
                    }),
                    UnaryOperator::Not if ty == Type::Bool => Ok(Type::Bool),
                    UnaryOperator::Negate => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("unary `-` requires a number, found {ty:?}"),
                        expression.span,
                    )),
                    UnaryOperator::Not => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("unary `!` requires Bool, found {ty:?}"),
                        expression.span,
                    )),
                }
            }
            Expression::Move(operand) => self.check_expression(operand),
            Expression::Borrow { mutable, target } => {
                let target = self.check_expression(target)?;
                if *mutable && target == Type::Str {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`str` is an immutable UTF-8 view",
                        expression.span,
                    ));
                }
                Ok(Type::Reference(Box::new(target), *mutable))
            }
            Expression::Dereference(target) => match self.check_expression(target)? {
                Type::Reference(inner, _) => Ok(*inner),
                Type::RawPointer(inner, _) if self.unsafe_depth > 0 => Ok(*inner),
                Type::RawPointer(_, _) => Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "raw pointer dereference requires an `unsafe` block",
                    expression.span,
                )),
                other => Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("cannot dereference {}", self.format_type(&other)),
                    expression.span,
                )),
            },
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left_type = self.check_expression(left)?;
                let right_type = self.check_expression(right)?;
                self.check_binary(*operator, left_type, right_type, expression.span)
            }
            Expression::Call { callee, arguments } => {
                if matches!(&callee.node, Expression::Identifier(name) if name == "String") {
                    if !arguments.is_empty() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`String` expects no arguments",
                            expression.span,
                        ));
                    }
                    return Ok(Type::String);
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Path") {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`Path` expects one String or str",
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !matches!(source, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "Path source must be String or str",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Path);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(owner) = &object.node
                    && matches!(
                        owner.as_str(),
                        "File" | "Directory" | "Time" | "Duration" | "Path"
                    )
                {
                    let io_error = Type::IoError;
                    let result_unit =
                        || Type::Result(Box::new(Type::Unit), Box::new(io_error.clone()));
                    match (owner.as_str(), field.as_str()) {
                        ("Path", "new") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(actual, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Path source must be String or str",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Path);
                        }
                        ("File", "read_text") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(Box::new(Type::String), Box::new(io_error)));
                        }
                        ("File", "read_bytes") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(io_error),
                            ));
                        }
                        ("File", "write_text" | "append_text") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            let text = self.check_expression(&arguments[1])?;
                            if !matches!(text, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "file text must be String or str",
                                    arguments[1].span,
                                ));
                            }
                            return Ok(result_unit());
                        }
                        ("File", "write_bytes" | "append_bytes") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            let bytes = self.check_expression(&arguments[1])?;
                            if !matches!(bytes,Type::List(ref element)|Type::Slice(ref element) if matches!(**element,Type::Unsigned(8)))
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "file bytes must be List<u8> or a u8 slice",
                                    arguments[1].span,
                                ));
                            }
                            return Ok(result_unit());
                        }
                        ("File", "copy" | "move") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            self.require_path(&arguments[1])?;
                            return Ok(result_unit());
                        }
                        ("File", "remove") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(result_unit());
                        }
                        ("File", "exists") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Bool);
                        }
                        ("Directory", "create" | "create_all" | "remove")
                            if arguments.len() == 1 =>
                        {
                            self.require_path(&arguments[0])?;
                            return Ok(result_unit());
                        }
                        ("Directory", "exists") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Bool);
                        }
                        ("Directory", "read") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Path))),
                                Box::new(io_error),
                            ));
                        }
                        ("File", "size" | "modified_seconds") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(Box::new(Type::UInt), Box::new(io_error)));
                        }
                        ("Time", "now") if arguments.is_empty() => return Ok(Type::Instant),
                        ("Time", "unix_seconds") if arguments.is_empty() => return Ok(Type::UInt),
                        ("Time", "sleep") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            self.require_same(
                                &Type::Duration,
                                &actual,
                                arguments[0].span,
                                "sleep duration",
                            )?;
                            return Ok(Type::Unit);
                        }
                        ("Duration", "from_nanos" | "from_millis" | "from_seconds")
                            if arguments.len() == 1 =>
                        {
                            let actual = self.check_expression(&arguments[0])?;
                            if !is_integer(&actual) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "duration value must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Duration);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no system operation `{owner}.{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "String")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => return Ok(Type::String),
                        "with_capacity" if arguments.len() == 1 => {
                            self.check_expression(&arguments[0])?;
                            return Ok(Type::String);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no String constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "List")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => {
                            return Ok(Type::List(Box::new(Type::Infer)));
                        }
                        "of" if !arguments.is_empty() => {
                            let first = self.check_expression(&arguments[0])?;
                            let element = materialize_literal(first);
                            for argument in &arguments[1..] {
                                let actual = self.check_expression(argument)?;
                                self.require_same(
                                    &element,
                                    &actual,
                                    argument.span,
                                    "List element",
                                )?;
                            }
                            return Ok(Type::List(Box::new(element)));
                        }
                        "with_capacity" if arguments.len() == 1 => {
                            let capacity = self.check_expression(&arguments[0])?;
                            if !is_integer(&capacity) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "List capacity must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::List(Box::new(Type::Infer)));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no List constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Map")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => {
                            return Ok(Type::Map(Box::new(Type::Infer), Box::new(Type::Infer)));
                        }
                        "with_capacity" if arguments.len() == 1 => {
                            let capacity = self.check_expression(&arguments[0])?;
                            if !is_integer(&capacity) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Map capacity must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Map(Box::new(Type::Infer), Box::new(Type::Infer)));
                        }
                        "of" if !arguments.is_empty() && arguments.len() % 2 == 0 => {
                            let key = materialize_literal(self.check_expression(&arguments[0])?);
                            if !is_collection_key(&key) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Map keys must be integers, bool, char, String, or str",
                                    arguments[0].span,
                                ));
                            }
                            let value = materialize_literal(self.check_expression(&arguments[1])?);
                            for pair in arguments[2..].chunks_exact(2) {
                                let actual_key = self.check_expression(&pair[0])?;
                                let actual_value = self.check_expression(&pair[1])?;
                                self.require_same(&key, &actual_key, pair[0].span, "Map key")?;
                                self.require_same(
                                    &value,
                                    &actual_value,
                                    pair[1].span,
                                    "Map value",
                                )?;
                            }
                            return Ok(Type::Map(Box::new(key), Box::new(value)));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no Map constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Set")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => {
                            return Ok(Type::Set(Box::new(Type::Infer)));
                        }
                        "with_capacity" if arguments.len() == 1 => {
                            let capacity = self.check_expression(&arguments[0])?;
                            if !is_integer(&capacity) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Set capacity must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Set(Box::new(Type::Infer)));
                        }
                        "of" if !arguments.is_empty() => {
                            let element =
                                materialize_literal(self.check_expression(&arguments[0])?);
                            if !is_collection_key(&element) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Set elements must be integers, bool, char, String, or str",
                                    arguments[0].span,
                                ));
                            }
                            for argument in &arguments[1..] {
                                let actual = self.check_expression(argument)?;
                                self.require_same(&element, &actual, argument.span, "Set element")?;
                            }
                            return Ok(Type::Set(Box::new(element)));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no Set constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::Identifier(name) = &callee.node
                    && let Some(target) = numeric_type(name)
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "numeric conversion `{name}` expects 1 argument, found {}",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !is_numeric(&source) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("cannot convert {} to {name}", self.format_type(&source)),
                            arguments[0].span,
                        ));
                    }
                    return Ok(target);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(name) = &object.node
                    && let Some(target) = numeric_type(name)
                    && field == "try_from"
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{name}.try_from` expects 1 argument"),
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !is_numeric(&source) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "checked conversion requires a numeric value",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(target),
                        Box::new(Type::ConversionError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(
                        field.as_str(),
                        "wrapping_add"
                            | "wrapping_sub"
                            | "wrapping_mul"
                            | "saturating_add"
                            | "saturating_sub"
                            | "saturating_mul"
                    )
                {
                    let receiver = self.check_expression(object)?;
                    if !is_integer(&receiver) || matches!(receiver, Type::IntLiteral(_)) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{field}` requires a concretely typed integer"),
                            object.span,
                        ));
                    }
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{field}` expects 1 argument"),
                            expression.span,
                        ));
                    }
                    let argument = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &receiver,
                        &argument,
                        arguments[0].span,
                        "integer method argument",
                    )?;
                    return Ok(receiver);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && !matches!(&object.node, Expression::Identifier(name) if matches!(self.types.get(name), Some(Type::Enum(_, _))))
                {
                    let receiver = self.check_expression(object)?;
                    if matches!(
                        receiver,
                        Type::Array(_, _)
                            | Type::Slice(_)
                            | Type::List(_)
                            | Type::Map(_, _)
                            | Type::Set(_)
                    ) && matches!(field.as_str(), "len" | "is_empty" | "count" | "empty")
                    {
                        if !arguments.is_empty() {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`len` expects no arguments",
                                expression.span,
                            ));
                        }
                        return Ok(if matches!(field.as_str(), "is_empty" | "empty") {
                            Type::Bool
                        } else {
                            Type::UInt
                        });
                    }
                    if let Type::Array(element, _) | Type::Slice(element) = &receiver
                        && field == "iter"
                        && arguments.is_empty()
                    {
                        return Ok(Type::Slice(element.clone()));
                    }
                    if let Type::List(element) = &receiver {
                        let field = match field.as_str() {
                            "add" => "push",
                            "count" => "len",
                            "empty" => "is_empty",
                            other => other,
                        };
                        match field {
                            "iter" if arguments.is_empty() => {
                                return Ok(Type::Slice(element.clone()));
                            }
                            "capacity" if arguments.is_empty() => return Ok(Type::UInt),
                            "push" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[0].span,
                                    "List element",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "pop" if arguments.is_empty() => {
                                return Ok(Type::Option(element.clone()));
                            }
                            "get" | "get_mut" if arguments.len() == 1 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "List access index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Option(Box::new(Type::Reference(
                                    element.clone(),
                                    field == "get_mut",
                                ))));
                            }
                            "insert" if arguments.len() == 2 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "List insertion index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let actual = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[1].span,
                                    "List element",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "remove" if arguments.len() == 1 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "List removal index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok((**element).clone());
                            }
                            "clear" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if let Type::Map(key, value) = &receiver {
                        match field.as_str() {
                            "keys" if arguments.is_empty() => return Ok(Type::Slice(key.clone())),
                            "values" if arguments.is_empty() => {
                                return Ok(Type::Slice(value.clone()));
                            }
                            "capacity" if arguments.is_empty() => return Ok(Type::UInt),
                            "has" | "contains_key" | "get" | "get_mut" | "remove"
                                if arguments.len() == 1 =>
                            {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(key, &actual, arguments[0].span, "Map key")?;
                                return Ok(match field.as_str() {
                                    "has" | "contains_key" => Type::Bool,
                                    "get" => Type::Option(Box::new(Type::Reference(
                                        value.clone(),
                                        false,
                                    ))),
                                    "get_mut" => {
                                        Type::Option(Box::new(Type::Reference(value.clone(), true)))
                                    }
                                    _ => Type::Option(value.clone()),
                                });
                            }
                            "set" | "insert" if arguments.len() == 2 => {
                                let actual_key = self.check_expression(&arguments[0])?;
                                let actual_value = self.check_expression(&arguments[1])?;
                                self.require_same(key, &actual_key, arguments[0].span, "Map key")?;
                                self.require_same(
                                    value,
                                    &actual_value,
                                    arguments[1].span,
                                    "Map value",
                                )?;
                                return Ok(Type::Option(value.clone()));
                            }
                            "clear" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if let Type::Set(element) = &receiver {
                        match field.as_str() {
                            "iter" if arguments.is_empty() => {
                                return Ok(Type::Slice(element.clone()));
                            }
                            "capacity" if arguments.is_empty() => return Ok(Type::UInt),
                            "has" | "contains" | "remove" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[0].span,
                                    "Set element",
                                )?;
                                return Ok(Type::Bool);
                            }
                            "add" | "insert" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[0].span,
                                    "Set element",
                                )?;
                                return Ok(Type::Bool);
                            }
                            "clear" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Path) {
                        match field.as_str() {
                            "join" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                if !matches!(actual, Type::String | Type::Str | Type::Path) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Path.join expects Path, String, or str",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Path);
                            }
                            "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_empty" | "is_absolute" if arguments.is_empty() => {
                                return Ok(Type::Bool);
                            }
                            "as_string" if arguments.is_empty() => return Ok(Type::String),
                            "name" | "extension" if arguments.is_empty() => {
                                return Ok(Type::Option(Box::new(Type::String)));
                            }
                            "parent" if arguments.is_empty() => {
                                return Ok(Type::Option(Box::new(Type::Path)));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Instant)
                        && field == "elapsed"
                        && arguments.is_empty()
                    {
                        return Ok(Type::Duration);
                    }
                    if matches!(receiver, Type::Duration)
                        && arguments.is_empty()
                        && matches!(field.as_str(), "nanos" | "millis" | "seconds")
                    {
                        return Ok(Type::UInt);
                    }
                    if matches!(receiver, Type::String | Type::Str)
                        && matches!(field.as_str(), "len" | "capacity" | "is_empty")
                    {
                        if !arguments.is_empty() {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`{field}` expects no arguments"),
                                expression.span,
                            ));
                        }
                        if matches!(receiver, Type::Str) && field == "capacity" {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "borrowed `str` has no capacity",
                                expression.span,
                            ));
                        }
                        return Ok(if field == "is_empty" {
                            Type::Bool
                        } else {
                            Type::UInt
                        });
                    }
                    if matches!(receiver, Type::String | Type::Str)
                        && matches!(field.as_str(), "contains" | "starts_with" | "ends_with")
                    {
                        if arguments.len() != 1 {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`{field}` expects 1 argument"),
                                expression.span,
                            ));
                        }
                        let actual = self.check_expression(&arguments[0])?;
                        if !matches!(actual, Type::String | Type::Str) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "string query argument must be `String` or `str`",
                                arguments[0].span,
                            ));
                        }
                        return Ok(Type::Bool);
                    }
                    if matches!(receiver, Type::String)
                        && matches!(
                            field.as_str(),
                            "push" | "push_str" | "append" | "add" | "clear"
                        )
                    {
                        let expected = if field == "clear" { 0 } else { 1 };
                        if arguments.len() != expected {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`{field}` expects {expected} arguments"),
                                expression.span,
                            ));
                        }
                        if let Some(argument) = arguments.first() {
                            let actual = self.check_expression(argument)?;
                            let wanted = if field == "push" {
                                Type::Char
                            } else {
                                Type::String
                            };
                            self.require_same(
                                &wanted,
                                &actual,
                                argument.span,
                                "String method argument",
                            )?;
                        }
                        return Ok(Type::Unit);
                    }
                    let candidates = self
                        .implementations
                        .iter()
                        .filter_map(|implementation| {
                            let trait_info = &self.traits[&implementation.trait_name];
                            let method = trait_info.methods.get(field)?;
                            let substitutions = trait_info
                                .generics
                                .iter()
                                .cloned()
                                .zip(implementation.trait_arguments.iter().cloned())
                                .collect();
                            let method = Signature {
                                generics: method.generics.clone(),
                                constraints: method.constraints.clone(),
                                parameters: method
                                    .parameters
                                    .iter()
                                    .map(|ty| substitute(ty, &substitutions))
                                    .collect(),
                                result: substitute(&method.result, &substitutions),
                            };
                            implementation_matches(&implementation.target, &receiver)
                                .then_some((implementation.target.clone(), method))
                        })
                        .chain(match &receiver {
                            Type::Generic(name) => self
                                .generic_types
                                .get(name)
                                .into_iter()
                                .flatten()
                                .filter_map(|trait_name| {
                                    self.traits
                                        .get(trait_name)?
                                        .methods
                                        .get(field)
                                        .map(|method| (receiver.clone(), method.clone()))
                                })
                                .collect::<Vec<_>>()
                                .into_iter(),
                            _ => Vec::new().into_iter(),
                        })
                        .collect::<Vec<_>>();
                    if candidates.len() > 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "ambiguous method `{field}` for {}",
                                self.format_type(&receiver)
                            ),
                            callee.span,
                        ));
                    }
                    if let Some((_target, method)) = candidates.first() {
                        let mut substitutions = HashMap::new();
                        substitutions.insert("Self".into(), receiver.clone());
                        let parameters = method
                            .parameters
                            .iter()
                            .map(|ty| substitute(ty, &substitutions))
                            .collect::<Vec<_>>();
                        if parameters.len() != arguments.len() + 1 {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "method `{field}` expects {} arguments, found {}",
                                    parameters.len().saturating_sub(1),
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                        let expected_receiver = match &parameters[0] {
                            Type::Reference(inner, _) => &**inner,
                            other => other,
                        };
                        self.require_same(
                            expected_receiver,
                            &receiver,
                            object.span,
                            "method receiver",
                        )?;
                        for (expected, argument) in parameters[1..].iter().zip(arguments) {
                            let actual = self.check_expression(argument)?;
                            self.require_same(expected, &actual, argument.span, "method argument")?;
                        }
                        return Ok(substitute(&method.result, &substitutions));
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("no method `{field}` for {}", self.format_type(&receiver)),
                        callee.span,
                    ));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "print") {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`print` expects 1 argument, found {}", arguments.len()),
                            expression.span,
                        ));
                    }
                    self.check_expression(&arguments[0])?;
                    return Ok(Type::Unit);
                }
                if let Expression::Identifier(name) = &callee.node
                    && matches!(name.as_str(), "Some" | "Ok" | "Err")
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{name}` expects 1 payload, found {}", arguments.len()),
                            expression.span,
                        ));
                    }
                    let payload = self.check_expression(&arguments[0])?;
                    return Ok(match name.as_str() {
                        "Some" => Type::Option(Box::new(payload)),
                        "Ok" => Type::Result(Box::new(payload), Box::new(Type::Infer)),
                        "Err" => Type::Result(Box::new(Type::Infer), Box::new(payload)),
                        _ => unreachable!(),
                    });
                }
                if let Expression::Identifier(name) = &callee.node
                    && let Some(signature) = self.functions.get(name).cloned()
                    && !signature.generics.is_empty()
                {
                    if signature.parameters.len() != arguments.len() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "function `{name}` expects {} arguments, found {}",
                                signature.parameters.len(),
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let mut substitutions = HashMap::new();
                    for (parameter, argument) in signature.parameters.iter().zip(arguments) {
                        let actual = self.check_expression(argument)?;
                        let inference_actual =
                            implicit_shared_borrow_target(parameter, &actual).unwrap_or(&actual);
                        if implicit_shared_borrow_target(parameter, &actual).is_some()
                            && !is_storage_expression(argument)
                        {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "implicit borrowing requires a named storage value",
                                argument.span,
                            )
                            .with_help("bind the value first or write an explicit borrow"));
                        }
                        let inference_parameter = match parameter {
                            Type::Reference(inner, false)
                                if !matches!(actual, Type::Reference(_, _)) =>
                            {
                                &**inner
                            }
                            _ => parameter,
                        };
                        infer_substitutions(
                            inference_parameter,
                            inference_actual,
                            &mut substitutions,
                            argument.span,
                        )?;
                        let expected = substitute(parameter, &substitutions);
                        let comparison =
                            implicit_shared_borrow_target(&expected, &actual).unwrap_or(&actual);
                        let expected = match &expected {
                            Type::Reference(inner, false)
                                if !matches!(actual, Type::Reference(_, _)) =>
                            {
                                &**inner
                            }
                            _ => &expected,
                        };
                        self.require_same(
                            expected,
                            comparison,
                            argument.span,
                            "function argument",
                        )?;
                    }
                    for generic in &signature.generics {
                        if !substitutions.contains_key(generic) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("cannot infer generic argument `{generic}` for `{name}`"),
                                expression.span,
                            ));
                        }
                        let concrete = &substitutions[generic];
                        if matches!(concrete, Type::IntLiteral(_) | Type::NegativeIntLiteral(_)) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "integer literal does not fit the default `int` type",
                                expression.span,
                            )
                            .with_help(
                                "bind it with an explicit i128 or u128 annotation before this call",
                            ));
                        }
                        for constraint in &signature.constraints[generic] {
                            if !(constraint == "Copy" && self.type_is_copy(concrete))
                                && !self.implementations.iter().any(|implementation| {
                                    implementation.trait_name == *constraint
                                        && implementation_matches(&implementation.target, concrete)
                                })
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    format!(
                                        "type {} does not satisfy constraint `{constraint}` for `{generic}`",
                                        self.format_type(concrete)
                                    ),
                                    expression.span,
                                ));
                            }
                        }
                    }
                    let result = substitute(&signature.result, &substitutions);
                    self.validate_instantiated_type(&result, expression.span)?;
                    return Ok(result);
                }
                let callee_type = self.check_expression(callee)?;
                let Type::Function(parameters, result) = callee_type else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "expression is not callable",
                        callee.span,
                    ));
                };
                if parameters.len() != arguments.len() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "function expects {} arguments, found {}",
                            parameters.len(),
                            arguments.len()
                        ),
                        expression.span,
                    ));
                }
                let mut substitutions = HashMap::new();
                for (parameter, argument) in parameters.iter().zip(arguments) {
                    let argument_type = self.check_expression(argument)?;
                    let inference_parameter = match parameter {
                        Type::Reference(inner, false)
                            if !matches!(argument_type, Type::Reference(_, _)) =>
                        {
                            &**inner
                        }
                        _ => parameter,
                    };
                    let inference_actual = implicit_shared_borrow_target(parameter, &argument_type)
                        .unwrap_or(&argument_type);
                    if implicit_shared_borrow_target(parameter, &argument_type).is_some()
                        && !is_storage_expression(argument)
                    {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "implicit borrowing requires a named storage value",
                            argument.span,
                        ));
                    }
                    infer_substitutions(
                        inference_parameter,
                        inference_actual,
                        &mut substitutions,
                        argument.span,
                    )?;
                    let expected = substitute(parameter, &substitutions);
                    let comparison = implicit_shared_borrow_target(&expected, &argument_type)
                        .unwrap_or(&argument_type);
                    let expected = match &expected {
                        Type::Reference(inner, false)
                            if !matches!(argument_type, Type::Reference(_, _)) =>
                        {
                            &**inner
                        }
                        _ => &expected,
                    };
                    self.require_same(expected, comparison, argument.span, "function argument")?;
                }
                let result = substitute(&result, &substitutions);
                self.validate_instantiated_type(&result, expression.span)?;
                Ok(result)
            }
            Expression::Match { value, arms } => {
                let matched_type = self.check_expression(value)?;
                let mut result_type = None;
                let mut coverage = MatchCoverage::new(&matched_type, self)?;
                for arm in arms {
                    self.begin_scope();
                    let pattern_result =
                        self.check_pattern(&arm.pattern.node, arm.pattern.span, &matched_type);
                    let arm_result = pattern_result.and_then(|key| {
                        coverage.add(key, arm.pattern.span)?;
                        self.check_expression(&arm.value)
                    });
                    self.end_scope();
                    let arm_type = arm_result?;
                    if let Some(expected) = &result_type {
                        self.require_same(expected, &arm_type, arm.value.span, "match arm")?;
                        result_type = Some(merge_types(expected, &arm_type));
                    } else {
                        result_type = Some(arm_type);
                    }
                }
                coverage.finish(expression.span)?;
                Ok(result_type.unwrap_or(Type::Unit))
            }
            Expression::Try(operand) => {
                let operand_type = self.check_expression(operand)?;
                match operand_type {
                    Type::Option(value) => {
                        if !matches!(self.expected_return, Type::Option(_)) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`?` on Option requires the enclosing function to return Option",
                                expression.span,
                            ));
                        }
                        Ok(*value)
                    }
                    Type::Result(value, error) => {
                        let Type::Result(_, expected_error) = &self.expected_return else {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`?` on Result requires the enclosing function to return Result",
                                expression.span,
                            ));
                        };
                        self.require_same(
                            expected_error,
                            &error,
                            expression.span,
                            "propagated error",
                        )?;
                        Ok(*value)
                    }
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("`?` requires Option or Result, found {other:?}"),
                        expression.span,
                    )),
                }
            }
        }
    }

    fn check_binary(
        &self,
        operator: BinaryOperator,
        mut left: Type,
        mut right: Type,
        span: Span,
    ) -> Result<Type, Diagnostic> {
        match (&left, &right) {
            (
                Type::IntLiteral(_) | Type::NegativeIntLiteral(_),
                Type::IntLiteral(_) | Type::NegativeIntLiteral(_),
            ) => {
                left = Type::Int;
                right = Type::Int;
            }
            (Type::FloatLiteral, Type::FloatLiteral) => {
                left = Type::Float;
                right = Type::Float;
            }
            (Type::IntLiteral(_) | Type::NegativeIntLiteral(_), other)
                if is_integer(other) && types_compatible(other, &left) =>
            {
                left = other.clone()
            }
            (other, Type::IntLiteral(_) | Type::NegativeIntLiteral(_))
                if is_integer(other) && types_compatible(other, &right) =>
            {
                right = other.clone()
            }
            (Type::FloatLiteral, other) if matches!(other, Type::Float | Type::Float32) => {
                left = other.clone()
            }
            (other, Type::FloatLiteral) if matches!(other, Type::Float | Type::Float32) => {
                right = other.clone()
            }
            _ => {}
        }
        if !types_compatible(&left, &right) && types_compatible(&right, &left) {
            left = right.clone();
        }
        self.require_same(&left, &right, span, "binary operands")?;
        match operator {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder
                if is_numeric(&left) =>
            {
                Ok(left)
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => Ok(Type::Bool),
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
                if is_numeric(&left) =>
            {
                Ok(Type::Bool)
            }
            BinaryOperator::And | BinaryOperator::Or if left == Type::Bool => Ok(Type::Bool),
            _ => Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("operator {operator:?} cannot be used with {left:?}"),
                span,
            )),
        }
    }

    fn variant_constructor_type(&self, variant: &VariantInfo) -> Type {
        let result = Type::Enum(
            variant.owner,
            self.enums[&variant.owner]
                .generics
                .iter()
                .cloned()
                .map(Type::Generic)
                .collect(),
        );
        if variant.payload.is_empty() {
            result
        } else {
            Type::Function(variant.payload.clone(), Box::new(result))
        }
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        span: Span,
        expected: &Type,
    ) -> Result<PatternKey, Diagnostic> {
        match pattern {
            Pattern::Wildcard => Ok(PatternKey::CatchAll),
            Pattern::Binding(name) => {
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty: expected.clone(),
                        constant: false,
                    },
                );
                Ok(PatternKey::CatchAll)
            }
            Pattern::Integer(value) => {
                self.require_same(expected, &Type::IntLiteral(*value), span, "integer pattern")?;
                Ok(PatternKey::Other)
            }
            Pattern::String(_) => {
                self.require_same(&Type::String, expected, span, "string pattern")?;
                Ok(PatternKey::Other)
            }
            Pattern::Character(_) => {
                self.require_same(&Type::Char, expected, span, "character pattern")?;
                Ok(PatternKey::Other)
            }
            Pattern::Bool(value) => {
                self.require_same(&Type::Bool, expected, span, "boolean pattern")?;
                Ok(PatternKey::Case(value.to_string(), true))
            }
            Pattern::Variant {
                type_name,
                variant,
                arguments,
            } => {
                let payload = match expected {
                    Type::Option(value) => {
                        if type_name.as_deref().is_some_and(|name| name != "Option") {
                            return Err(self.wrong_variant_owner(
                                type_name.as_deref().unwrap(),
                                expected,
                                span,
                            ));
                        }
                        match variant.as_str() {
                            "Some" => vec![(**value).clone()],
                            "None" => vec![],
                            _ => return Err(self.unknown_variant(variant, expected, span)),
                        }
                    }
                    Type::Result(ok, error) => {
                        if type_name.as_deref().is_some_and(|name| name != "Result") {
                            return Err(self.wrong_variant_owner(
                                type_name.as_deref().unwrap(),
                                expected,
                                span,
                            ));
                        }
                        match variant.as_str() {
                            "Ok" => vec![(**ok).clone()],
                            "Err" => vec![(**error).clone()],
                            _ => return Err(self.unknown_variant(variant, expected, span)),
                        }
                    }
                    Type::Enum(id, arguments) => {
                        let info = &self.enums[id];
                        if type_name.as_deref().is_some_and(|name| name != info.name) {
                            return Err(self.wrong_variant_owner(
                                type_name.as_deref().unwrap(),
                                expected,
                                span,
                            ));
                        }
                        let payload = info
                            .variants
                            .get(variant)
                            .map(|info| info.payload.clone())
                            .ok_or_else(|| self.unknown_variant(variant, expected, span))?;
                        let substitutions = info
                            .generics
                            .iter()
                            .cloned()
                            .zip(arguments.iter().cloned())
                            .collect();
                        payload
                            .iter()
                            .map(|ty| substitute(ty, &substitutions))
                            .collect()
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("variant pattern cannot match {expected:?}"),
                            span,
                        ));
                    }
                };
                if payload.len() != arguments.len() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "variant `{variant}` expects {} payload patterns, found {}",
                            payload.len(),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                for (argument, ty) in arguments.iter().zip(payload) {
                    self.check_pattern(&argument.node, argument.span, &ty)?;
                }
                Ok(PatternKey::Case(
                    variant.clone(),
                    arguments
                        .iter()
                        .all(|argument| pattern_is_irrefutable(&argument.node)),
                ))
            }
        }
    }

    fn wrong_variant_owner(&self, owner: &str, expected: &Type, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("variant from `{owner}` cannot match {expected:?}"),
            span,
        )
    }

    fn unknown_variant(&self, variant: &str, expected: &Type, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("`{variant}` is not a variant of {expected:?}"),
            span,
        )
    }

    fn resolve_type(&self, ty: &TypeName) -> Result<Type, Diagnostic> {
        if ty.qualifier != TypeQualifier::Owned {
            let inner = match ty.qualifier {
                TypeQualifier::SharedReference | TypeQualifier::MutableReference => {
                    let mut owned = ty.clone();
                    owned.qualifier = TypeQualifier::Owned;
                    self.resolve_type(&owned)?
                }
                TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer => {
                    if ty.name != "ptr" || ty.arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "raw pointer type must be `ptr<T>` or `mut ptr<T>`",
                            ty.span,
                        ));
                    }
                    self.resolve_type(&ty.arguments[0])?
                }
                TypeQualifier::Owned => unreachable!(),
            };
            return Ok(match ty.qualifier {
                TypeQualifier::SharedReference => Type::Reference(Box::new(inner), false),
                TypeQualifier::MutableReference => Type::Reference(Box::new(inner), true),
                TypeQualifier::RawConstPointer => Type::RawPointer(Box::new(inner), false),
                TypeQualifier::RawMutPointer => Type::RawPointer(Box::new(inner), true),
                TypeQualifier::Owned => unreachable!(),
            });
        }
        let resolved = match ty.name.as_str() {
            "int" if ty.arguments.is_empty() => Type::Int,
            "uint" if ty.arguments.is_empty() => Type::UInt,
            "f64" if ty.arguments.is_empty() => Type::Float,
            "f32" if ty.arguments.is_empty() => Type::Float32,
            "i8" if ty.arguments.is_empty() => Type::Signed(8),
            "i16" if ty.arguments.is_empty() => Type::Signed(16),
            "i32" if ty.arguments.is_empty() => Type::Signed(32),
            "i64" if ty.arguments.is_empty() => Type::Signed(64),
            "i128" if ty.arguments.is_empty() => Type::Signed(128),
            "u8" if ty.arguments.is_empty() => Type::Unsigned(8),
            "u16" if ty.arguments.is_empty() => Type::Unsigned(16),
            "u32" if ty.arguments.is_empty() => Type::Unsigned(32),
            "u64" if ty.arguments.is_empty() => Type::Unsigned(64),
            "u128" if ty.arguments.is_empty() => Type::Unsigned(128),
            "String" if ty.arguments.is_empty() => Type::String,
            "str" if ty.arguments.is_empty() => Type::Str,
            "Path" if ty.arguments.is_empty() => Type::Path,
            "Instant" if ty.arguments.is_empty() => Type::Instant,
            "Duration" if ty.arguments.is_empty() => Type::Duration,
            "IoError" if ty.arguments.is_empty() => Type::IoError,
            "[]" if ty.arguments.len() == 1 => {
                Type::Slice(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "List" if ty.arguments.len() == 1 => {
                Type::List(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Map" if ty.arguments.len() == 2 => Type::Map(
                Box::new(self.resolve_type(&ty.arguments[0])?),
                Box::new(self.resolve_type(&ty.arguments[1])?),
            ),
            "Set" if ty.arguments.len() == 1 => {
                Type::Set(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            name if name.starts_with("[;") && name.ends_with(']') && ty.arguments.len() == 1 => {
                let length = name[2..name.len() - 1].parse::<usize>().map_err(|_| {
                    Diagnostic::new(DiagnosticKind::Type, "invalid array length", ty.span)
                })?;
                Type::Array(Box::new(self.resolve_type(&ty.arguments[0])?), length)
            }
            "char" if ty.arguments.is_empty() => Type::Char,
            "bool" if ty.arguments.is_empty() => Type::Bool,
            "ConversionError" if ty.arguments.is_empty() => Type::ConversionError,
            "Unit" if ty.arguments.is_empty() => Type::Unit,
            "Option" if ty.arguments.len() == 1 => {
                Type::Option(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Result" if ty.arguments.len() == 2 => Type::Result(
                Box::new(self.resolve_type(&ty.arguments[0])?),
                Box::new(self.resolve_type(&ty.arguments[1])?),
            ),
            name if ty.arguments.is_empty() && self.generic_types.contains_key(name) => {
                Type::Generic(name.into())
            }
            name if self.types.contains_key(name) => {
                let arguments = ty
                    .arguments
                    .iter()
                    .map(|argument| self.resolve_type(argument))
                    .collect::<Result<Vec<_>, _>>()?;
                match self.types[name] {
                    Type::Struct(id, _) => {
                        let expected = self.structs[&id].generics.len();
                        if arguments.len() != expected {
                            return Err(self.generic_arity(
                                name,
                                expected,
                                arguments.len(),
                                ty.span,
                            ));
                        }
                        self.require_constraints(
                            &self.structs[&id].constraints,
                            &arguments,
                            ty.span,
                        )?;
                        Type::Struct(id, arguments)
                    }
                    Type::Enum(id, _) => {
                        let expected = self.enums[&id].generics.len();
                        if arguments.len() != expected {
                            return Err(self.generic_arity(
                                name,
                                expected,
                                arguments.len(),
                                ty.span,
                            ));
                        }
                        self.require_constraints(
                            &self.enums[&id].constraints,
                            &arguments,
                            ty.span,
                        )?;
                        Type::Enum(id, arguments)
                    }
                    _ => unreachable!(),
                }
            }
            name => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("unknown type `{name}`"),
                    ty.span,
                ));
            }
        };
        Ok(resolved)
    }

    fn require_constraints(
        &self,
        constraints: &[Vec<String>],
        arguments: &[Type],
        span: Span,
    ) -> Result<(), Diagnostic> {
        for (requirements, argument) in constraints.iter().zip(arguments) {
            for requirement in requirements {
                let satisfied = match argument {
                    Type::Generic(name) => self
                        .generic_types
                        .get(name)
                        .is_some_and(|available| available.contains(requirement)),
                    concrete if requirement == "Copy" && self.type_is_copy(concrete) => true,
                    concrete => self.implementations.iter().any(|implementation| {
                        implementation.trait_name == *requirement
                            && implementation_matches(&implementation.target, concrete)
                    }),
                };
                if !satisfied {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "type {} does not satisfy constraint `{requirement}`",
                            self.format_type(argument)
                        ),
                        span,
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_instantiated_type(&self, ty: &Type, span: Span) -> Result<(), Diagnostic> {
        match ty {
            Type::Struct(id, arguments) => {
                self.require_constraints(&self.structs[id].constraints, arguments, span)
            }
            Type::Enum(id, arguments) => {
                self.require_constraints(&self.enums[id].constraints, arguments, span)
            }
            Type::Option(value) => self.validate_instantiated_type(value, span),
            Type::Result(ok, error) => {
                self.validate_instantiated_type(ok, span)?;
                self.validate_instantiated_type(error, span)
            }
            _ => Ok(()),
        }
    }

    fn type_is_copy(&self, ty: &Type) -> bool {
        match ty {
            Type::Int
            | Type::IntLiteral(_)
            | Type::NegativeIntLiteral(_)
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
            | Type::Float
            | Type::Float32
            | Type::FloatLiteral
            | Type::Char
            | Type::Bool
            | Type::Instant
            | Type::Duration
            | Type::Unit
            | Type::Reference(_, false)
            | Type::RawPointer(_, _) => true,
            Type::Option(value) => self.type_is_copy(value),
            Type::Result(ok, error) => self.type_is_copy(ok) && self.type_is_copy(error),
            Type::Struct(id, _) => self.implementations.iter().any(|implementation| {
                implementation.trait_name == "Copy"
                    && matches!(implementation.target, Type::Struct(other, _) if other == *id)
            }),
            Type::Enum(id, _) => self.implementations.iter().any(|implementation| {
                implementation.trait_name == "Copy"
                    && matches!(implementation.target, Type::Enum(other, _) if other == *id)
            }),
            _ => false,
        }
    }

    fn generic_arity(&self, name: &str, expected: usize, actual: usize, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("type `{name}` expects {expected} type arguments, found {actual}"),
            span,
        )
    }

    fn require_same(
        &self,
        expected: &Type,
        actual: &Type,
        span: Span,
        context: &str,
    ) -> Result<(), Diagnostic> {
        if types_compatible(expected, actual) {
            return Ok(());
        }
        Err(Diagnostic::new(
            DiagnosticKind::Type,
            format!(
                "{context} expected {}, found {}",
                self.format_type(expected),
                self.format_type(actual)
            ),
            span,
        ))
    }

    fn require_path(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let actual = self.check_expression(expression)?;
        if matches!(actual, Type::Path) {
            Ok(())
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "filesystem path must be Path, found {}",
                    self.format_type(&actual)
                ),
                expression.span,
            ))
        }
    }

    fn format_type(&self, ty: &Type) -> String {
        match ty {
            Type::Int => "Int".into(),
            Type::IntLiteral(_) => "Int".into(),
            Type::NegativeIntLiteral(_) => "Int".into(),
            Type::UInt => "uint".into(),
            Type::Signed(width) => format!("i{width}"),
            Type::Unsigned(width) => format!("u{width}"),
            Type::Float => "Float".into(),
            Type::Float32 => "f32".into(),
            Type::FloatLiteral => "floating-point literal".into(),
            Type::String => "String".into(),
            Type::Path => "Path".into(),
            Type::Instant => "Instant".into(),
            Type::Duration => "Duration".into(),
            Type::Str => "str".into(),
            Type::Array(element, length) => format!("[{}; {length}]", self.format_type(element)),
            Type::Slice(element) => format!("[{}]", self.format_type(element)),
            Type::List(element) => format!("List<{}>", self.format_type(element)),
            Type::Map(key, value) => format!(
                "Map<{}, {}>",
                self.format_type(key),
                self.format_type(value)
            ),
            Type::Set(element) => format!("Set<{}>", self.format_type(element)),
            Type::Char => "Char".into(),
            Type::Bool => "Bool".into(),
            Type::ConversionError => "ConversionError".into(),
            Type::IoError => "IoError".into(),
            Type::Unit => "Unit".into(),
            Type::Struct(id, arguments) => self.format_nominal(&self.structs[id].name, arguments),
            Type::Enum(id, arguments) => self.format_nominal(&self.enums[id].name, arguments),
            Type::Generic(name) => name.clone(),
            Type::Reference(inner, mutable) => format!(
                "&{}{}",
                if *mutable { "mut " } else { "" },
                self.format_type(inner)
            ),
            Type::RawPointer(inner, mutable) => format!(
                "{}ptr<{}>",
                if *mutable { "mut " } else { "" },
                self.format_type(inner)
            ),
            Type::Option(value) => format!("Option<{}>", self.format_type(value)),
            Type::Result(ok, error) => format!(
                "Result<{}, {}>",
                self.format_type(ok),
                self.format_type(error)
            ),
            Type::Function(parameters, result) => format!(
                "fn({}) -> {}",
                parameters
                    .iter()
                    .map(|parameter| self.format_type(parameter))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.format_type(result)
            ),
            Type::Infer => "_".into(),
        }
    }

    fn format_nominal(&self, name: &str, arguments: &[Type]) -> String {
        if arguments.is_empty() {
            name.into()
        } else {
            format!(
                "{name}<{}>",
                arguments
                    .iter()
                    .map(|argument| self.format_type(argument))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn is_constant_expression(&self, expression: &Expr) -> bool {
        match &expression.node {
            Expression::Array(values) => values
                .iter()
                .all(|value| self.is_constant_expression(value)),
            Expression::Index { .. } => false,
            Expression::Subslice { .. } => false,
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => true,
            Expression::Identifier(name) => self
                .lookup_variable(name)
                .is_some_and(|variable| variable.constant),
            Expression::Unary { operand, .. } => self.is_constant_expression(operand),
            Expression::Binary { left, right, .. } => {
                self.is_constant_expression(left) && self.is_constant_expression(right)
            }
            Expression::StructConstruct { fields, .. } => fields
                .iter()
                .all(|field| self.is_constant_expression(&field.value)),
            Expression::FieldAccess { object, .. } => self.is_constant_expression(object),
            Expression::Match { value, arms } => {
                self.is_constant_expression(value)
                    && arms
                        .iter()
                        .all(|arm| self.is_constant_expression(&arm.value))
            }
            Expression::Try(_)
            | Expression::Call { .. }
            | Expression::Move(_)
            | Expression::Borrow { .. }
            | Expression::Dereference(_) => false,
        }
    }

    fn lookup_variable(&self, name: &str) -> Option<Variable> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn begin_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PatternKey {
    CatchAll,
    Case(String, bool),
    Other,
}

struct MatchCoverage {
    required: HashSet<String>,
    seen: HashSet<String>,
    catch_all: bool,
    requires_catch_all: bool,
}

impl MatchCoverage {
    fn new(expected: &Type, checker: &TypeChecker) -> Result<Self, Diagnostic> {
        let required = match expected {
            Type::Bool => ["true".into(), "false".into()].into_iter().collect(),
            Type::Option(_) => ["Some".into(), "None".into()].into_iter().collect(),
            Type::Result(_, _) => ["Ok".into(), "Err".into()].into_iter().collect(),
            Type::Enum(id, _) => checker.enums[id].variants.keys().cloned().collect(),
            _ => HashSet::new(),
        };
        let requires_catch_all = required.is_empty();
        Ok(Self {
            required,
            seen: HashSet::new(),
            catch_all: false,
            requires_catch_all,
        })
    }

    fn add(&mut self, key: PatternKey, span: Span) -> Result<(), Diagnostic> {
        if self.catch_all {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "unreachable match arm after a catch-all pattern",
                span,
            ));
        }
        match key {
            PatternKey::CatchAll => self.catch_all = true,
            PatternKey::Case(case, complete) => {
                if self.seen.contains(&case) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("unreachable match case `{case}` after it was fully covered"),
                        span,
                    ));
                }
                if complete {
                    self.seen.insert(case);
                }
            }
            PatternKey::Other => {}
        }
        Ok(())
    }

    fn finish(&self, span: Span) -> Result<(), Diagnostic> {
        if self.catch_all {
            return Ok(());
        }
        let mut missing = self
            .required
            .difference(&self.seen)
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        if !missing.is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("non-exhaustive match; missing {}", missing.join(", ")),
                span,
            )
            .with_help("add the missing variants or a `_` catch-all arm"));
        }
        if self.requires_catch_all {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "non-exhaustive match over an open value domain",
                span,
            )
            .with_help("add a `_` catch-all arm"));
        }
        Ok(())
    }
}

fn types_compatible(expected: &Type, actual: &Type) -> bool {
    match (expected, actual) {
        (Type::Infer, _) | (_, Type::Infer) => true,
        (Type::Int, Type::IntLiteral(value)) => *value <= i64::MAX as u128,
        (Type::Int, Type::NegativeIntLiteral(value)) => *value >= i64::MIN as i128,
        (Type::UInt, Type::IntLiteral(value)) => *value <= u64::MAX as u128,
        (Type::Signed(width), Type::IntLiteral(value)) => {
            *value <= i128::MAX as u128 && signed_fits(*value as i128, *width)
        }
        (Type::Signed(width), Type::NegativeIntLiteral(value)) => signed_fits(*value, *width),
        (Type::Unsigned(width), Type::IntLiteral(value)) => unsigned_fits(*value, *width),
        (Type::Float | Type::Float32, Type::FloatLiteral) => true,
        (Type::Signed(expected), Type::Signed(actual)) => expected >= actual,
        (Type::Unsigned(expected), Type::Unsigned(actual)) => expected >= actual,
        (Type::Int, Type::Signed(actual)) => *actual <= 64,
        (Type::Signed(expected), Type::Int) => *expected >= 128,
        (Type::UInt, Type::Unsigned(actual)) => *actual <= 64,
        (Type::Unsigned(expected), Type::UInt) => *expected >= 128,
        (Type::Signed(expected), Type::Unsigned(actual)) => *expected > *actual,
        (Type::Float, Type::Float32) => true,
        (Type::Generic(left), Type::Generic(right)) => left == right,
        (Type::Reference(left, lm), Type::Reference(right, rm)) => {
            (!*lm || *rm) && types_compatible(left, right)
        }
        (Type::RawPointer(left, lm), Type::RawPointer(right, rm)) => {
            lm == rm && types_compatible(left, right)
        }
        (Type::Str, Type::String) => true,
        (Type::Struct(left, a), Type::Struct(right, b))
        | (Type::Enum(left, a), Type::Enum(right, b)) => {
            left == right
                && a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| types_compatible(x, y))
        }
        (Type::Option(left), Type::Option(right)) => types_compatible(left, right),
        (Type::Array(left, left_len), Type::Array(right, right_len)) => {
            left_len == right_len && types_compatible(left, right)
        }
        (Type::Slice(left), Type::Slice(right)) => types_compatible(left, right),
        (Type::List(left), Type::List(right)) => types_compatible(left, right),
        (Type::Map(lk, lv), Type::Map(rk, rv)) => {
            types_compatible(lk, rk) && types_compatible(lv, rv)
        }
        (Type::Set(left), Type::Set(right)) => types_compatible(left, right),
        (Type::Result(left_ok, left_error), Type::Result(right_ok, right_error)) => {
            types_compatible(left_ok, right_ok) && types_compatible(left_error, right_error)
        }
        (
            Type::Function(left_parameters, left_result),
            Type::Function(right_parameters, right_result),
        ) => {
            left_parameters.len() == right_parameters.len()
                && left_parameters
                    .iter()
                    .zip(right_parameters)
                    .all(|(left, right)| types_compatible(left, right))
                && types_compatible(left_result, right_result)
        }
        _ => expected == actual,
    }
}

fn signed_fits(value: i128, width: u16) -> bool {
    width == 128 || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1).contains(&value)
}

fn unsigned_fits(value: u128, width: u16) -> bool {
    width == 128 || value < (1_u128 << width)
}

fn materialize_literal(ty: Type) -> Type {
    match ty {
        Type::IntLiteral(value) if value <= i64::MAX as u128 => Type::Int,
        Type::NegativeIntLiteral(value) if value >= i64::MIN as i128 => Type::Int,
        Type::FloatLiteral => Type::Float,
        other => other,
    }
}

fn infer_substitutions(
    template: &Type,
    actual: &Type,
    substitutions: &mut HashMap<String, Type>,
    span: Span,
) -> Result<(), Diagnostic> {
    match (template, actual) {
        (Type::Generic(name), actual) => {
            if let Some(previous) = substitutions.get(name) {
                if !types_compatible(previous, actual) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("conflicting inferred types for `{name}`"),
                        span,
                    ));
                }
            } else {
                substitutions.insert(name.clone(), materialize_literal(actual.clone()));
            }
        }
        (Type::Struct(a, xs), Type::Struct(b, ys)) | (Type::Enum(a, xs), Type::Enum(b, ys))
            if a == b && xs.len() == ys.len() =>
        {
            for (x, y) in xs.iter().zip(ys) {
                infer_substitutions(x, y, substitutions, span)?;
            }
        }
        (Type::Option(x), Type::Option(y)) => infer_substitutions(x, y, substitutions, span)?,
        (Type::Array(x, a), Type::Array(y, b)) if a == b => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::Slice(x), Type::Slice(y)) => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::List(x), Type::List(y)) => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            infer_substitutions(ak, bk, substitutions, span)?;
            infer_substitutions(av, bv, substitutions, span)?;
        }
        (Type::Set(x), Type::Set(y)) => infer_substitutions(x, y, substitutions, span)?,
        (Type::Result(a, b), Type::Result(x, y)) => {
            infer_substitutions(a, x, substitutions, span)?;
            infer_substitutions(b, y, substitutions, span)?;
        }
        (Type::Reference(x, xm), Type::Reference(y, ym)) if !*xm || *ym => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::RawPointer(x, xm), Type::RawPointer(y, ym)) if xm == ym => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        _ => {}
    }
    Ok(())
}

fn substitute(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Generic(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Type::Struct(id, args) => Type::Struct(
            *id,
            args.iter().map(|x| substitute(x, substitutions)).collect(),
        ),
        Type::Enum(id, args) => Type::Enum(
            *id,
            args.iter().map(|x| substitute(x, substitutions)).collect(),
        ),
        Type::Option(x) => Type::Option(Box::new(substitute(x, substitutions))),
        Type::Array(x, length) => Type::Array(Box::new(substitute(x, substitutions)), *length),
        Type::Slice(x) => Type::Slice(Box::new(substitute(x, substitutions))),
        Type::List(x) => Type::List(Box::new(substitute(x, substitutions))),
        Type::Map(key, value) => Type::Map(
            Box::new(substitute(key, substitutions)),
            Box::new(substitute(value, substitutions)),
        ),
        Type::Set(x) => Type::Set(Box::new(substitute(x, substitutions))),
        Type::Result(a, b) => Type::Result(
            Box::new(substitute(a, substitutions)),
            Box::new(substitute(b, substitutions)),
        ),
        Type::Function(args, result) => Type::Function(
            args.iter().map(|x| substitute(x, substitutions)).collect(),
            Box::new(substitute(result, substitutions)),
        ),
        Type::Reference(inner, mutable) => {
            Type::Reference(Box::new(substitute(inner, substitutions)), *mutable)
        }
        Type::RawPointer(inner, mutable) => {
            Type::RawPointer(Box::new(substitute(inner, substitutions)), *mutable)
        }
        _ => ty.clone(),
    }
}

fn implementation_matches(template: &Type, concrete: &Type) -> bool {
    let mut substitutions = HashMap::new();
    infer_substitutions(template, concrete, &mut substitutions, Span::point(1, 1)).is_ok()
        && types_compatible(&substitute(template, &substitutions), concrete)
}

fn types_overlap(left: &Type, right: &Type) -> bool {
    match (left, right) {
        (Type::Generic(_), _) | (_, Type::Generic(_)) => true,
        (Type::Struct(a, xs), Type::Struct(b, ys)) | (Type::Enum(a, xs), Type::Enum(b, ys)) => {
            a == b && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| types_overlap(x, y))
        }
        (Type::Option(x), Type::Option(y)) => types_overlap(x, y),
        (Type::Array(x, a), Type::Array(y, b)) => a == b && types_overlap(x, y),
        (Type::Slice(x), Type::Slice(y)) => types_overlap(x, y),
        (Type::List(x), Type::List(y)) => types_overlap(x, y),
        (Type::Map(ak, av), Type::Map(bk, bv)) => types_overlap(ak, bk) && types_overlap(av, bv),
        (Type::Set(x), Type::Set(y)) => types_overlap(x, y),
        (Type::Result(a, b), Type::Result(x, y)) => types_overlap(a, x) && types_overlap(b, y),
        _ => left == right,
    }
}

fn contains_infer(ty: &Type) -> bool {
    match ty {
        Type::Infer => true,
        Type::Option(value) => contains_infer(value),
        Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
            contains_infer(element)
        }
        Type::Map(key, value) => contains_infer(key) || contains_infer(value),
        Type::Set(element) => contains_infer(element),
        Type::Result(ok, error) => contains_infer(ok) || contains_infer(error),
        Type::Function(parameters, result) => {
            parameters.iter().any(contains_infer) || contains_infer(result)
        }
        Type::Reference(inner, _) | Type::RawPointer(inner, _) => contains_infer(inner),
        _ => false,
    }
}

fn merge_types(left: &Type, right: &Type) -> Type {
    match (left, right) {
        (Type::Infer, other) | (other, Type::Infer) => other.clone(),
        (Type::Option(left), Type::Option(right)) => {
            Type::Option(Box::new(merge_types(left, right)))
        }
        (Type::Array(left, length), Type::Array(right, other)) if length == other => {
            Type::Array(Box::new(merge_types(left, right)), *length)
        }
        (Type::List(left), Type::List(right)) => Type::List(Box::new(merge_types(left, right))),
        (Type::Map(lk, lv), Type::Map(rk, rv)) => {
            Type::Map(Box::new(merge_types(lk, rk)), Box::new(merge_types(lv, rv)))
        }
        (Type::Set(left), Type::Set(right)) => Type::Set(Box::new(merge_types(left, right))),
        (Type::Result(left_ok, left_error), Type::Result(right_ok, right_error)) => Type::Result(
            Box::new(merge_types(left_ok, right_ok)),
            Box::new(merge_types(left_error, right_error)),
        ),
        _ => left.clone(),
    }
}

fn pattern_is_irrefutable(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard | Pattern::Binding(_) => true,
        Pattern::Variant { .. } => false,
        Pattern::Integer(_) | Pattern::String(_) | Pattern::Character(_) | Pattern::Bool(_) => {
            false
        }
    }
}

fn is_numeric(ty: &Type) -> bool {
    is_integer(ty) || matches!(ty, Type::Float | Type::Float32 | Type::FloatLiteral)
}

fn is_integer(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int
            | Type::IntLiteral(_)
            | Type::NegativeIntLiteral(_)
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
    )
}

fn is_collection_key(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
            | Type::IntLiteral(_)
            | Type::NegativeIntLiteral(_)
            | Type::Bool
            | Type::Char
            | Type::String
            | Type::Str
    )
}

fn numeric_type(name: &str) -> Option<Type> {
    Some(match name {
        "int" => Type::Int,
        "uint" => Type::UInt,
        "i8" => Type::Signed(8),
        "i16" => Type::Signed(16),
        "i32" => Type::Signed(32),
        "i64" => Type::Signed(64),
        "i128" => Type::Signed(128),
        "u8" => Type::Unsigned(8),
        "u16" => Type::Unsigned(16),
        "u32" => Type::Unsigned(32),
        "u64" => Type::Unsigned(64),
        "u128" => Type::Unsigned(128),
        "f32" => Type::Float32,
        "f64" => Type::Float,
        _ => return None,
    })
}

fn contains_break_for_current_loop(block: &Block) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.node {
            Statement::Break => true,
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                contains_break_for_current_loop(then_branch)
                    || else_branch
                        .as_ref()
                        .is_some_and(contains_break_for_current_loop)
            }
            Statement::While { .. }
            | Statement::For { .. }
            | Statement::ForEach { .. }
            | Statement::Loop(_) => false,
            _ => false,
        })
}

fn implicit_shared_borrow_target<'a>(expected: &Type, actual: &'a Type) -> Option<&'a Type> {
    matches!(expected, Type::Reference(_, false))
        .then_some(actual)
        .filter(|_| !matches!(actual, Type::Reference(_, _)))
}

fn is_storage_expression(expression: &Expr) -> bool {
    matches!(
        expression.node,
        Expression::Identifier(_) | Expression::FieldAccess { .. } | Expression::Index { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer::Lexer, parser::Parser, resolver::Resolver};

    fn check(source: &str) -> Result<(), Diagnostic> {
        let program = Parser::new(Lexer::new(source).tokenize()?).parse()?;
        Resolver::new().resolve(&program)?;
        TypeChecker::new().check(&program)
    }

    #[test]
    fn checks_user_functions_and_returns() {
        assert!(
            check(
                "fn add(a: int, b: int) -> int { return a + b } fn main() { print(add(10, 20)) }"
            )
            .is_ok()
        );
        assert!(check("fn bad() -> bool { return 1 } fn main() {}").is_err());
        assert!(check("fn bad() -> int { if true { return 1 } } fn main() {}").is_err());
    }

    #[test]
    fn checks_annotations_assignments_and_conditions() {
        assert!(check("fn main() { var x: int = 1 x += 2 if x > 0 { print(x) } }").is_ok());
        assert!(check("fn main() { var x: bool = 1 }").is_err());
        assert!(check("fn main() { if 1 {} }").is_err());
    }

    #[test]
    fn constants_require_compile_time_expressions() {
        assert!(check("fn main() { const x = 1 + 2 const y = x * 3 print(y) }").is_ok());
        assert!(check("fn value() -> int { return 1 } fn main() { const x = value() }").is_err());
    }

    #[test]
    fn checks_unary_and_call_arity() {
        assert!(
            check("fn id(x: bool) -> bool { return !x } fn main() { print(id(false)) }").is_ok()
        );
        assert!(check("fn id(x: int) -> int { return x } fn main() { print(id()) }").is_err());
        assert!(check("fn main() { print(-true) }").is_err());
    }

    #[test]
    fn checks_exact_width_numeric_ranges() {
        assert!(
            check("fn value(x: i32) -> i32 { return x } fn main() { let x: i8 = 127 }").is_ok()
        );
        assert!(check("fn main() { let x: i8 = 128 }").is_err());
        assert!(check("fn main() { let x: u8 = -1 }").is_err());
    }
}
