use crate::ast::{self, BinaryOperator, BindingKind, TypeQualifier, UnaryOperator};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::collections::{HashMap, HashSet};

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub usize);
    };
}

id!(FunctionId);
id!(LocalId);
id!(TypeId);
id!(StructId);
id!(EnumId);
id!(VariantId);
id!(TraitId);
id!(ImplId);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Unit,
    Bool,
    Char,
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
    Int { signed: bool, width: Option<u16> },
    Float { width: u16 },
    Reference { mutable: bool, inner: Box<Type> },
    RawPointer { mutable: bool, inner: Box<Type> },
    Struct(StructId, Vec<Type>),
    Enum(EnumId, Vec<Type>),
    Option(Box<Type>),
    Result(Box<Type>, Box<Type>),
    Generic(String),
    Function(Vec<Type>, Box<Type>),
    Unknown,
}

impl Type {
    pub fn is_copy(&self, program: &Program) -> bool {
        match self {
            Self::Unit
            | Self::Bool
            | Self::Char
            | Self::Int { .. }
            | Self::Float { .. }
            | Self::Instant
            | Self::Duration
            | Self::Reference { .. }
            | Self::RawPointer { .. } => true,
            Self::Struct(id, _) => program.copy_types.contains(&TypeId(id.0)),
            Self::Enum(id, _) => program
                .copy_types
                .contains(&TypeId(program.structs.len() + id.0)),
            Self::Option(inner) => inner.is_copy(program),
            Self::Result(ok, error) => ok.is_copy(program) && error.is_copy(program),
            Self::Array(element, _) => element.is_copy(program),
            Self::Slice(_) | Self::Str => true,
            Self::String
            | Self::Path
            | Self::List(_)
            | Self::Map(_, _)
            | Self::Set(_)
            | Self::Generic(_)
            | Self::Function(_, _)
            | Self::Unknown => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Program {
    pub structs: Vec<Struct>,
    pub enums: Vec<Enum>,
    pub traits: Vec<Trait>,
    pub implementations: Vec<Implementation>,
    pub functions: Vec<Function>,
    pub copy_types: HashSet<TypeId>,
}

#[derive(Debug, Clone)]
pub struct Struct {
    pub id: StructId,
    pub type_id: TypeId,
    pub name: String,
    pub fields: Vec<Field>,
    pub span: Span,
    pub generic_parameters: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub index: usize,
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Enum {
    pub id: EnumId,
    pub type_id: TypeId,
    pub name: String,
    pub variants: Vec<Variant>,
    pub span: Span,
    pub generic_parameters: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub id: VariantId,
    pub index: usize,
    pub name: String,
    pub payload: Vec<Type>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Trait {
    pub id: TraitId,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Implementation {
    pub id: ImplId,
    pub trait_id: Option<TraitId>,
    pub target: Type,
    pub methods: Vec<FunctionId>,
    pub span: Span,
    pub generic_parameters: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub id: FunctionId,
    pub name: String,
    pub parameters: Vec<LocalId>,
    pub locals: Vec<Local>,
    pub return_type: Type,
    pub body: Block,
    pub generic_parameters: Vec<String>,
    pub owner_impl: Option<ImplId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub id: LocalId,
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    pub parameter: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StatementKind {
    Let {
        local: LocalId,
        value: Option<Expr>,
    },
    Assign {
        target: Place,
        operator: ast::AssignmentOperator,
        value: Expr,
    },
    Expression(Expr),
    Return(Option<Expr>),
    If {
        condition: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    While {
        condition: Expr,
        body: Block,
    },
    For {
        local: LocalId,
        start: Expr,
        end: Expr,
        inclusive: bool,
        body: Block,
    },
    ForEach {
        local: LocalId,
        iterable: Expr,
        body: Block,
    },
    Loop(Block),
    Unsafe(Block),
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Array(Vec<Expr>),
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Subslice {
        object: Box<Expr>,
        start: Box<Expr>,
        end: Box<Expr>,
    },
    Constant(Constant),
    Local(LocalId),
    Function(FunctionId),
    Struct {
        id: StructId,
        fields: Vec<(usize, Expr)>,
    },
    Variant {
        enum_id: EnumId,
        variant_id: VariantId,
    },
    EnumConstruct {
        enum_id: EnumId,
        variant_id: VariantId,
        payload: Vec<Expr>,
    },
    Field {
        object: Box<Expr>,
        index: usize,
    },
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Try(Box<Expr>),
    Move(Place),
    Borrow {
        mutable: bool,
        place: Place,
    },
    Dereference(Box<Expr>, bool),
    Unary {
        operator: UnaryOperator,
        operand: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        operator: BinaryOperator,
        right: Box<Expr>,
    },
    Call(Call),
}

#[derive(Debug, Clone)]
pub struct Call {
    pub target: CallTarget,
    pub arguments: Vec<Expr>,
    pub receiver: Option<ReceiverMode>,
    pub substitutions: Vec<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTarget {
    Function(FunctionId),
    TraitMethod { trait_id: TraitId, method: usize },
    Intrinsic(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverMode {
    Move,
    Shared,
    Mutable,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Binding(LocalId),
    Constant(Constant),
    Variant {
        enum_id: EnumId,
        variant_id: VariantId,
        arguments: Vec<Pattern>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Signed(i128, Option<u16>),
    Unsigned(u128, Option<u16>),
    Float(f64, u16),
    Bool(bool),
    Char(char),
    String(String),
    Unit,
}

#[derive(Debug, Clone)]
pub struct Place {
    pub local: LocalId,
    pub projections: Vec<Projection>,
}

#[derive(Debug, Clone)]
pub enum Projection {
    Field(usize),
    SafeDereference,
    RawDereference,
    VariantField(VariantId, usize),
    Index(Box<Expr>),
    Subslice { start: Box<Expr>, end: Box<Expr> },
}

pub fn lower(program: &ast::Program) -> Result<Program, Diagnostic> {
    Lowering::new(program).lower()
}

struct Lowering<'a> {
    ast: &'a ast::Program,
    struct_names: HashMap<String, StructId>,
    enum_names: HashMap<String, EnumId>,
    trait_names: HashMap<String, TraitId>,
    function_names: HashMap<String, FunctionId>,
    method_ids: HashMap<(usize, usize), FunctionId>,
    next_variant: usize,
}

impl<'a> Lowering<'a> {
    fn new(ast: &'a ast::Program) -> Self {
        let struct_names = ast
            .structs
            .iter()
            .enumerate()
            .map(|(i, x)| (x.name.clone(), StructId(i)))
            .collect();
        let enum_names = ast
            .enums
            .iter()
            .enumerate()
            .map(|(i, x)| (x.name.clone(), EnumId(i)))
            .collect();
        let trait_names = ast
            .traits
            .iter()
            .enumerate()
            .map(|(i, x)| (x.name.clone(), TraitId(i)))
            .collect();
        let mut function_names = HashMap::new();
        for (i, function) in ast.functions.iter().enumerate() {
            function_names.insert(function.name.clone(), FunctionId(i));
        }
        let mut next = ast.functions.len();
        let mut method_ids = HashMap::new();
        for (impl_index, implementation) in ast.implementations.iter().enumerate() {
            for method_index in 0..implementation.methods.len() {
                method_ids.insert((impl_index, method_index), FunctionId(next));
                next += 1;
            }
        }
        Self {
            ast,
            struct_names,
            enum_names,
            trait_names,
            function_names,
            method_ids,
            next_variant: 0,
        }
    }

    fn lower(mut self) -> Result<Program, Diagnostic> {
        let mut structs = Vec::new();
        for (index, declaration) in self.ast.structs.iter().enumerate() {
            let id = StructId(index);
            structs.push(Struct {
                id,
                type_id: TypeId(index),
                name: declaration.name.clone(),
                fields: declaration
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(field_index, field)| Field {
                        index: field_index,
                        name: field.name.clone(),
                        ty: self.lower_type(&field.ty),
                        span: field.name_span,
                    })
                    .collect(),
                span: declaration.span,
                generic_parameters: declaration
                    .generics
                    .iter()
                    .map(|generic| generic.name.clone())
                    .collect(),
            });
        }
        let mut enums = Vec::new();
        for (index, declaration) in self.ast.enums.iter().enumerate() {
            let mut variants = Vec::new();
            for (variant_index, variant) in declaration.variants.iter().enumerate() {
                let id = VariantId(self.next_variant);
                self.next_variant += 1;
                variants.push(Variant {
                    id,
                    index: variant_index,
                    name: variant.name.clone(),
                    payload: variant
                        .payload
                        .iter()
                        .map(|ty| self.lower_type(ty))
                        .collect(),
                    span: variant.name_span,
                });
            }
            enums.push(Enum {
                id: EnumId(index),
                type_id: TypeId(structs.len() + index),
                name: declaration.name.clone(),
                variants,
                span: declaration.span,
                generic_parameters: declaration
                    .generics
                    .iter()
                    .map(|generic| generic.name.clone())
                    .collect(),
            });
        }
        let traits = self
            .ast
            .traits
            .iter()
            .enumerate()
            .map(|(i, x)| Trait {
                id: TraitId(i),
                name: x.name.clone(),
                span: x.span,
            })
            .collect::<Vec<_>>();
        let mut implementations = Vec::new();
        for (index, implementation) in self.ast.implementations.iter().enumerate() {
            implementations.push(Implementation {
                id: ImplId(index),
                trait_id: implementation
                    .trait_name
                    .as_ref()
                    .and_then(|x| self.trait_names.get(&x.name).copied())
                    .or_else(|| {
                        (implementation
                            .trait_name
                            .as_ref()
                            .is_some_and(|x| x.name == "Copy"))
                        .then_some(TraitId(usize::MAX))
                    }),
                target: self.lower_type(&implementation.target),
                methods: implementation
                    .methods
                    .iter()
                    .enumerate()
                    .map(|(m, _)| self.method_ids[&(index, m)])
                    .collect(),
                span: implementation.span,
                generic_parameters: implementation
                    .generics
                    .iter()
                    .map(|generic| generic.name.clone())
                    .collect(),
            });
        }
        let mut functions = Vec::new();
        for (index, function) in self.ast.functions.iter().enumerate() {
            functions.push(self.lower_function(function, FunctionId(index), None)?);
        }
        for (impl_index, implementation) in self.ast.implementations.iter().enumerate() {
            for (method_index, method) in implementation.methods.iter().enumerate() {
                functions.push(self.lower_function(
                    method,
                    self.method_ids[&(impl_index, method_index)],
                    Some(ImplId(impl_index)),
                )?);
            }
        }
        let mut copy_types = HashSet::new();
        for implementation in &implementations {
            if implementation.trait_id == Some(TraitId(usize::MAX)) {
                match implementation.target {
                    Type::Struct(id, _) => {
                        copy_types.insert(TypeId(id.0));
                    }
                    Type::Enum(id, _) => {
                        copy_types.insert(TypeId(structs.len() + id.0));
                    }
                    _ => {}
                }
            }
        }
        let program = Program {
            structs,
            enums,
            traits,
            implementations,
            functions,
            copy_types,
        };
        validate(&program)?;
        Ok(program)
    }

    fn lower_function(
        &self,
        function: &ast::Function,
        id: FunctionId,
        owner_impl: Option<ImplId>,
    ) -> Result<Function, Diagnostic> {
        let self_ty =
            owner_impl.map(|owner| self.lower_type(&self.ast.implementations[owner.0].target));
        let mut cx = FunctionLowering {
            root: self,
            scopes: vec![HashMap::new()],
            locals: Vec::new(),
            self_ty,
            expected_return: Type::Unit,
            generic_traits: function
                .generics
                .iter()
                .map(|generic| {
                    (
                        generic.name.clone(),
                        generic
                            .constraints
                            .iter()
                            .filter_map(|constraint| {
                                self.trait_names.get(&constraint.name).copied()
                            })
                            .collect(),
                    )
                })
                .collect(),
        };
        cx.expected_return = function
            .return_type
            .as_ref()
            .map(|x| cx.lower_type(x))
            .unwrap_or(Type::Unit);
        let mut parameters = Vec::new();
        for parameter in &function.parameters {
            let ty = cx.lower_type(&parameter.ty);
            parameters.push(cx.declare(&parameter.name, ty, false, true, parameter.name_span)?);
        }
        let body = cx.lower_block_contents(&function.body)?;
        let return_type = cx.expected_return.clone();
        Ok(Function {
            id,
            name: function.name.clone(),
            parameters,
            locals: cx.locals,
            return_type,
            body,
            generic_parameters: owner_impl
                .into_iter()
                .flat_map(|owner| {
                    self.ast.implementations[owner.0]
                        .generics
                        .iter()
                        .map(|generic| generic.name.clone())
                })
                .chain(function.generics.iter().map(|x| x.name.clone()))
                .collect(),
            owner_impl,
            span: function.span,
        })
    }

    fn lower_type(&self, ty: &ast::TypeName) -> Type {
        if matches!(
            ty.qualifier,
            TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer
        ) && ty.name == "ptr"
        {
            return Type::RawPointer {
                mutable: ty.qualifier == TypeQualifier::RawMutPointer,
                inner: Box::new(
                    ty.arguments
                        .first()
                        .map(|argument| self.lower_type(argument))
                        .unwrap_or(Type::Unknown),
                ),
            };
        }
        let base = match ty.name.as_str() {
            "unit" | "Unit" => Type::Unit,
            "bool" => Type::Bool,
            "char" => Type::Char,
            "String" => Type::String,
            "str" => Type::Str,
            "Path" => Type::Path,
            "Instant" => Type::Instant,
            "Duration" => Type::Duration,
            "IoError" => Type::Generic("IoError".into()),
            "[]" => Type::Slice(Box::new(
                ty.arguments
                    .first()
                    .map(|x| self.lower_type(x))
                    .unwrap_or(Type::Unknown),
            )),
            "List" => Type::List(Box::new(
                ty.arguments
                    .first()
                    .map(|x| self.lower_type(x))
                    .unwrap_or(Type::Unknown),
            )),
            "Map" => Type::Map(
                Box::new(
                    ty.arguments
                        .first()
                        .map(|x| self.lower_type(x))
                        .unwrap_or(Type::Unknown),
                ),
                Box::new(
                    ty.arguments
                        .get(1)
                        .map(|x| self.lower_type(x))
                        .unwrap_or(Type::Unknown),
                ),
            ),
            "Set" => Type::Set(Box::new(
                ty.arguments
                    .first()
                    .map(|x| self.lower_type(x))
                    .unwrap_or(Type::Unknown),
            )),
            name if name.starts_with("[;") && name.ends_with(']') => Type::Array(
                Box::new(
                    ty.arguments
                        .first()
                        .map(|x| self.lower_type(x))
                        .unwrap_or(Type::Unknown),
                ),
                name[2..name.len() - 1].parse().unwrap_or(0),
            ),
            "int" => Type::Int {
                signed: true,
                width: None,
            },
            "uint" => Type::Int {
                signed: false,
                width: None,
            },
            "float" | "f64" => Type::Float { width: 64 },
            "f32" => Type::Float { width: 32 },
            name if name.starts_with('i') && name[1..].parse::<u16>().is_ok() => Type::Int {
                signed: true,
                width: name[1..].parse().ok(),
            },
            name if name.starts_with('u') && name[1..].parse::<u16>().is_ok() => Type::Int {
                signed: false,
                width: name[1..].parse().ok(),
            },
            "Option" => Type::Option(Box::new(
                ty.arguments
                    .first()
                    .map(|x| self.lower_type(x))
                    .unwrap_or(Type::Unknown),
            )),
            "Result" => Type::Result(
                Box::new(
                    ty.arguments
                        .first()
                        .map(|x| self.lower_type(x))
                        .unwrap_or(Type::Unknown),
                ),
                Box::new(
                    ty.arguments
                        .get(1)
                        .map(|x| self.lower_type(x))
                        .unwrap_or(Type::Unknown),
                ),
            ),
            name => self
                .struct_names
                .get(name)
                .copied()
                .map(|id| {
                    Type::Struct(
                        id,
                        ty.arguments.iter().map(|x| self.lower_type(x)).collect(),
                    )
                })
                .or_else(|| {
                    self.enum_names.get(name).copied().map(|id| {
                        Type::Enum(
                            id,
                            ty.arguments.iter().map(|x| self.lower_type(x)).collect(),
                        )
                    })
                })
                .unwrap_or_else(|| Type::Generic(name.into())),
        };
        match ty.qualifier {
            TypeQualifier::Owned => base,
            TypeQualifier::SharedReference => Type::Reference {
                mutable: false,
                inner: Box::new(base),
            },
            TypeQualifier::MutableReference => Type::Reference {
                mutable: true,
                inner: Box::new(base),
            },
            TypeQualifier::RawConstPointer => Type::RawPointer {
                mutable: false,
                inner: Box::new(base),
            },
            TypeQualifier::RawMutPointer => Type::RawPointer {
                mutable: true,
                inner: Box::new(base),
            },
        }
    }
}

struct FunctionLowering<'a, 'b> {
    root: &'a Lowering<'b>,
    scopes: Vec<HashMap<String, LocalId>>,
    locals: Vec<Local>,
    self_ty: Option<Type>,
    expected_return: Type,
    generic_traits: HashMap<String, Vec<TraitId>>,
}

impl FunctionLowering<'_, '_> {
    fn lower_type(&self, ty: &ast::TypeName) -> Type {
        if ty.name == "Self" {
            let base = self.self_ty.clone().unwrap_or(Type::Unknown);
            return match ty.qualifier {
                TypeQualifier::Owned => base,
                TypeQualifier::SharedReference => Type::Reference {
                    mutable: false,
                    inner: Box::new(base),
                },
                TypeQualifier::MutableReference => Type::Reference {
                    mutable: true,
                    inner: Box::new(base),
                },
                TypeQualifier::RawConstPointer => Type::RawPointer {
                    mutable: false,
                    inner: Box::new(base),
                },
                TypeQualifier::RawMutPointer => Type::RawPointer {
                    mutable: true,
                    inner: Box::new(base),
                },
            };
        }
        self.root.lower_type(ty)
    }
    fn declare(
        &mut self,
        name: &str,
        ty: Type,
        mutable: bool,
        parameter: bool,
        span: Span,
    ) -> Result<LocalId, Diagnostic> {
        let id = LocalId(self.locals.len());
        self.locals.push(Local {
            id,
            name: name.into(),
            ty,
            mutable,
            parameter,
            span,
        });
        self.scopes.last_mut().unwrap().insert(name.into(), id);
        Ok(id)
    }
    fn lookup(&self, name: &str, span: Span) -> Result<LocalId, Diagnostic> {
        self.lookup_optional(name).ok_or_else(|| {
            Diagnostic::new(
                DiagnosticKind::Internal,
                format!("HIR lowering lost resolved local `{name}`"),
                span,
            )
        })
    }
    fn lookup_optional(&self, name: &str) -> Option<LocalId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }
    fn lower_block(&mut self, block: &ast::Block) -> Result<Block, Diagnostic> {
        self.scopes.push(HashMap::new());
        let result = self.lower_block_contents(block);
        self.scopes.pop();
        result
    }
    fn lower_block_contents(&mut self, block: &ast::Block) -> Result<Block, Diagnostic> {
        let mut statements = Vec::new();
        for statement in &block.statements {
            statements.push(self.lower_statement(statement)?);
        }
        Ok(Block {
            statements,
            span: block.span,
        })
    }
    fn lower_statement(&mut self, statement: &ast::Stmt) -> Result<Statement, Diagnostic> {
        let kind = match &statement.node {
            ast::Statement::Binding {
                kind,
                name,
                name_span,
                annotation,
                value,
            } => {
                let mut value = value.as_ref().map(|x| self.lower_expr(x)).transpose()?;
                let ty = annotation
                    .as_ref()
                    .map(|x| self.lower_type(x))
                    .or_else(|| value.as_ref().map(|x| x.ty.clone()))
                    .unwrap_or(Type::Unknown);
                if let Some(value) = &mut value {
                    fill_unknown(&mut value.ty, &ty);
                    if matches!(
                        (&ty, &value.ty),
                        (
                            Type::Reference {
                                mutable: false,
                                inner: expected,
                            },
                            Type::Reference {
                                mutable: false,
                                inner: actual,
                            }
                        ) if matches!((&**expected, &**actual), (Type::Str, Type::String))
                    ) {
                        value.ty = ty.clone();
                    }
                }
                let local = self.declare(name, ty, *kind == BindingKind::Var, false, *name_span)?;
                StatementKind::Let { local, value }
            }
            ast::Statement::Assignment {
                name,
                operator,
                value,
                ..
            } => {
                let value = self.lower_expr(value)?;
                if let Some(local) = self.lookup_optional(name) {
                    StatementKind::Assign {
                        target: Place {
                            local,
                            projections: vec![],
                        },
                        operator: *operator,
                        value,
                    }
                } else {
                    let ty = value.ty.clone();
                    let local = self.declare(name, ty, true, false, statement.span)?;
                    StatementKind::Let {
                        local,
                        value: Some(value),
                    }
                }
            }
            ast::Statement::PlaceAssignment {
                target,
                operator,
                value,
            } => StatementKind::Assign {
                target: self.lower_place(target)?,
                operator: *operator,
                value: self.lower_expr(value)?,
            },
            ast::Statement::Expression(x) => StatementKind::Expression(self.lower_expr(x)?),
            ast::Statement::Return(x) => {
                let mut value = x.as_ref().map(|x| self.lower_expr(x)).transpose()?;
                if let Some(value) = &mut value {
                    fill_unknown(&mut value.ty, &self.expected_return);
                    coerce_str_view(value, &self.expected_return);
                }
                StatementKind::Return(value)
            }
            ast::Statement::If {
                condition,
                then_branch,
                else_branch,
            } => StatementKind::If {
                condition: self.lower_expr(condition)?,
                then_block: self.lower_block(then_branch)?,
                else_block: else_branch
                    .as_ref()
                    .map(|x| self.lower_block(x))
                    .transpose()?,
            },
            ast::Statement::While { condition, body } => StatementKind::While {
                condition: self.lower_expr(condition)?,
                body: self.lower_block(body)?,
            },
            ast::Statement::For {
                name,
                name_span,
                start,
                end,
                inclusive,
                body,
            } => {
                let start = self.lower_expr(start)?;
                let end = self.lower_expr(end)?;
                self.scopes.push(HashMap::new());
                let local = self.declare(
                    name,
                    Type::Int {
                        signed: true,
                        width: None,
                    },
                    false,
                    false,
                    *name_span,
                )?;
                let body = self.lower_block_contents(body)?;
                self.scopes.pop();
                StatementKind::For {
                    local,
                    start,
                    end,
                    inclusive: *inclusive,
                    body,
                }
            }
            ast::Statement::ForEach {
                name,
                name_span,
                iterable,
                body,
            } => {
                let iterable = self.lower_expr(iterable)?;
                let element = match &iterable.ty {
                    Type::Array(element, _)
                    | Type::Slice(element)
                    | Type::List(element)
                    | Type::Set(element) => (**element).clone(),
                    Type::Reference { inner, .. } => match &**inner {
                        Type::Array(element, _)
                        | Type::Slice(element)
                        | Type::List(element)
                        | Type::Set(element) => (**element).clone(),
                        _ => Type::Unknown,
                    },
                    _ => Type::Unknown,
                };
                let item = if surface_type_is_copy(&element) {
                    element
                } else {
                    Type::Reference {
                        mutable: false,
                        inner: Box::new(element),
                    }
                };
                self.scopes.push(HashMap::new());
                let local = self.declare(name, item, false, false, *name_span)?;
                let body = self.lower_block_contents(body)?;
                self.scopes.pop();
                StatementKind::ForEach {
                    local,
                    iterable,
                    body,
                }
            }
            ast::Statement::Loop(body) => StatementKind::Loop(self.lower_block(body)?),
            ast::Statement::Unsafe(body) => StatementKind::Unsafe(self.lower_block(body)?),
            ast::Statement::Break => StatementKind::Break,
            ast::Statement::Continue => StatementKind::Continue,
        };
        Ok(Statement {
            kind,
            span: statement.span,
        })
    }

    fn lower_expr(&mut self, expression: &ast::Expr) -> Result<Expr, Diagnostic> {
        let (kind, ty) = match &expression.node {
            ast::Expression::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| self.lower_expr(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let element = values
                    .first()
                    .map(|value| value.ty.clone())
                    .unwrap_or(Type::Unknown);
                let length = values.len();
                (
                    ExprKind::Array(values),
                    Type::Array(Box::new(element), length),
                )
            }
            ast::Expression::Index { object, index } => {
                let object = self.lower_expr(object)?;
                let result = match &object.ty {
                    Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                        (**element).clone()
                    }
                    _ => Type::Unknown,
                };
                let index = self.lower_expr(index)?;
                (
                    ExprKind::Index {
                        object: Box::new(object),
                        index: Box::new(index),
                    },
                    result,
                )
            }
            ast::Expression::Subslice { object, start, end } => {
                let object = self.lower_expr(object)?;
                let result = match &object.ty {
                    Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                        Type::Slice(Box::new((**element).clone()))
                    }
                    Type::String | Type::Str => Type::Str,
                    _ => Type::Unknown,
                };
                let start = self.lower_expr(start)?;
                let end = self.lower_expr(end)?;
                (
                    ExprKind::Subslice {
                        object: Box::new(object),
                        start: Box::new(start),
                        end: Box::new(end),
                    },
                    result,
                )
            }
            ast::Expression::Integer(x) => (
                ExprKind::Constant(Constant::Unsigned(*x, None)),
                Type::Int {
                    signed: true,
                    width: None,
                },
            ),
            ast::Expression::Float(x) => (
                ExprKind::Constant(Constant::Float(*x, 64)),
                Type::Float { width: 64 },
            ),
            ast::Expression::String(x) => (
                ExprKind::Constant(Constant::String(x.clone())),
                Type::String,
            ),
            ast::Expression::Character(x) => (ExprKind::Constant(Constant::Char(*x)), Type::Char),
            ast::Expression::Bool(x) => (ExprKind::Constant(Constant::Bool(*x)), Type::Bool),
            ast::Expression::Identifier(name) => {
                if let Ok(local) = self.lookup(name, expression.span) {
                    (ExprKind::Local(local), self.locals[local.0].ty.clone())
                } else if let Some(function) = self.root.function_names.get(name).copied() {
                    let f = &self.root.ast.functions[function.0];
                    (
                        ExprKind::Function(function),
                        Type::Function(
                            f.parameters
                                .iter()
                                .map(|x| self.lower_type(&x.ty))
                                .collect(),
                            Box::new(
                                f.return_type
                                    .as_ref()
                                    .map(|x| self.lower_type(x))
                                    .unwrap_or(Type::Unit),
                            ),
                        ),
                    )
                } else if let Some((enum_id, variant_id)) = self.find_variant(name) {
                    (
                        ExprKind::Variant {
                            enum_id,
                            variant_id,
                        },
                        Type::Enum(enum_id, vec![]),
                    )
                } else if matches!(name.as_str(), "None") {
                    (
                        ExprKind::Variant {
                            enum_id: EnumId(usize::MAX),
                            variant_id: builtin_variant(name),
                        },
                        Type::Generic("Option".into()),
                    )
                } else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        format!("HIR lowering could not resolve `{name}`"),
                        expression.span,
                    ));
                }
            }
            ast::Expression::StructConstruct { name, fields, .. } => {
                let id = self.root.struct_names[name];
                let declaration = &self.root.ast.structs[id.0];
                let mut lowered = Vec::new();
                for field in fields {
                    let index = declaration
                        .fields
                        .iter()
                        .position(|x| x.name == field.name)
                        .unwrap();
                    lowered.push((index, self.lower_expr(&field.value)?));
                }
                let mut inferred = HashMap::new();
                for (index, value) in &lowered {
                    infer_named_type(
                        &declaration.fields[*index].ty,
                        &value.ty,
                        &declaration.generics,
                        &mut inferred,
                    );
                }
                let arguments = declaration
                    .generics
                    .iter()
                    .map(|generic| {
                        inferred
                            .get(&generic.name)
                            .cloned()
                            .unwrap_or_else(|| Type::Generic(generic.name.clone()))
                    })
                    .collect();
                (
                    ExprKind::Struct {
                        id,
                        fields: lowered,
                    },
                    Type::Struct(id, arguments),
                )
            }
            ast::Expression::FieldAccess { object, field, .. } => {
                if let ast::Expression::Identifier(owner) = &object.node
                    && let Some(enum_id) = self.root.enum_names.get(owner).copied()
                {
                    let variant_id = self.find_variant_in(enum_id, field).unwrap();
                    return Ok(Expr {
                        kind: ExprKind::Variant {
                            enum_id,
                            variant_id,
                        },
                        ty: Type::Enum(enum_id, vec![]),
                        span: expression.span,
                    });
                }
                let object = self.lower_expr(object)?;
                let (index, ty) = self.field(&object.ty, field, expression.span)?;
                (
                    ExprKind::Field {
                        object: Box::new(object),
                        index,
                    },
                    ty,
                )
            }
            ast::Expression::Move(x) => {
                let place = self.lower_place(x)?;
                let ty = self.place_type(&place);
                (ExprKind::Move(place), ty)
            }
            ast::Expression::Borrow { mutable, target } => {
                let place = self.lower_place(target)?;
                let inner = self.place_type(&place);
                (
                    ExprKind::Borrow {
                        mutable: *mutable,
                        place,
                    },
                    Type::Reference {
                        mutable: *mutable,
                        inner: Box::new(inner),
                    },
                )
            }
            ast::Expression::Dereference(x) => {
                let x = self.lower_expr(x)?;
                let (ty, raw) = match &x.ty {
                    Type::Reference { inner, .. } => ((**inner).clone(), false),
                    Type::RawPointer { inner, .. } => ((**inner).clone(), true),
                    _ => (Type::Unknown, false),
                };
                (ExprKind::Dereference(Box::new(x), raw), ty)
            }
            ast::Expression::Unary { operator, operand } => {
                let operand = self.lower_expr(operand)?;
                let ty = operand.ty.clone();
                (
                    ExprKind::Unary {
                        operator: *operator,
                        operand: Box::new(operand),
                    },
                    ty,
                )
            }
            ast::Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.lower_expr(left)?;
                let right = self.lower_expr(right)?;
                let ty = if matches!(
                    operator,
                    BinaryOperator::Equal
                        | BinaryOperator::NotEqual
                        | BinaryOperator::Less
                        | BinaryOperator::LessEqual
                        | BinaryOperator::Greater
                        | BinaryOperator::GreaterEqual
                        | BinaryOperator::And
                        | BinaryOperator::Or
                ) {
                    Type::Bool
                } else {
                    left.ty.clone()
                };
                (
                    ExprKind::Binary {
                        left: Box::new(left),
                        operator: *operator,
                        right: Box::new(right),
                    },
                    ty,
                )
            }
            ast::Expression::Call { callee, arguments } => {
                return self.lower_call(callee, arguments, expression.span);
            }
            ast::Expression::Match { value, arms } => {
                let value = self.lower_expr(value)?;
                let mut lowered = Vec::new();
                let mut result_ty = Type::Unknown;
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    let pattern =
                        self.lower_pattern(&arm.pattern.node, &value.ty, arm.pattern.span)?;
                    let arm_value = self.lower_expr(&arm.value)?;
                    result_ty = merge_types(&result_ty, &arm_value.ty);
                    self.scopes.pop();
                    lowered.push(MatchArm {
                        pattern,
                        value: arm_value,
                        span: arm.span,
                    });
                }
                for arm in &mut lowered {
                    fill_unknown(&mut arm.value.ty, &result_ty);
                }
                (
                    ExprKind::Match {
                        value: Box::new(value),
                        arms: lowered,
                    },
                    result_ty,
                )
            }
            ast::Expression::Try(x) => {
                let x = self.lower_expr(x)?;
                let ty = match &x.ty {
                    Type::Option(inner) | Type::Result(inner, _) => (**inner).clone(),
                    _ => Type::Unknown,
                };
                (ExprKind::Try(Box::new(x)), ty)
            }
        };
        Ok(Expr {
            kind,
            ty,
            span: expression.span,
        })
    }

    fn lower_call(
        &mut self,
        callee: &ast::Expr,
        arguments: &[ast::Expr],
        span: Span,
    ) -> Result<Expr, Diagnostic> {
        if let ast::Expression::FieldAccess { object, field, .. } = &callee.node {
            if let ast::Expression::Identifier(owner) = &object.node {
                if owner == "String" {
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("String.{field}")),
                            arguments: args,
                            receiver: None,
                            substitutions: vec![],
                        }),
                        ty: Type::String,
                        span,
                    });
                }
                if owner == "List" {
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    let element = args
                        .first()
                        .map(|value| value.ty.clone())
                        .unwrap_or(Type::Unknown);
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("List.{field}")),
                            arguments: args,
                            receiver: None,
                            substitutions: vec![],
                        }),
                        ty: Type::List(Box::new(element)),
                        span,
                    });
                }
                if owner == "Map" {
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    let key = args
                        .first()
                        .map(|value| value.ty.clone())
                        .unwrap_or(Type::Unknown);
                    let value = args
                        .get(1)
                        .map(|value| value.ty.clone())
                        .unwrap_or(Type::Unknown);
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("Map.{field}")),
                            arguments: args,
                            receiver: None,
                            substitutions: vec![key.clone(), value.clone()],
                        }),
                        ty: Type::Map(Box::new(key), Box::new(value)),
                        span,
                    });
                }
                if owner == "Set" {
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    let element = args
                        .first()
                        .map(|value| value.ty.clone())
                        .unwrap_or(Type::Unknown);
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("Set.{field}")),
                            arguments: args,
                            receiver: None,
                            substitutions: vec![element.clone()],
                        }),
                        ty: Type::Set(Box::new(element)),
                        span,
                    });
                }
                if matches!(
                    owner.as_str(),
                    "Path" | "File" | "Directory" | "Time" | "Duration"
                ) {
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    let io = Type::Generic("IoError".into());
                    let ty = match (owner.as_str(), field.as_str()) {
                        ("Path", _) => Type::Path,
                        ("File", "read_text") => Type::Result(Box::new(Type::String), Box::new(io)),
                        ("File", "read_bytes") => Type::Result(
                            Box::new(Type::List(Box::new(Type::Int {
                                signed: false,
                                width: Some(8),
                            }))),
                            Box::new(io),
                        ),
                        ("File", "size" | "modified_seconds") => Type::Result(
                            Box::new(Type::Int {
                                signed: false,
                                width: None,
                            }),
                            Box::new(io),
                        ),
                        ("File", "exists") | ("Directory", "exists") => Type::Bool,
                        ("Directory", "read") => {
                            Type::Result(Box::new(Type::List(Box::new(Type::Path))), Box::new(io))
                        }
                        ("File", _) | ("Directory", _) => {
                            Type::Result(Box::new(Type::Unit), Box::new(io))
                        }
                        ("Time", "now") => Type::Instant,
                        ("Time", "unix_seconds") => Type::Int {
                            signed: false,
                            width: None,
                        },
                        ("Time", "sleep") => Type::Unit,
                        ("Duration", _) => Type::Duration,
                        _ => Type::Unknown,
                    };
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("{owner}.{field}")),
                            arguments: args,
                            receiver: None,
                            substitutions: vec![],
                        }),
                        ty,
                        span,
                    });
                }
                if let Some(enum_id) = self.root.enum_names.get(owner).copied() {
                    let variant_id = self.find_variant_in(enum_id, field).unwrap();
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    let declaration = &self.root.ast.enums[enum_id.0];
                    let variant = declaration
                        .variants
                        .iter()
                        .find(|candidate| candidate.name == *field)
                        .unwrap();
                    let mut inferred = HashMap::new();
                    for (template, value) in variant.payload.iter().zip(&args) {
                        infer_named_type(template, &value.ty, &declaration.generics, &mut inferred);
                    }
                    let type_arguments = declaration
                        .generics
                        .iter()
                        .map(|generic| {
                            inferred
                                .get(&generic.name)
                                .cloned()
                                .unwrap_or_else(|| Type::Generic(generic.name.clone()))
                        })
                        .collect();
                    return Ok(Expr {
                        kind: ExprKind::EnumConstruct {
                            enum_id,
                            variant_id,
                            payload: args,
                        },
                        ty: Type::Enum(enum_id, type_arguments),
                        span,
                    });
                }
                if is_numeric_name(owner) {
                    let args = arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?;
                    let numeric = numeric_type(owner);
                    let ty = if field == "try_from" {
                        Type::Result(
                            Box::new(numeric),
                            Box::new(Type::Generic("ConversionError".into())),
                        )
                    } else {
                        numeric
                    };
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("{owner}.{field}")),
                            arguments: args,
                            receiver: None,
                            substitutions: vec![],
                        }),
                        ty,
                        span,
                    });
                }
            }
            let receiver = self.lower_expr(object)?;
            let ergonomic_field = match field.as_str() {
                "add" => "push",
                "count" => "len",
                "empty" => "is_empty",
                other => other,
            };
            if matches!(
                receiver.ty,
                Type::Array(_, _) | Type::Slice(_) | Type::List(_) | Type::Map(_, _) | Type::Set(_)
            ) && matches!(ergonomic_field, "len" | "is_empty")
            {
                let collection = match receiver.ty {
                    Type::List(_) => Some("List"),
                    Type::Map(_, _) => Some("Map"),
                    Type::Set(_) => Some("Set"),
                    _ => None,
                };
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(if let Some(collection) = collection {
                            format!("{collection}.{ergonomic_field}")
                        } else if ergonomic_field == "is_empty" {
                            "slice_is_empty".into()
                        } else {
                            "array_len".into()
                        }),
                        arguments: vec![receiver],
                        receiver: collection.map(|_| ReceiverMode::Shared),
                        substitutions: vec![],
                    }),
                    ty: if ergonomic_field == "is_empty" {
                        Type::Bool
                    } else {
                        Type::Int {
                            signed: false,
                            width: None,
                        }
                    },
                    span,
                });
            }
            if let Type::Array(element, _) | Type::Slice(element) = &receiver.ty
                && ergonomic_field == "iter"
            {
                let element = element.clone();
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic("collection.iter".into()),
                        arguments: vec![receiver],
                        receiver: Some(ReceiverMode::Shared),
                        substitutions: vec![(*element).clone()],
                    }),
                    ty: Type::Slice(element),
                    span,
                });
            }
            if let Type::List(element) = &receiver.ty
                && matches!(
                    ergonomic_field,
                    "capacity"
                        | "push"
                        | "pop"
                        | "get"
                        | "get_mut"
                        | "insert"
                        | "remove"
                        | "clear"
                        | "iter"
                )
            {
                let element = element.clone();
                let mut args = vec![receiver];
                args.extend(
                    arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                let ty = match ergonomic_field {
                    "iter" => Type::Slice(element.clone()),
                    "capacity" => Type::Int {
                        signed: false,
                        width: None,
                    },
                    "pop" => Type::Option(element.clone()),
                    "get" | "get_mut" => Type::Option(Box::new(Type::Reference {
                        mutable: ergonomic_field == "get_mut",
                        inner: element.clone(),
                    })),
                    "remove" => (*element).clone(),
                    _ => Type::Unit,
                };
                let mode = if matches!(ergonomic_field, "capacity" | "get" | "iter") {
                    ReceiverMode::Shared
                } else {
                    ReceiverMode::Mutable
                };
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(format!("List.{ergonomic_field}")),
                        arguments: args,
                        receiver: Some(mode),
                        substitutions: vec![(*element).clone()],
                    }),
                    ty,
                    span,
                });
            }
            if let Type::Map(key, value) = &receiver.ty {
                let key = key.clone();
                let value = value.clone();
                let method = match ergonomic_field {
                    "contains_key" => "has",
                    "insert" => "set",
                    other => other,
                };
                if matches!(
                    method,
                    "capacity"
                        | "has"
                        | "get"
                        | "get_mut"
                        | "set"
                        | "remove"
                        | "clear"
                        | "keys"
                        | "values"
                ) {
                    let mut args = vec![receiver];
                    args.extend(
                        arguments
                            .iter()
                            .map(|x| self.lower_expr(x))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                    let ty = match method {
                        "keys" => Type::Slice(key.clone()),
                        "values" => Type::Slice(value.clone()),
                        "capacity" => Type::Int {
                            signed: false,
                            width: None,
                        },
                        "has" => Type::Bool,
                        "get" | "get_mut" => Type::Option(Box::new(Type::Reference {
                            mutable: method == "get_mut",
                            inner: value.clone(),
                        })),
                        "set" | "remove" => Type::Option(value.clone()),
                        _ => Type::Unit,
                    };
                    let mode = if matches!(method, "capacity" | "has" | "get" | "keys" | "values") {
                        ReceiverMode::Shared
                    } else {
                        ReceiverMode::Mutable
                    };
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("Map.{method}")),
                            arguments: args,
                            receiver: Some(mode),
                            substitutions: vec![(*key).clone(), (*value).clone()],
                        }),
                        ty,
                        span,
                    });
                }
            }
            if let Type::Set(element) = &receiver.ty {
                let element = element.clone();
                let method = match ergonomic_field {
                    "push" => "add",
                    "contains" => "has",
                    "insert" => "add",
                    other => other,
                };
                if matches!(
                    method,
                    "capacity" | "has" | "add" | "remove" | "clear" | "iter"
                ) {
                    let mut args = vec![receiver];
                    args.extend(
                        arguments
                            .iter()
                            .map(|x| self.lower_expr(x))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                    let ty = match method {
                        "iter" => Type::Slice(element.clone()),
                        "capacity" => Type::Int {
                            signed: false,
                            width: None,
                        },
                        "has" | "add" | "remove" => Type::Bool,
                        _ => Type::Unit,
                    };
                    let mode = if matches!(method, "capacity" | "has" | "iter") {
                        ReceiverMode::Shared
                    } else {
                        ReceiverMode::Mutable
                    };
                    return Ok(Expr {
                        kind: ExprKind::Call(Call {
                            target: CallTarget::Intrinsic(format!("Set.{method}")),
                            arguments: args,
                            receiver: Some(mode),
                            substitutions: vec![(*element).clone()],
                        }),
                        ty,
                        span,
                    });
                }
            }
            if matches!(receiver.ty, Type::Path) {
                let mut args = vec![receiver];
                args.extend(
                    arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                let ty = match field.as_str() {
                    "join" => Type::Path,
                    "as_string" => Type::String,
                    "name" | "extension" => Type::Option(Box::new(Type::String)),
                    "parent" => Type::Option(Box::new(Type::Path)),
                    "len" => Type::Int {
                        signed: false,
                        width: None,
                    },
                    _ => Type::Bool,
                };
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(format!("Path.{field}")),
                        arguments: args,
                        receiver: Some(ReceiverMode::Shared),
                        substitutions: vec![],
                    }),
                    ty,
                    span,
                });
            }
            if matches!(receiver.ty, Type::Instant) && field == "elapsed" {
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic("Instant.elapsed".into()),
                        arguments: vec![receiver],
                        receiver: None,
                        substitutions: vec![],
                    }),
                    ty: Type::Duration,
                    span,
                });
            }
            if matches!(receiver.ty, Type::Duration) {
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(format!("Duration.{field}")),
                        arguments: vec![receiver],
                        receiver: None,
                        substitutions: vec![],
                    }),
                    ty: Type::Int {
                        signed: false,
                        width: None,
                    },
                    span,
                });
            }
            if matches!(receiver.ty, Type::String | Type::Str)
                && matches!(field.as_str(), "len" | "capacity" | "is_empty")
            {
                if matches!(receiver.ty, Type::Str) && field == "capacity" {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "HIR lowering received invalid `str.capacity()`",
                        span,
                    ));
                }
                let ty = if field == "is_empty" {
                    Type::Bool
                } else {
                    Type::Int {
                        signed: false,
                        width: None,
                    }
                };
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(format!("String.{field}")),
                        arguments: vec![receiver],
                        receiver: Some(ReceiverMode::Shared),
                        substitutions: vec![],
                    }),
                    ty,
                    span,
                });
            }
            if matches!(receiver.ty, Type::String | Type::Str)
                && matches!(field.as_str(), "contains" | "starts_with" | "ends_with")
            {
                let mut args = vec![receiver];
                args.extend(
                    arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(format!("String.{field}")),
                        arguments: args,
                        receiver: Some(ReceiverMode::Shared),
                        substitutions: vec![],
                    }),
                    ty: Type::Bool,
                    span,
                });
            }
            if matches!(receiver.ty, Type::String)
                && matches!(
                    field.as_str(),
                    "push" | "push_str" | "append" | "add" | "clear"
                )
            {
                let mut args = vec![receiver];
                args.extend(
                    arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(format!(
                            "String.{}",
                            if matches!(field.as_str(), "append" | "add") {
                                "push_str"
                            } else {
                                field
                            }
                        )),
                        arguments: args,
                        receiver: Some(ReceiverMode::Mutable),
                        substitutions: vec![],
                    }),
                    ty: Type::Unit,
                    span,
                });
            }
            if let Some((target, mode, result, substitutions)) =
                self.resolve_method(&receiver.ty, field)
            {
                let mut args = vec![receiver];
                args.extend(
                    arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target,
                        arguments: args,
                        receiver: Some(mode),
                        substitutions,
                    }),
                    ty: result,
                    span,
                });
            }
            if matches!(receiver.ty, Type::Int { .. } | Type::Float { .. }) {
                let ty = receiver.ty.clone();
                let mut args = vec![receiver];
                args.extend(
                    arguments
                        .iter()
                        .map(|x| self.lower_expr(x))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(field.clone()),
                        arguments: args,
                        receiver: None,
                        substitutions: vec![],
                    }),
                    ty,
                    span,
                });
            }
        }
        if let ast::Expression::Identifier(name) = &callee.node {
            if name == "String" {
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic("String.new".into()),
                        arguments: vec![],
                        receiver: None,
                        substitutions: vec![],
                    }),
                    ty: Type::String,
                    span,
                });
            }
            if name == "Path" {
                let args = arguments
                    .iter()
                    .map(|x| self.lower_expr(x))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic("Path.new".into()),
                        arguments: args,
                        receiver: None,
                        substitutions: vec![],
                    }),
                    ty: Type::Path,
                    span,
                });
            }
            if matches!(name.as_str(), "print" | "Some" | "Ok" | "Err")
                || name.parse::<u16>().is_ok()
                || matches!(
                    name.as_str(),
                    "int"
                        | "uint"
                        | "i8"
                        | "i16"
                        | "i32"
                        | "i64"
                        | "i128"
                        | "u8"
                        | "u16"
                        | "u32"
                        | "u64"
                        | "u128"
                        | "f32"
                        | "f64"
                        | "float"
                )
            {
                let args = arguments
                    .iter()
                    .map(|x| self.lower_expr(x))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(name.as_str(), "Some" | "Ok" | "Err") {
                    let payload = args
                        .first()
                        .map(|value| value.ty.clone())
                        .unwrap_or(Type::Unknown);
                    let ty = if name == "Some" {
                        Type::Option(Box::new(payload))
                    } else if name == "Ok" {
                        Type::Result(Box::new(payload), Box::new(Type::Unknown))
                    } else {
                        Type::Result(Box::new(Type::Unknown), Box::new(payload))
                    };
                    return Ok(Expr {
                        kind: ExprKind::EnumConstruct {
                            enum_id: EnumId(usize::MAX),
                            variant_id: builtin_variant(name),
                            payload: args,
                        },
                        ty,
                        span,
                    });
                }
                let ty = if name == "print" {
                    Type::Unit
                } else if is_numeric_name(name) {
                    numeric_type(name)
                } else {
                    args.first().map(|x| x.ty.clone()).unwrap_or(Type::Unknown)
                };
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Intrinsic(name.clone()),
                        arguments: args,
                        receiver: None,
                        substitutions: vec![],
                    }),
                    ty,
                    span,
                });
            }
            if let Some(target) = self.root.function_names.get(name).copied() {
                let function = &self.root.ast.functions[target.0];
                let mut args = arguments
                    .iter()
                    .map(|x| self.lower_expr(x))
                    .collect::<Result<Vec<_>, _>>()?;
                for ((argument, source), parameter) in
                    args.iter_mut().zip(arguments).zip(&function.parameters)
                {
                    let expected = self.lower_type(&parameter.ty);
                    if matches!(expected, Type::Reference { mutable: false, .. })
                        && !matches!(argument.ty, Type::Reference { .. })
                    {
                        let place = self.lower_place(source)?;
                        let span = argument.span;
                        let borrowed = Type::Reference {
                            mutable: false,
                            inner: Box::new(argument.ty.clone()),
                        };
                        *argument = Expr {
                            kind: ExprKind::Borrow {
                                mutable: false,
                                place,
                            },
                            ty: borrowed,
                            span,
                        };
                    }
                    coerce_str_view(argument, &expected);
                }
                let substitutions = infer_substitutions(function, &args);
                let declared = function
                    .return_type
                    .as_ref()
                    .map(|x| self.lower_type(x))
                    .unwrap_or(Type::Unit);
                let mapping = function
                    .generics
                    .iter()
                    .map(|generic| generic.name.clone())
                    .zip(substitutions.iter().cloned())
                    .collect::<HashMap<_, _>>();
                let ty = substitute_type(&declared, &mapping);
                return Ok(Expr {
                    kind: ExprKind::Call(Call {
                        target: CallTarget::Function(target),
                        arguments: args,
                        receiver: None,
                        substitutions,
                    }),
                    ty,
                    span,
                });
            }
        }
        Err(Diagnostic::new(
            DiagnosticKind::Internal,
            "HIR lowering encountered an unresolved call target",
            span,
        ))
    }

    fn lower_place(&mut self, expr: &ast::Expr) -> Result<Place, Diagnostic> {
        match &expr.node {
            ast::Expression::Identifier(name) => Ok(Place {
                local: self.lookup(name, expr.span)?,
                projections: vec![],
            }),
            ast::Expression::FieldAccess { object, field, .. } => {
                let mut place = self.lower_place(object)?;
                let ty = self.place_type(&place);
                if matches!(ty, Type::Reference { .. }) {
                    place.projections.push(Projection::SafeDereference);
                }
                let (index, _) = self.field(&ty, field, expr.span)?;
                place.projections.push(Projection::Field(index));
                Ok(place)
            }
            ast::Expression::Index { object, index } => {
                let mut place = self.lower_place(object)?;
                place
                    .projections
                    .push(Projection::Index(Box::new(self.lower_expr(index)?)));
                Ok(place)
            }
            ast::Expression::Subslice { object, start, end } => {
                let mut place = self.lower_place(object)?;
                place.projections.push(Projection::Subslice {
                    start: Box::new(self.lower_expr(start)?),
                    end: Box::new(self.lower_expr(end)?),
                });
                Ok(place)
            }
            ast::Expression::Dereference(x) => {
                let mut place = self.lower_place(x)?;
                let projection = match self.place_type(&place) {
                    Type::RawPointer { .. } => Projection::RawDereference,
                    _ => Projection::SafeDereference,
                };
                place.projections.push(projection);
                Ok(place)
            }
            _ => Err(Diagnostic::new(
                DiagnosticKind::Internal,
                "HIR place lowering received a non-place expression",
                expr.span,
            )),
        }
    }
    fn place_type(&self, place: &Place) -> Type {
        let mut ty = self.locals[place.local.0].ty.clone();
        for projection in &place.projections {
            match projection {
                Projection::SafeDereference | Projection::RawDereference => {
                    if let Type::Reference { inner, .. } | Type::RawPointer { inner, .. } = ty {
                        ty = *inner
                    }
                }
                Projection::Field(index) => {
                    if let Ok((_, field_ty)) = self.field_by_index(&ty, *index) {
                        ty = field_ty
                    }
                }
                Projection::VariantField(_, _) => ty = Type::Unknown,
                Projection::Index(_) => {
                    if let Type::Array(element, _) | Type::Slice(element) | Type::List(element) = ty
                    {
                        ty = *element
                    }
                }
                Projection::Subslice { .. } => {
                    ty = match ty {
                        Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                            Type::Slice(element)
                        }
                        Type::String | Type::Str => Type::Str,
                        _ => Type::Unknown,
                    };
                }
            }
        }
        ty
    }
    fn field(&self, ty: &Type, name: &str, span: Span) -> Result<(usize, Type), Diagnostic> {
        let ty = match ty {
            Type::Reference { inner, .. } => &**inner,
            x => x,
        };
        if let Type::Struct(id, arguments) = ty {
            let declaration = &self.root.ast.structs[id.0];
            let field = declaration
                .fields
                .iter()
                .enumerate()
                .find(|(_, x)| x.name == name)
                .unwrap();
            let substitutions = declaration
                .generics
                .iter()
                .map(|generic| generic.name.clone())
                .zip(arguments.iter().cloned())
                .collect();
            Ok((
                field.0,
                substitute_type(&self.lower_type(&field.1.ty), &substitutions),
            ))
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Internal,
                "field lost its resolved nominal type during HIR lowering",
                span,
            ))
        }
    }
    fn field_by_index(&self, ty: &Type, index: usize) -> Result<(usize, Type), ()> {
        let ty = match ty {
            Type::Reference { inner, .. } => &**inner,
            x => x,
        };
        if let Type::Struct(id, arguments) = ty {
            let declaration = &self.root.ast.structs[id.0];
            let substitutions = declaration
                .generics
                .iter()
                .map(|generic| generic.name.clone())
                .zip(arguments.iter().cloned())
                .collect();
            Ok((
                index,
                substitute_type(
                    &self.lower_type(&declaration.fields[index].ty),
                    &substitutions,
                ),
            ))
        } else {
            Err(())
        }
    }
    fn find_variant(&self, name: &str) -> Option<(EnumId, VariantId)> {
        self.root.ast.enums.iter().enumerate().find_map(|(e, x)| {
            x.variants
                .iter()
                .position(|v| v.name == name)
                .map(|v| (EnumId(e), self.variant_id(EnumId(e), v)))
        })
    }
    fn find_variant_in(&self, enum_id: EnumId, name: &str) -> Option<VariantId> {
        self.root.ast.enums[enum_id.0]
            .variants
            .iter()
            .position(|x| x.name == name)
            .map(|v| self.variant_id(enum_id, v))
    }
    fn variant_id(&self, enum_id: EnumId, index: usize) -> VariantId {
        VariantId(
            self.root
                .ast
                .enums
                .iter()
                .take(enum_id.0)
                .map(|x| x.variants.len())
                .sum::<usize>()
                + index,
        )
    }
    fn lower_pattern(
        &mut self,
        pattern: &ast::Pattern,
        matched: &Type,
        span: Span,
    ) -> Result<Pattern, Diagnostic> {
        Ok(match pattern {
            ast::Pattern::Wildcard => Pattern::Wildcard,
            ast::Pattern::Binding(name) => {
                Pattern::Binding(self.declare(name, matched.clone(), false, false, span)?)
            }
            ast::Pattern::Integer(x) => Pattern::Constant(Constant::Unsigned(*x, None)),
            ast::Pattern::String(x) => Pattern::Constant(Constant::String(x.clone())),
            ast::Pattern::Character(x) => Pattern::Constant(Constant::Char(*x)),
            ast::Pattern::Bool(x) => Pattern::Constant(Constant::Bool(*x)),
            ast::Pattern::Variant {
                type_name,
                variant,
                arguments,
            } => {
                let enum_id = type_name
                    .as_ref()
                    .and_then(|x| self.root.enum_names.get(x).copied())
                    .or_else(|| match matched {
                        Type::Enum(id, _) => Some(*id),
                        _ => self
                            .root
                            .enum_names
                            .get(if matches!(variant.as_str(), "Some" | "None") {
                                "Option"
                            } else {
                                "Result"
                            })
                            .copied(),
                    })
                    .unwrap_or(EnumId(usize::MAX));
                let variant_id = if enum_id.0 == usize::MAX {
                    builtin_variant(variant)
                } else {
                    self.find_variant_in(enum_id, variant).unwrap()
                };
                let payload_types = match (matched, variant.as_str()) {
                    (Type::Option(inner), "Some") => vec![(**inner).clone()],
                    (Type::Result(ok, _), "Ok") => vec![(**ok).clone()],
                    (Type::Result(_, error), "Err") => vec![(**error).clone()],
                    (Type::Enum(id, type_arguments), _) => self.root.ast.enums[id.0]
                        .variants
                        .iter()
                        .find(|candidate| candidate.name == *variant)
                        .map(|candidate| {
                            let declaration = &self.root.ast.enums[id.0];
                            let substitutions = declaration
                                .generics
                                .iter()
                                .map(|generic| generic.name.clone())
                                .zip(type_arguments.iter().cloned())
                                .collect();
                            candidate
                                .payload
                                .iter()
                                .map(|ty| substitute_type(&self.lower_type(ty), &substitutions))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                Pattern::Variant {
                    enum_id,
                    variant_id,
                    arguments: arguments
                        .iter()
                        .enumerate()
                        .map(|(index, x)| {
                            self.lower_pattern(
                                &x.node,
                                payload_types.get(index).unwrap_or(&Type::Unknown),
                                x.span,
                            )
                        })
                        .collect::<Result<_, _>>()?,
                }
            }
        })
    }
    fn resolve_method(
        &self,
        receiver: &Type,
        name: &str,
    ) -> Option<(CallTarget, ReceiverMode, Type, Vec<Type>)> {
        if let Type::Generic(generic) = receiver {
            for trait_id in self.generic_traits.get(generic)? {
                let declaration = &self.root.ast.traits[trait_id.0];
                if let Some((method_index, method)) = declaration
                    .methods
                    .iter()
                    .enumerate()
                    .find(|(_, method)| method.name == name)
                {
                    let mode = match method
                        .parameters
                        .first()
                        .map(|parameter| parameter.ty.qualifier)
                        .unwrap_or(TypeQualifier::Owned)
                    {
                        TypeQualifier::SharedReference => ReceiverMode::Shared,
                        TypeQualifier::MutableReference => ReceiverMode::Mutable,
                        _ => ReceiverMode::Move,
                    };
                    return Some((
                        CallTarget::TraitMethod {
                            trait_id: *trait_id,
                            method: method_index,
                        },
                        mode,
                        method
                            .return_type
                            .as_ref()
                            .map(|ty| self.lower_type(ty))
                            .unwrap_or(Type::Unit),
                        vec![],
                    ));
                }
            }
        }
        let nominal = match receiver {
            Type::Struct(id, _) => Some((true, id.0)),
            Type::Enum(id, _) => Some((false, id.0)),
            Type::Reference { inner, .. } => match &**inner {
                Type::Struct(id, _) => Some((true, id.0)),
                Type::Enum(id, _) => Some((false, id.0)),
                _ => None,
            },
            _ => None,
        }?;
        for (impl_index, implementation) in self.root.ast.implementations.iter().enumerate() {
            let matches = if nominal.0 {
                self.root
                    .struct_names
                    .get(&implementation.target.name)
                    .is_some_and(|x| x.0 == nominal.1)
            } else {
                self.root
                    .enum_names
                    .get(&implementation.target.name)
                    .is_some_and(|x| x.0 == nominal.1)
            };
            if matches
                && let Some((method_index, method)) = implementation
                    .methods
                    .iter()
                    .enumerate()
                    .find(|(_, x)| x.name == name)
            {
                let concrete_receiver = match receiver {
                    Type::Reference { inner, .. } => &**inner,
                    other => other,
                };
                let pattern = self.lower_type(&implementation.target);
                let mut inferred = HashMap::new();
                if !infer_hir_type(&pattern, concrete_receiver, &mut inferred) {
                    continue;
                }
                let substitutions = implementation
                    .generics
                    .iter()
                    .map(|generic| {
                        inferred
                            .get(&generic.name)
                            .cloned()
                            .unwrap_or_else(|| Type::Generic(generic.name.clone()))
                    })
                    .collect::<Vec<_>>();
                let substitution_map = implementation
                    .generics
                    .iter()
                    .map(|generic| generic.name.clone())
                    .zip(substitutions.iter().cloned())
                    .collect();
                let mode = match method
                    .parameters
                    .first()
                    .map(|x| x.ty.qualifier)
                    .unwrap_or(TypeQualifier::Owned)
                {
                    TypeQualifier::SharedReference => ReceiverMode::Shared,
                    TypeQualifier::MutableReference => ReceiverMode::Mutable,
                    _ => ReceiverMode::Move,
                };
                return Some((
                    CallTarget::Function(self.root.method_ids[&(impl_index, method_index)]),
                    mode,
                    substitute_type(
                        &method
                            .return_type
                            .as_ref()
                            .map(|x| self.lower_type(x))
                            .unwrap_or(Type::Unit),
                        &substitution_map,
                    ),
                    substitutions,
                ));
            }
        }
        None
    }
}

fn stable_builtin(name: &str) -> usize {
    name.bytes().fold(17usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte as usize)
    }) % 100_000
}
pub fn builtin_variant(name: &str) -> VariantId {
    VariantId(stable_builtin(name))
}
fn is_numeric_name(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "uint"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "f32"
            | "f64"
            | "float"
    )
}

fn surface_type_is_copy(ty: &Type) -> bool {
    match ty {
        Type::Unit
        | Type::Bool
        | Type::Char
        | Type::Int { .. }
        | Type::Float { .. }
        | Type::Reference { .. }
        | Type::RawPointer { .. }
        | Type::Str
        | Type::Slice(_) => true,
        Type::Array(element, _) | Type::Option(element) => surface_type_is_copy(element),
        Type::Result(ok, error) => surface_type_is_copy(ok) && surface_type_is_copy(error),
        Type::String
        | Type::Path
        | Type::List(_)
        | Type::Map(_, _)
        | Type::Set(_)
        | Type::Struct(_, _)
        | Type::Enum(_, _)
        | Type::Generic(_)
        | Type::Function(_, _)
        | Type::Unknown => false,
        Type::Instant | Type::Duration => true,
    }
}
fn numeric_type(name: &str) -> Type {
    match name {
        "float" | "f64" => Type::Float { width: 64 },
        "f32" => Type::Float { width: 32 },
        "uint" => Type::Int {
            signed: false,
            width: None,
        },
        "int" => Type::Int {
            signed: true,
            width: None,
        },
        name if name.starts_with('u') => Type::Int {
            signed: false,
            width: name[1..].parse().ok(),
        },
        name => Type::Int {
            signed: true,
            width: name[1..].parse().ok(),
        },
    }
}
fn infer_substitutions(function: &ast::Function, arguments: &[Expr]) -> Vec<Type> {
    let mut inferred = HashMap::new();
    for (parameter, argument) in function.parameters.iter().zip(arguments) {
        infer_named_type(
            &parameter.ty,
            &argument.ty,
            &function.generics,
            &mut inferred,
        );
    }
    function
        .generics
        .iter()
        .map(|generic| {
            inferred
                .get(&generic.name)
                .cloned()
                .unwrap_or_else(|| Type::Generic(generic.name.clone()))
        })
        .map(|x| match x {
            Type::Unknown => Type::Generic("unresolved".into()),
            other => other,
        })
        .collect()
}

fn infer_named_type(
    template: &ast::TypeName,
    actual: &Type,
    generics: &[ast::GenericParameter],
    inferred: &mut HashMap<String, Type>,
) {
    let actual = match (&template.qualifier, actual) {
        (
            TypeQualifier::SharedReference | TypeQualifier::MutableReference,
            Type::Reference { inner, .. },
        )
        | (
            TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer,
            Type::RawPointer { inner, .. },
        ) => &**inner,
        _ => actual,
    };
    if template.arguments.is_empty()
        && !matches!(
            template.name.as_str(),
            "unit"
                | "bool"
                | "char"
                | "String"
                | "int"
                | "uint"
                | "float"
                | "f32"
                | "f64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
        )
        && generics.iter().any(|generic| generic.name == template.name)
    {
        inferred
            .entry(template.name.clone())
            .or_insert_with(|| actual.clone());
        return;
    }
    let actual_arguments: &[Type] = match actual {
        Type::Struct(_, arguments) | Type::Enum(_, arguments) => arguments,
        Type::Array(element, _)
        | Type::Slice(element)
        | Type::List(element)
        | Type::Option(element) => std::slice::from_ref(element),
        _ => &[],
    };
    for (template, actual) in template.arguments.iter().zip(actual_arguments) {
        infer_named_type(template, actual, generics, inferred);
    }
}

fn infer_hir_type(pattern: &Type, concrete: &Type, inferred: &mut HashMap<String, Type>) -> bool {
    match (pattern, concrete) {
        (Type::Generic(name), concrete) => {
            inferred
                .get(name)
                .is_none_or(|previous| previous == concrete)
                && {
                    inferred.insert(name.clone(), concrete.clone());
                    true
                }
        }
        (Type::Struct(a, xs), Type::Struct(b, ys)) => {
            a == b
                && xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys)
                    .all(|(x, y)| infer_hir_type(x, y, inferred))
        }
        (Type::Enum(a, xs), Type::Enum(b, ys)) => {
            a == b
                && xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys)
                    .all(|(x, y)| infer_hir_type(x, y, inferred))
        }
        (Type::Option(x), Type::Option(y)) => infer_hir_type(x, y, inferred),
        (Type::Array(x, a), Type::Array(y, b)) if a == b => infer_hir_type(x, y, inferred),
        (Type::Slice(x), Type::Slice(y)) => infer_hir_type(x, y, inferred),
        (Type::List(x), Type::List(y)) => infer_hir_type(x, y, inferred),
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            infer_hir_type(ak, bk, inferred) && infer_hir_type(av, bv, inferred)
        }
        (Type::Set(x), Type::Set(y)) => infer_hir_type(x, y, inferred),
        (
            Type::Reference {
                mutable: a,
                inner: x,
            },
            Type::Reference {
                mutable: b,
                inner: y,
            },
        ) if !*a || *b => infer_hir_type(x, y, inferred),
        (
            Type::RawPointer {
                mutable: a,
                inner: x,
            },
            Type::RawPointer {
                mutable: b,
                inner: y,
            },
        ) if a == b => infer_hir_type(x, y, inferred),
        (Type::Result(a, b), Type::Result(x, y)) => {
            infer_hir_type(a, x, inferred) && infer_hir_type(b, y, inferred)
        }
        _ => pattern == concrete,
    }
}

fn substitute_type(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Generic(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Type::Reference { mutable, inner } => Type::Reference {
            mutable: *mutable,
            inner: Box::new(substitute_type(inner, substitutions)),
        },
        Type::RawPointer { mutable, inner } => Type::RawPointer {
            mutable: *mutable,
            inner: Box::new(substitute_type(inner, substitutions)),
        },
        Type::Struct(id, arguments) => Type::Struct(
            *id,
            arguments
                .iter()
                .map(|argument| substitute_type(argument, substitutions))
                .collect(),
        ),
        Type::Enum(id, arguments) => Type::Enum(
            *id,
            arguments
                .iter()
                .map(|argument| substitute_type(argument, substitutions))
                .collect(),
        ),
        Type::Option(inner) => Type::Option(Box::new(substitute_type(inner, substitutions))),
        Type::Array(inner, length) => {
            Type::Array(Box::new(substitute_type(inner, substitutions)), *length)
        }
        Type::Slice(inner) => Type::Slice(Box::new(substitute_type(inner, substitutions))),
        Type::List(inner) => Type::List(Box::new(substitute_type(inner, substitutions))),
        Type::Map(key, value) => Type::Map(
            Box::new(substitute_type(key, substitutions)),
            Box::new(substitute_type(value, substitutions)),
        ),
        Type::Set(inner) => Type::Set(Box::new(substitute_type(inner, substitutions))),
        Type::Result(ok, error) => Type::Result(
            Box::new(substitute_type(ok, substitutions)),
            Box::new(substitute_type(error, substitutions)),
        ),
        Type::Function(arguments, result) => Type::Function(
            arguments
                .iter()
                .map(|argument| substitute_type(argument, substitutions))
                .collect(),
            Box::new(substitute_type(result, substitutions)),
        ),
        _ => ty.clone(),
    }
}

fn merge_types(left: &Type, right: &Type) -> Type {
    match (left, right) {
        (Type::Unknown, other) | (other, Type::Unknown) => other.clone(),
        (Type::Option(a), Type::Option(b)) => Type::Option(Box::new(merge_types(a, b))),
        (Type::Result(a, b), Type::Result(x, y)) => {
            Type::Result(Box::new(merge_types(a, x)), Box::new(merge_types(b, y)))
        }
        _ if left == right => left.clone(),
        _ => right.clone(),
    }
}

fn fill_unknown(actual: &mut Type, expected: &Type) {
    match (&mut *actual, expected) {
        (Type::Unknown, expected) => *actual = expected.clone(),
        (Type::Generic(name), Type::Option(_)) if name == "Option" => {
            *actual = expected.clone();
        }
        (Type::Option(actual), Type::Option(expected)) => fill_unknown(actual, expected),
        (Type::List(actual), Type::List(expected)) => fill_unknown(actual, expected),
        (Type::Map(ak, av), Type::Map(ek, ev)) => {
            fill_unknown(ak, ek);
            fill_unknown(av, ev);
        }
        (Type::Set(actual), Type::Set(expected)) => fill_unknown(actual, expected),
        (Type::Result(actual_ok, actual_error), Type::Result(expected_ok, expected_error)) => {
            fill_unknown(actual_ok, expected_ok);
            fill_unknown(actual_error, expected_error);
        }
        (
            Type::Reference { inner: actual, .. },
            Type::Reference {
                inner: expected, ..
            },
        )
        | (
            Type::RawPointer { inner: actual, .. },
            Type::RawPointer {
                inner: expected, ..
            },
        ) => {
            fill_unknown(actual, expected);
        }
        (Type::Struct(actual_id, actual_args), Type::Struct(expected_id, expected_args))
            if actual_id == expected_id && actual_args.is_empty() =>
        {
            *actual_args = expected_args.clone();
        }
        (Type::Enum(actual_id, actual_args), Type::Enum(expected_id, expected_args))
            if actual_id == expected_id && actual_args.is_empty() =>
        {
            *actual_args = expected_args.clone();
        }
        _ => {}
    }
}

fn coerce_str_view(actual: &mut Expr, expected: &Type) {
    if matches!(
        (expected, &actual.ty),
        (
            Type::Reference {
                mutable: false,
                inner: expected,
            },
            Type::Reference {
                mutable: false,
                inner: source,
            }
        ) if matches!((&**expected, &**source), (Type::Str, Type::String))
    ) {
        actual.ty = expected.clone();
    }
}

pub fn validate(program: &Program) -> Result<(), Diagnostic> {
    for (index, function) in program.functions.iter().enumerate() {
        if function.id.0 != index {
            return Err(Diagnostic::new(
                DiagnosticKind::Internal,
                "non-contiguous HIR function identity",
                function.span,
            ));
        }
        for local in &function.locals {
            if local.id.0 >= function.locals.len() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Internal,
                    "HIR local identity is out of range",
                    local.span,
                ));
            }
        }
        validate_block(&function.body, function.locals.len())?;
        validate_semantics_block(&function.body, program.functions.len())?;
    }
    Ok(())
}

fn validate_semantics_block(block: &Block, functions: usize) -> Result<(), Diagnostic> {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Let { value, .. } | StatementKind::Return(value) => {
                if let Some(value) = value {
                    validate_semantics_expr(value, functions)?;
                }
            }
            StatementKind::Assign { value, .. } | StatementKind::Expression(value) => {
                validate_semantics_expr(value, functions)?
            }
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                validate_semantics_expr(condition, functions)?;
                validate_semantics_block(then_block, functions)?;
                if let Some(block) = else_block {
                    validate_semantics_block(block, functions)?;
                }
            }
            StatementKind::While { condition, body } => {
                validate_semantics_expr(condition, functions)?;
                validate_semantics_block(body, functions)?;
            }
            StatementKind::For {
                start, end, body, ..
            } => {
                validate_semantics_expr(start, functions)?;
                validate_semantics_expr(end, functions)?;
                validate_semantics_block(body, functions)?;
            }
            StatementKind::ForEach { iterable, body, .. } => {
                validate_semantics_expr(iterable, functions)?;
                validate_semantics_block(body, functions)?;
            }
            StatementKind::Loop(block) | StatementKind::Unsafe(block) => {
                validate_semantics_block(block, functions)?
            }
            StatementKind::Break | StatementKind::Continue => {}
        }
    }
    Ok(())
}

fn validate_semantics_expr(expr: &Expr, functions: usize) -> Result<(), Diagnostic> {
    if contains_unknown(&expr.ty) {
        return Err(Diagnostic::new(
            DiagnosticKind::Internal,
            "HIR expression is missing resolved type information",
            expr.span,
        ));
    }
    match &expr.kind {
        ExprKind::Array(values) => {
            for value in values {
                validate_semantics_expr(value, functions)?;
            }
        }
        ExprKind::Index { object, index } => {
            validate_semantics_expr(object, functions)?;
            validate_semantics_expr(index, functions)?;
        }
        ExprKind::Subslice { object, start, end } => {
            validate_semantics_expr(object, functions)?;
            validate_semantics_expr(start, functions)?;
            validate_semantics_expr(end, functions)?;
        }
        ExprKind::Call(call) => {
            if let CallTarget::Function(target) = call.target
                && target.0 >= functions
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Internal,
                    "HIR call target is out of range",
                    expr.span,
                ));
            }
            for argument in &call.arguments {
                validate_semantics_expr(argument, functions)?;
            }
        }
        ExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                validate_semantics_expr(value, functions)?;
            }
        }
        ExprKind::EnumConstruct { payload, .. } => {
            for value in payload {
                validate_semantics_expr(value, functions)?;
            }
        }
        ExprKind::Field { object, .. }
        | ExprKind::Try(object)
        | ExprKind::Dereference(object, _)
        | ExprKind::Unary {
            operand: object, ..
        } => validate_semantics_expr(object, functions)?,
        ExprKind::Binary { left, right, .. } => {
            validate_semantics_expr(left, functions)?;
            validate_semantics_expr(right, functions)?;
        }
        ExprKind::Match { value, arms } => {
            validate_semantics_expr(value, functions)?;
            for arm in arms {
                validate_semantics_expr(&arm.value, functions)?;
            }
        }
        ExprKind::Constant(_)
        | ExprKind::Local(_)
        | ExprKind::Function(_)
        | ExprKind::Variant { .. }
        | ExprKind::Move(_)
        | ExprKind::Borrow { .. } => {}
    }
    Ok(())
}

fn contains_unknown(ty: &Type) -> bool {
    match ty {
        Type::Unknown => true,
        Type::Reference { inner, .. }
        | Type::RawPointer { inner, .. }
        | Type::Option(inner)
        | Type::Slice(inner)
        | Type::List(inner) => contains_unknown(inner),
        Type::Map(key, value) => contains_unknown(key) || contains_unknown(value),
        Type::Set(inner) => contains_unknown(inner),
        Type::Result(ok, error) => contains_unknown(ok) || contains_unknown(error),
        Type::Struct(_, arguments) | Type::Enum(_, arguments) => {
            arguments.iter().any(contains_unknown)
        }
        Type::Function(arguments, result) => {
            arguments.iter().any(contains_unknown) || contains_unknown(result)
        }
        _ => false,
    }
}

fn validate_block(block: &Block, locals: usize) -> Result<(), Diagnostic> {
    for statement in &block.statements {
        let check = |expr: &Expr| validate_expr(expr, locals);
        match &statement.kind {
            StatementKind::Let { local, value } => {
                if local.0 >= locals {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "HIR binding references an invalid local",
                        statement.span,
                    ));
                }
                if let Some(x) = value {
                    check(x)?;
                }
            }
            StatementKind::Assign { target, value, .. } => {
                if target.local.0 >= locals {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "HIR assignment references an invalid local",
                        statement.span,
                    ));
                }
                check(value)?;
            }
            StatementKind::Expression(x) => check(x)?,
            StatementKind::Return(x) => {
                if let Some(x) = x {
                    check(x)?;
                }
            }
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                check(condition)?;
                validate_block(then_block, locals)?;
                if let Some(x) = else_block {
                    validate_block(x, locals)?;
                }
            }
            StatementKind::While { condition, body } => {
                check(condition)?;
                validate_block(body, locals)?;
            }
            StatementKind::For {
                local,
                start,
                end,
                body,
                ..
            } => {
                if local.0 >= locals {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "HIR for loop references an invalid local",
                        statement.span,
                    ));
                }
                check(start)?;
                check(end)?;
                validate_block(body, locals)?;
            }
            StatementKind::ForEach {
                local,
                iterable,
                body,
            } => {
                if local.0 >= locals {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "HIR collection loop references an invalid local",
                        statement.span,
                    ));
                }
                check(iterable)?;
                validate_block(body, locals)?;
            }
            StatementKind::Loop(x) | StatementKind::Unsafe(x) => validate_block(x, locals)?,
            StatementKind::Break | StatementKind::Continue => {}
        }
    }
    Ok(())
}
fn validate_expr(expr: &Expr, locals: usize) -> Result<(), Diagnostic> {
    let invalid = |local: LocalId| {
        (local.0 >= locals).then(|| {
            Diagnostic::new(
                DiagnosticKind::Internal,
                "HIR expression references an invalid local",
                expr.span,
            )
        })
    };
    match &expr.kind {
        ExprKind::Array(values) => {
            for value in values {
                validate_expr(value, locals)?;
            }
        }
        ExprKind::Index { object, index } => {
            validate_expr(object, locals)?;
            validate_expr(index, locals)?;
        }
        ExprKind::Subslice { object, start, end } => {
            validate_expr(object, locals)?;
            validate_expr(start, locals)?;
            validate_expr(end, locals)?;
        }
        ExprKind::Local(x) => {
            if let Some(error) = invalid(*x) {
                return Err(error);
            }
        }
        ExprKind::Move(x) | ExprKind::Borrow { place: x, .. } => {
            if let Some(error) = invalid(x.local) {
                return Err(error);
            }
        }
        ExprKind::Field { object, .. }
        | ExprKind::Try(object)
        | ExprKind::Dereference(object, _)
        | ExprKind::Unary {
            operand: object, ..
        } => validate_expr(object, locals)?,
        ExprKind::Binary { left, right, .. } => {
            validate_expr(left, locals)?;
            validate_expr(right, locals)?;
        }
        ExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                validate_expr(value, locals)?;
            }
        }
        ExprKind::EnumConstruct { payload, .. } => {
            for value in payload {
                validate_expr(value, locals)?;
            }
        }
        ExprKind::Match { value, arms } => {
            validate_expr(value, locals)?;
            for arm in arms {
                validate_expr(&arm.value, locals)?;
            }
        }
        ExprKind::Call(call) => {
            for argument in &call.arguments {
                validate_expr(argument, locals)?;
            }
        }
        ExprKind::Constant(_) | ExprKind::Function(_) | ExprKind::Variant { .. } => {}
    }
    Ok(())
}

pub fn dump(program: &Program) -> String {
    let mut out = String::new();
    for function in &program.functions {
        out.push_str(&format!(
            "fn{} {}({}) -> {:?} @ {}:{}\n",
            function.id.0,
            function.name,
            function
                .parameters
                .iter()
                .map(|x| format!("_{}", x.0))
                .collect::<Vec<_>>()
                .join(", "),
            function.return_type,
            function.span.start.line,
            function.span.start.column
        ));
        for local in &function.locals {
            out.push_str(&format!(
                "  _{} {}: {:?}{}\n",
                local.id.0,
                local.name,
                local.ty,
                if local.parameter { " [arg]" } else { "" }
            ));
        }
        dump_block(&function.body, 1, &mut out);
    }
    out
}
fn dump_block(block: &Block, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    for statement in &block.statements {
        out.push_str(&format!(
            "{pad}{:?} @ {}:{}\n",
            statement.kind, statement.span.start.line, statement.span.start.column
        ));
    }
}
