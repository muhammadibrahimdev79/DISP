use crate::diagnostics::{SourceFile, Span};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub source_files: Vec<SourceFile>,
    pub module: Option<ModuleDeclaration>,
    pub imports: Vec<ImportDeclaration>,
    pub public_items: Vec<Spanned<String>>,
    pub structs: Vec<StructDeclaration>,
    pub enums: Vec<EnumDeclaration>,
    pub traits: Vec<TraitDeclaration>,
    pub implementations: Vec<Implementation>,
    pub functions: Vec<Function>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDeclaration {
    pub path: Vec<Spanned<String>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportDeclaration {
    pub path: Vec<Spanned<String>>,
    pub items: Option<Vec<ImportItem>>,
    pub public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    pub name: String,
    pub name_span: Span,
    pub alias: String,
    pub alias_span: Span,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParameter {
    pub name: String,
    pub name_span: Span,
    pub constraints: Vec<TypeName>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDeclaration {
    pub name: String,
    pub name_span: Span,
    pub generics: Vec<GenericParameter>,
    pub fields: Vec<FieldDeclaration>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDeclaration {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDeclaration {
    pub name: String,
    pub name_span: Span,
    pub generics: Vec<GenericParameter>,
    pub variants: Vec<VariantDeclaration>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantDeclaration {
    pub name: String,
    pub name_span: Span,
    pub payload: Vec<TypeName>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub name_span: Span,
    pub generics: Vec<GenericParameter>,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeName>,
    pub body: Block,
    pub external: Option<ExternalFunction>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFunction {
    pub abi: ExternalAbi,
    pub library: Option<String>,
    pub link_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAbi {
    C,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDeclaration {
    pub name: String,
    pub name_span: Span,
    pub generics: Vec<GenericParameter>,
    pub associated_types: Vec<(String, Span)>,
    pub methods: Vec<FunctionSignature>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    pub name: String,
    pub name_span: Span,
    pub generics: Vec<GenericParameter>,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeName>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Implementation {
    pub generics: Vec<GenericParameter>,
    pub trait_name: Option<TypeName>,
    pub target: TypeName,
    pub associated_types: Vec<(String, TypeName, Span)>,
    pub methods: Vec<Function>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeName {
    pub name: String,
    pub arguments: Vec<TypeName>,
    pub qualifier: TypeQualifier,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeQualifier {
    Owned,
    SharedReference,
    MutableReference,
    RawConstPointer,
    RawMutPointer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Spanned<Statement>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

pub type Expr = Spanned<Expression>;
pub type Stmt = Spanned<Statement>;

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Binding(String),
    Integer(u128),
    String(String),
    Character(char),
    Bool(bool),
    Variant {
        type_name: Option<String>,
        variant: String,
        arguments: Vec<Spanned<Pattern>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Let,
    Var,
    Const,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Binding {
        kind: BindingKind,
        name: String,
        name_span: Span,
        annotation: Option<TypeName>,
        value: Option<Expr>,
    },
    Assignment {
        name: String,
        name_span: Span,
        operator: AssignmentOperator,
        value: Expr,
    },
    PlaceAssignment {
        target: Expr,
        operator: AssignmentOperator,
        value: Expr,
    },
    Expression(Expr),
    Return(Option<Expr>),
    If {
        condition: Expr,
        then_branch: Block,
        else_branch: Option<Block>,
    },
    While {
        condition: Expr,
        body: Block,
    },
    For {
        name: String,
        name_span: Span,
        start: Expr,
        end: Expr,
        inclusive: bool,
        body: Block,
    },
    ForEach {
        name: String,
        name_span: Span,
        iterable: Expr,
        body: Block,
    },
    Loop(Block),
    Unsafe(Block),
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    Array(Vec<Expr>),
    Integer(u128),
    Float(f64),
    String(String),
    Character(char),
    Bool(bool),
    Closure {
        move_captures: bool,
        parameters: Vec<Parameter>,
        return_type: Option<TypeName>,
        body: ClosureBody,
    },
    Identifier(String),
    StructConstruct {
        name: String,
        name_span: Span,
        fields: Vec<StructFieldValue>,
    },
    FieldAccess {
        object: Box<Expr>,
        field: String,
        field_span: Span,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Subslice {
        object: Box<Expr>,
        start: Box<Expr>,
        end: Box<Expr>,
    },
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Try(Box<Expr>),
    Spawn(Box<Expr>),
    Move(Box<Expr>),
    Borrow {
        mutable: bool,
        target: Box<Expr>,
    },
    Dereference(Box<Expr>),
    Unary {
        operator: UnaryOperator,
        operand: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        operator: BinaryOperator,
        right: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        arguments: Vec<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClosureBody {
    Expression(Box<Expr>),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldValue {
    pub name: String,
    pub name_span: Span,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Negate,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureUse {
    pub span: Span,
    pub mutated: bool,
    pub consumed: bool,
}

#[derive(Debug, Clone, Copy)]
enum CaptureMode {
    Read,
    Mutate,
    Consume,
}

pub fn closure_capture_uses(
    parameters: &[Parameter],
    body: &ClosureBody,
) -> BTreeMap<String, CaptureUse> {
    let mut locals = parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect::<HashSet<_>>();
    let mut captures = BTreeMap::new();
    match body {
        ClosureBody::Expression(expression) => {
            collect_capture_expr(expression, CaptureMode::Consume, &mut locals, &mut captures)
        }
        ClosureBody::Block(block) => collect_capture_block(block, &mut locals, &mut captures),
    }
    captures
}

fn record_capture(
    name: &str,
    span: Span,
    mode: CaptureMode,
    locals: &HashSet<String>,
    captures: &mut BTreeMap<String, CaptureUse>,
) {
    if locals.contains(name) {
        return;
    }
    let capture = captures.entry(name.to_owned()).or_insert(CaptureUse {
        span,
        mutated: false,
        consumed: false,
    });
    capture.mutated |= matches!(mode, CaptureMode::Mutate);
    capture.consumed |= matches!(mode, CaptureMode::Consume);
}

fn collect_capture_block(
    block: &Block,
    outer: &mut HashSet<String>,
    captures: &mut BTreeMap<String, CaptureUse>,
) {
    let mut locals = outer.clone();
    for statement in &block.statements {
        match &statement.node {
            Statement::Binding { name, value, .. } => {
                if let Some(value) = value {
                    collect_capture_expr(value, CaptureMode::Consume, &mut locals, captures);
                }
                locals.insert(name.clone());
            }
            Statement::Assignment {
                name,
                name_span,
                value,
                ..
            } => {
                record_capture(name, *name_span, CaptureMode::Mutate, &locals, captures);
                collect_capture_expr(value, CaptureMode::Consume, &mut locals, captures);
            }
            Statement::PlaceAssignment { target, value, .. } => {
                collect_capture_place(target, &mut locals, captures);
                collect_capture_expr(value, CaptureMode::Consume, &mut locals, captures);
            }
            Statement::Expression(value) => {
                collect_capture_expr(value, CaptureMode::Read, &mut locals, captures)
            }
            Statement::Return(value) => {
                if let Some(value) = value {
                    collect_capture_expr(value, CaptureMode::Consume, &mut locals, captures);
                }
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                collect_capture_expr(condition, CaptureMode::Read, &mut locals, captures);
                collect_capture_block(then_branch, &mut locals, captures);
                if let Some(else_branch) = else_branch {
                    collect_capture_block(else_branch, &mut locals, captures);
                }
            }
            Statement::While { condition, body } => {
                collect_capture_expr(condition, CaptureMode::Read, &mut locals, captures);
                collect_capture_block(body, &mut locals, captures);
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                collect_capture_expr(start, CaptureMode::Read, &mut locals, captures);
                collect_capture_expr(end, CaptureMode::Read, &mut locals, captures);
                let mut loop_locals = locals.clone();
                loop_locals.insert(name.clone());
                collect_capture_block(body, &mut loop_locals, captures);
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                collect_capture_expr(iterable, CaptureMode::Read, &mut locals, captures);
                let mut loop_locals = locals.clone();
                loop_locals.insert(name.clone());
                collect_capture_block(body, &mut loop_locals, captures);
            }
            Statement::Loop(body) | Statement::Unsafe(body) => {
                collect_capture_block(body, &mut locals, captures)
            }
            Statement::Break | Statement::Continue => {}
        }
    }
}

fn collect_capture_place(
    expression: &Expr,
    locals: &mut HashSet<String>,
    captures: &mut BTreeMap<String, CaptureUse>,
) {
    match &expression.node {
        Expression::Identifier(name) => {
            record_capture(name, expression.span, CaptureMode::Mutate, locals, captures)
        }
        Expression::FieldAccess { object, .. } => collect_capture_place(object, locals, captures),
        Expression::Index { object, index } => {
            collect_capture_place(object, locals, captures);
            collect_capture_expr(index, CaptureMode::Read, locals, captures);
        }
        Expression::Subslice { object, start, end } => {
            collect_capture_place(object, locals, captures);
            collect_capture_expr(start, CaptureMode::Read, locals, captures);
            collect_capture_expr(end, CaptureMode::Read, locals, captures);
        }
        Expression::Dereference(target) => {
            collect_capture_expr(target, CaptureMode::Read, locals, captures)
        }
        _ => collect_capture_expr(expression, CaptureMode::Mutate, locals, captures),
    }
}

fn collect_capture_expr(
    expression: &Expr,
    mode: CaptureMode,
    locals: &mut HashSet<String>,
    captures: &mut BTreeMap<String, CaptureUse>,
) {
    match &expression.node {
        Expression::Identifier(name) => {
            record_capture(name, expression.span, mode, locals, captures)
        }
        Expression::Array(values) => {
            for value in values {
                collect_capture_expr(value, CaptureMode::Consume, locals, captures);
            }
        }
        Expression::Closure {
            move_captures,
            parameters,
            body,
            ..
        } => {
            let nested = closure_capture_uses(parameters, body);
            for (name, usage) in nested {
                record_capture(
                    &name,
                    usage.span,
                    if *move_captures {
                        CaptureMode::Consume
                    } else if usage.mutated {
                        CaptureMode::Mutate
                    } else {
                        CaptureMode::Read
                    },
                    locals,
                    captures,
                );
            }
        }
        Expression::StructConstruct { fields, .. } => {
            for field in fields {
                collect_capture_expr(&field.value, CaptureMode::Consume, locals, captures);
            }
        }
        Expression::FieldAccess { object, .. } => {
            collect_capture_expr(object, mode, locals, captures)
        }
        Expression::Index { object, index } => {
            collect_capture_expr(object, mode, locals, captures);
            collect_capture_expr(index, CaptureMode::Read, locals, captures);
        }
        Expression::Subslice { object, start, end } => {
            collect_capture_expr(object, CaptureMode::Read, locals, captures);
            collect_capture_expr(start, CaptureMode::Read, locals, captures);
            collect_capture_expr(end, CaptureMode::Read, locals, captures);
        }
        Expression::Match { value, arms } => {
            collect_capture_expr(value, CaptureMode::Consume, locals, captures);
            for arm in arms {
                let mut arm_locals = locals.clone();
                collect_pattern_bindings(&arm.pattern.node, &mut arm_locals);
                collect_capture_expr(&arm.value, mode, &mut arm_locals, captures);
            }
        }
        Expression::Try(value) | Expression::Move(value) => {
            collect_capture_expr(value, CaptureMode::Consume, locals, captures)
        }
        Expression::Spawn(value) => {
            collect_capture_expr(value, CaptureMode::Consume, locals, captures)
        }
        Expression::Borrow { mutable, target } => {
            if *mutable {
                collect_capture_place(target, locals, captures);
            } else {
                collect_capture_expr(target, CaptureMode::Read, locals, captures);
            }
        }
        Expression::Dereference(value) | Expression::Unary { operand: value, .. } => {
            collect_capture_expr(value, CaptureMode::Read, locals, captures)
        }
        Expression::Binary { left, right, .. } => {
            collect_capture_expr(left, CaptureMode::Read, locals, captures);
            collect_capture_expr(right, CaptureMode::Read, locals, captures);
        }
        Expression::Call { callee, arguments } => {
            collect_capture_expr(callee, CaptureMode::Read, locals, captures);
            for argument in arguments {
                collect_capture_expr(argument, CaptureMode::Read, locals, captures);
            }
        }
        Expression::Integer(_)
        | Expression::Float(_)
        | Expression::String(_)
        | Expression::Character(_)
        | Expression::Bool(_) => {}
    }
}

fn collect_pattern_bindings(pattern: &Pattern, locals: &mut HashSet<String>) {
    match pattern {
        Pattern::Binding(name) => {
            locals.insert(name.clone());
        }
        Pattern::Variant { arguments, .. } => {
            for argument in arguments {
                collect_pattern_bindings(&argument.node, locals);
            }
        }
        _ => {}
    }
}
