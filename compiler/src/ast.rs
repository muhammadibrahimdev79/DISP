use crate::diagnostics::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub structs: Vec<StructDeclaration>,
    pub enums: Vec<EnumDeclaration>,
    pub traits: Vec<TraitDeclaration>,
    pub implementations: Vec<Implementation>,
    pub functions: Vec<Function>,
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
