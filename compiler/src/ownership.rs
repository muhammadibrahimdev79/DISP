use crate::ast::{
    BindingKind, Block, DataQueryKind, Expr, Expression, Function, Pattern, Program, Statement,
    TypeName, TypeQualifier,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ty {
    Copy,
    Owned(String),
    Nominal(String, Vec<Ty>),
    Generic(String),
    Reference(Box<Ty>, bool),
    RawPointer(Box<Ty>, bool),
    MemoryPointer(Box<Ty>, bool),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    Array(Box<Ty>),
    Slice(Box<Ty>),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    Thread(Box<Ty>),
    Future(Box<Ty>),
    Task(Box<Ty>),
    Mutex(Box<Ty>),
    MutexGuard(Box<Ty>),
    Channel(Box<Ty>),
    AtomicInt,
    Str,
    CString,
    CStr,
    CRegistration,
    Memory,
    Path,
    Url,
    Json,
    IpAddress,
    SocketAddress,
    TcpStream,
    TlsStream,
    HttpRequest,
    HttpResponse,
    TcpListener,
    UdpSocket,
    UdpDatagram,
    Instant,
    Duration,
    Function,
    Unit,
}

fn substitute_ownership_ty(ty: Ty, substitutions: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Generic(name) => substitutions
            .get(&name)
            .cloned()
            .unwrap_or(Ty::Generic(name)),
        Ty::Nominal(name, arguments) => Ty::Nominal(
            name,
            arguments
                .into_iter()
                .map(|argument| substitute_ownership_ty(argument, substitutions))
                .collect(),
        ),
        Ty::Reference(inner, mutable) => Ty::Reference(
            Box::new(substitute_ownership_ty(*inner, substitutions)),
            mutable,
        ),
        Ty::RawPointer(inner, mutable) => Ty::RawPointer(
            Box::new(substitute_ownership_ty(*inner, substitutions)),
            mutable,
        ),
        Ty::MemoryPointer(inner, mutable) => Ty::MemoryPointer(
            Box::new(substitute_ownership_ty(*inner, substitutions)),
            mutable,
        ),
        Ty::Option(inner) => Ty::Option(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Result(ok, error) => Ty::Result(
            Box::new(substitute_ownership_ty(*ok, substitutions)),
            Box::new(substitute_ownership_ty(*error, substitutions)),
        ),
        Ty::Array(inner) => Ty::Array(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Slice(inner) => Ty::Slice(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::List(inner) => Ty::List(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Map(key, value) => Ty::Map(
            Box::new(substitute_ownership_ty(*key, substitutions)),
            Box::new(substitute_ownership_ty(*value, substitutions)),
        ),
        Ty::Set(inner) => Ty::Set(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Thread(inner) => Ty::Thread(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Future(inner) => Ty::Future(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Task(inner) => Ty::Task(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::Mutex(inner) => Ty::Mutex(Box::new(substitute_ownership_ty(*inner, substitutions))),
        Ty::MutexGuard(inner) => {
            Ty::MutexGuard(Box::new(substitute_ownership_ty(*inner, substitutions)))
        }
        Ty::Channel(inner) => Ty::Channel(Box::new(substitute_ownership_ty(*inner, substitutions))),
        other => other,
    }
}

fn collect_ownership_substitutions(
    template: &Ty,
    actual: &Ty,
    substitutions: &mut HashMap<String, Ty>,
) {
    match (template, actual) {
        (Ty::Generic(name), actual) => {
            substitutions
                .entry(name.clone())
                .or_insert_with(|| actual.clone());
        }
        (
            Ty::Nominal(template_name, template_arguments),
            Ty::Nominal(actual_name, actual_arguments),
        ) if template_name == actual_name => {
            for (template, actual) in template_arguments.iter().zip(actual_arguments) {
                collect_ownership_substitutions(template, actual, substitutions);
            }
        }
        (Ty::Reference(template, _), Ty::Reference(actual, _))
        | (Ty::RawPointer(template, _), Ty::RawPointer(actual, _))
        | (Ty::MemoryPointer(template, _), Ty::MemoryPointer(actual, _))
        | (Ty::Option(template), Ty::Option(actual))
        | (Ty::Array(template), Ty::Array(actual))
        | (Ty::Slice(template), Ty::Slice(actual))
        | (Ty::List(template), Ty::List(actual))
        | (Ty::Set(template), Ty::Set(actual))
        | (Ty::Thread(template), Ty::Thread(actual))
        | (Ty::Future(template), Ty::Future(actual))
        | (Ty::Task(template), Ty::Task(actual))
        | (Ty::Mutex(template), Ty::Mutex(actual))
        | (Ty::MutexGuard(template), Ty::MutexGuard(actual))
        | (Ty::Channel(template), Ty::Channel(actual)) => {
            collect_ownership_substitutions(template, actual, substitutions);
        }
        (Ty::Result(template_ok, template_error), Ty::Result(actual_ok, actual_error))
        | (Ty::Map(template_ok, template_error), Ty::Map(actual_ok, actual_error)) => {
            collect_ownership_substitutions(template_ok, actual_ok, substitutions);
            collect_ownership_substitutions(template_error, actual_error, substitutions);
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SlotId(usize);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    root: SlotId,
    fields: Vec<String>,
}

#[derive(Debug, Clone)]
enum InitState {
    Uninitialized,
    Initialized,
    Moved { at: Span },
    Partial { fields: HashMap<String, Span> },
}

#[derive(Debug, Clone)]
struct Slot {
    name: String,
    ty: Ty,
    mutable: bool,
    defined: Span,
    state: InitState,
    scope_depth: usize,
    parameter: bool,
    reference_origin: Option<Place>,
    borrow_origins: Vec<(Place, bool)>,
    closure_origins: Vec<(Place, bool)>,
}

#[derive(Debug, Clone)]
struct Loan {
    place: Place,
    mutable: bool,
    borrower: Option<SlotId>,
    at: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    ScopeEnd,
    Return,
    Break,
    Continue,
    Propagation,
}

#[derive(Debug, Clone)]
pub struct DropFact {
    pub name: String,
    pub declaration: Span,
    pub exit: Span,
    pub reason: DropReason,
}

#[derive(Debug, Clone, Default)]
pub struct OwnershipReport {
    pub drops: Vec<DropFact>,
}

#[derive(Debug, Clone, Default)]
struct Scope {
    names: HashMap<String, SlotId>,
    order: Vec<SlotId>,
}

#[derive(Debug, Clone)]
struct Analyzer<'a> {
    program: &'a Program,
    slots: HashMap<SlotId, Slot>,
    scopes: Vec<Scope>,
    loans: Vec<Loan>,
    next_slot: usize,
    copy_types: HashSet<String>,
    generic_copy: HashSet<String>,
    generic_traits: HashMap<String, Vec<String>>,
    self_type: Option<String>,
    return_has_reference: bool,
    report: OwnershipReport,
}

#[derive(Debug, Clone, Copy)]
enum UseMode {
    Read,
    Consume,
}

#[derive(Debug, Clone)]
struct MethodInfo {
    asynchronous: bool,
    parameters: Vec<crate::ast::Parameter>,
    return_type: Option<TypeName>,
}

pub fn check(program: &Program) -> Result<OwnershipReport, Diagnostic> {
    let copy_types = program
        .implementations
        .iter()
        .filter(|implementation| {
            implementation
                .trait_name
                .as_ref()
                .is_some_and(|trait_name| trait_name.name == "Copy")
        })
        .map(|implementation| implementation.target.name.clone())
        .collect();
    let mut analyzer = Analyzer {
        program,
        slots: HashMap::new(),
        scopes: vec![],
        loans: vec![],
        next_slot: 0,
        copy_types,
        generic_copy: HashSet::new(),
        generic_traits: HashMap::new(),
        self_type: None,
        return_has_reference: false,
        report: OwnershipReport::default(),
    };
    analyzer.validate_copy_implementations()?;
    for function in &program.functions {
        analyzer.self_type = None;
        analyzer.check_function(function)?;
    }
    for implementation in &program.implementations {
        analyzer.self_type = Some(implementation.target.name.clone());
        for method in &implementation.methods {
            analyzer.check_function(method)?;
        }
    }
    Ok(analyzer.report)
}

impl<'a> Analyzer<'a> {
    fn validate_copy_implementations(&self) -> Result<(), Diagnostic> {
        for implementation in self
            .program
            .implementations
            .iter()
            .filter(|implementation| {
                implementation
                    .trait_name
                    .as_ref()
                    .is_some_and(|trait_name| trait_name.name == "Copy")
            })
        {
            let declaration_generics = self
                .program
                .structs
                .iter()
                .find(|declaration| declaration.name == implementation.target.name)
                .map(|declaration| &declaration.generics)
                .or_else(|| {
                    self.program
                        .enums
                        .iter()
                        .find(|declaration| declaration.name == implementation.target.name)
                        .map(|declaration| &declaration.generics)
                })
                .unwrap();
            let implementation_constraints = implementation
                .generics
                .iter()
                .map(|parameter| {
                    (
                        parameter.name.as_str(),
                        parameter
                            .constraints
                            .iter()
                            .map(|constraint| constraint.name.as_str())
                            .collect::<HashSet<_>>(),
                    )
                })
                .collect::<HashMap<_, _>>();
            let mut used = HashSet::new();
            let universal = implementation.target.arguments.len() == declaration_generics.len()
                && implementation.generics.len() == declaration_generics.len()
                && implementation
                    .target
                    .arguments
                    .iter()
                    .zip(declaration_generics)
                    .all(|(argument, declaration_generic)| {
                        let expected = declaration_generic
                            .constraints
                            .iter()
                            .map(|constraint| constraint.name.as_str())
                            .collect::<HashSet<_>>();
                        argument.qualifier == TypeQualifier::Owned
                            && argument.arguments.is_empty()
                            && used.insert(argument.name.as_str())
                            && implementation_constraints
                                .get(argument.name.as_str())
                                .is_some_and(|actual| *actual == expected)
                    });
            if !universal {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "`Copy` implementation for `{}` must cover every permitted instantiation",
                        implementation.target.name
                    ),
                    implementation.span,
                )
                .with_help(
                    "use one implementation generic per aggregate generic and mirror its declaration constraints",
                ));
            }
        }
        for declaration in &self.program.structs {
            if self.copy_types.contains(&declaration.name) {
                let generic_copy = declaration
                    .generics
                    .iter()
                    .filter(|parameter| parameter.constraints.iter().any(|c| c.name == "Copy"))
                    .map(|parameter| parameter.name.clone())
                    .collect::<HashSet<_>>();
                for field in &declaration.fields {
                    if !self.type_name_is_copy(&field.ty, &generic_copy) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "`{}` cannot implement `Copy` because field `{}` is not Copy",
                                declaration.name, field.name
                            ),
                            field.name_span,
                        ));
                    }
                }
            }
        }
        for declaration in &self.program.enums {
            if self.copy_types.contains(&declaration.name) {
                let generic_copy = declaration
                    .generics
                    .iter()
                    .filter(|parameter| parameter.constraints.iter().any(|c| c.name == "Copy"))
                    .map(|parameter| parameter.name.clone())
                    .collect::<HashSet<_>>();
                for variant in &declaration.variants {
                    for payload in &variant.payload {
                        if !self.type_name_is_copy(payload, &generic_copy) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "`{}` cannot implement `Copy` because variant `{}` owns a non-Copy payload",
                                    declaration.name, variant.name
                                ),
                                variant.name_span,
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn check_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        self.slots.clear();
        self.scopes.clear();
        self.loans.clear();
        self.generic_copy = function
            .generics
            .iter()
            .filter(|parameter| parameter.constraints.iter().any(|c| c.name == "Copy"))
            .map(|parameter| parameter.name.clone())
            .collect();
        self.generic_traits = function
            .generics
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
        self.push_scope();
        self.return_has_reference = function
            .return_type
            .as_ref()
            .is_some_and(|ty| self.type_name_contains_reference(ty));
        if self.return_has_reference
            && function
                .parameters
                .iter()
                .filter(|parameter| ty_is_borrowed_view(&self.ty_from_name(&parameter.ty)))
                .count()
                > 1
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "a borrowed return requires exactly one borrowed input lifetime",
                function
                    .return_type
                    .as_ref()
                    .map_or(function.name_span, |ty| ty.span),
            )
            .with_help("return owned data, or redesign the function with one borrowed input until explicit lifetime parameters are available"));
        }
        for parameter in &function.parameters {
            let ty = self.ty_from_name(&parameter.ty);
            let id = self.declare(
                &parameter.name,
                ty.clone(),
                parameter.name_span,
                false,
                true,
                None,
            )?;
            let slot = self.slots.get_mut(&id).unwrap();
            slot.parameter = true;
            if matches!(&ty, Ty::Slice(_) | Ty::Str | Ty::CStr) {
                let origin = Place {
                    root: id,
                    fields: vec![],
                };
                slot.reference_origin = Some(origin.clone());
                slot.borrow_origins.push((origin, false));
            } else if let Ty::Reference(_, mutable)
            | Ty::RawPointer(_, mutable)
            | Ty::MemoryPointer(_, mutable) = &ty
            {
                let origin = Place {
                    root: id,
                    fields: vec![],
                };
                slot.borrow_origins.push((origin, *mutable));
            }
            if matches!(&ty, Ty::Function) {
                // A callable parameter may carry captures chosen by its caller.
                // Keep that hidden lifetime symbolic so it cannot be smuggled
                // into a return value or longer-lived aggregate.
                slot.closure_origins.push((
                    Place {
                        root: id,
                        fields: vec![],
                    },
                    false,
                ));
            }
        }
        self.check_block_contents(&function.body)?;
        self.pop_scope(function.body.span, DropReason::ScopeEnd);
        Ok(())
    }

    fn check_block(&mut self, block: &Block) -> Result<(), Diagnostic> {
        self.push_scope();
        let result = self.check_block_contents(block);
        self.pop_scope(block.span, DropReason::ScopeEnd);
        result
    }

    fn check_block_contents(&mut self, block: &Block) -> Result<(), Diagnostic> {
        let last_uses = block_last_uses(block);
        for (index, statement) in block.statements.iter().enumerate() {
            self.expire_loans(index, &last_uses);
            self.check_statement(&statement.node, statement.span, index, &last_uses)?;
        }
        Ok(())
    }

    fn check_statement(
        &mut self,
        statement: &Statement,
        span: Span,
        index: usize,
        last_uses: &HashMap<String, usize>,
    ) -> Result<(), Diagnostic> {
        match statement {
            Statement::Binding {
                kind,
                name,
                name_span,
                annotation,
                value,
            } => {
                if let Some(Expr {
                    node: Expression::Borrow { mutable, target },
                    ..
                }) = value
                {
                    let place = self.place(target)?;
                    self.check_borrow(&place, *mutable, target.span)?;
                    let inner = self.place_ty(&place)?;
                    let ty = annotation
                        .as_ref()
                        .map(|annotation| self.ty_from_name(annotation))
                        .unwrap_or_else(|| Ty::Reference(Box::new(inner), *mutable));
                    let id = self.declare(
                        name,
                        ty,
                        *name_span,
                        *kind == BindingKind::Var,
                        true,
                        Some(place.clone()),
                    )?;
                    self.loans.push(Loan {
                        place,
                        mutable: *mutable,
                        borrower: Some(id),
                        at: target.span,
                    });
                    if last_uses.get(name).copied().unwrap_or(index) == index {
                        self.loans.retain(|loan| loan.borrower != Some(id));
                    }
                    return Ok(());
                }
                if let Some(Expr {
                    node: Expression::Call { callee, arguments },
                    ..
                }) = value
                    && let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(field.as_str(), "get" | "get_mut")
                    && arguments.len() == 1
                    && matches!(self.expr_ty(object), Ok(Ty::List(_) | Ty::Map(_, _)))
                {
                    let mutable = field == "get_mut";
                    let mut place = self.place(object)?;
                    self.use_place(&place, UseMode::Read, object.span)?;
                    self.check_expr(&arguments[0], UseMode::Read)?;
                    let map = matches!(self.expr_ty(object), Ok(Ty::Map(_, _)));
                    place.fields.push(if map {
                        "@k:*".into()
                    } else {
                        match arguments[0].node {
                            Expression::Integer(value) => format!("@i:{value}"),
                            _ => "@i:*".into(),
                        }
                    });
                    self.check_borrow(&place, mutable, object.span)?;
                    let inner = self.place_ty(&place)?;
                    let ty = annotation
                        .as_ref()
                        .map(|annotation| self.ty_from_name(annotation))
                        .unwrap_or_else(|| {
                            Ty::Option(Box::new(Ty::Reference(Box::new(inner), mutable)))
                        });
                    let id = self.declare(
                        name,
                        ty,
                        *name_span,
                        *kind == BindingKind::Var,
                        true,
                        Some(place.clone()),
                    )?;
                    self.loans.push(Loan {
                        place,
                        mutable,
                        borrower: Some(id),
                        at: object.span,
                    });
                    if last_uses.get(name).copied().unwrap_or(index) == index {
                        self.loans.retain(|loan| loan.borrower != Some(id));
                    }
                    return Ok(());
                }
                if let Some(Expr {
                    node: Expression::Call { callee, arguments },
                    ..
                }) = value
                    && let Expression::FieldAccess { object, field, .. } = &callee.node
                    && field == "as_c_str"
                    && arguments.is_empty()
                    && matches!(self.expr_ty(object), Ok(Ty::CString))
                {
                    let place = self.place(object)?;
                    self.use_place(&place, UseMode::Read, object.span)?;
                    self.check_borrow(&place, false, object.span)?;
                    let ty = annotation
                        .as_ref()
                        .map(|annotation| self.ty_from_name(annotation))
                        .unwrap_or(Ty::CStr);
                    let id = self.declare(
                        name,
                        ty,
                        *name_span,
                        *kind == BindingKind::Var,
                        true,
                        Some(place.clone()),
                    )?;
                    self.attach_borrow_origins(id, vec![(place, false)], object.span)?;
                    if last_uses.get(name).copied().unwrap_or(index) == index {
                        self.loans.retain(|loan| loan.borrower != Some(id));
                    }
                    return Ok(());
                }
                let closure_origins = value
                    .as_ref()
                    .map(|value| self.closure_origins(value))
                    .unwrap_or_default();
                let borrow_origins = value
                    .as_ref()
                    .map(|value| self.borrow_origins(value))
                    .unwrap_or_default();
                let (ty, origin) = if let Some(value) = value {
                    let ty = self.check_expr(value, UseMode::Consume)?;
                    (
                        annotation
                            .as_ref()
                            .map(|a| self.ty_from_name(a))
                            .unwrap_or(ty),
                        borrow_origins.first().map(|(place, _)| place.clone()),
                    )
                } else {
                    (
                        self.ty_from_name(annotation.as_ref().expect("parser requires annotation")),
                        None,
                    )
                };
                let id = self.declare(
                    name,
                    ty.clone(),
                    *name_span,
                    *kind == BindingKind::Var,
                    value.is_some(),
                    origin.clone(),
                )?;
                self.attach_closure_origins(id, closure_origins, *name_span);
                if !borrow_origins.is_empty() {
                    self.attach_borrow_origins(id, borrow_origins, *name_span)?;
                    if last_uses.get(name).copied().unwrap_or(index) == index {
                        self.loans.retain(|loan| loan.borrower != Some(id));
                    }
                }
            }
            Statement::Assignment {
                name,
                name_span,
                value,
                operator,
            } => {
                let Some(id) = self.lookup(name) else {
                    if *operator != crate::ast::AssignmentOperator::Assign {
                        return Err(self.error_unknown(name, *name_span));
                    }
                    if let Expression::Call { callee, arguments } = &value.node
                        && let Expression::FieldAccess { object, field, .. } = &callee.node
                        && field == "as_c_str"
                        && arguments.is_empty()
                        && matches!(self.expr_ty(object), Ok(Ty::CString))
                    {
                        let place = self.place(object)?;
                        self.use_place(&place, UseMode::Read, object.span)?;
                        self.check_borrow(&place, false, object.span)?;
                        let id = self.declare(
                            name,
                            Ty::CStr,
                            *name_span,
                            true,
                            true,
                            Some(place.clone()),
                        )?;
                        self.attach_borrow_origins(id, vec![(place, false)], object.span)?;
                        if last_uses.get(name).copied().unwrap_or(index) == index {
                            self.loans.retain(|loan| loan.borrower != Some(id));
                        }
                        return Ok(());
                    }
                    let closure_origins = self.closure_origins(value);
                    let borrow_origins = self.borrow_origins(value);
                    let ty = self.check_expr(value, UseMode::Consume)?;
                    let origin = borrow_origins.first().map(|(place, _)| place.clone());
                    let id =
                        self.declare(name, ty.clone(), *name_span, true, true, origin.clone())?;
                    self.attach_closure_origins(id, closure_origins, *name_span);
                    if !borrow_origins.is_empty() {
                        self.attach_borrow_origins(id, borrow_origins, *name_span)?;
                        if last_uses.get(name).copied().unwrap_or(index) == index {
                            self.loans.retain(|loan| loan.borrower != Some(id));
                        }
                    }
                    return Ok(());
                };
                let place = Place {
                    root: id,
                    fields: vec![],
                };
                let closure_origins = self.closure_origins(value);
                self.check_assignment(&place, value, span)?;
                self.loans.retain(|loan| loan.borrower != Some(id));
                self.slots.get_mut(&id).unwrap().closure_origins.clear();
                self.attach_closure_origins(id, closure_origins, *name_span);
            }
            Statement::PlaceAssignment { target, value, .. } => {
                let place = self.place(target)?;
                self.check_assignment(&place, value, span)?;
            }
            Statement::Expression(expression) => {
                self.check_expr(expression, UseMode::Read)?;
            }
            Statement::Return(value) => {
                if let Some(value) = value {
                    let closure_origins = self.closure_origins(value);
                    let borrow_origins = self.borrow_origins(value);
                    if let Expression::Identifier(name) = &value.node
                        && self.lookup(name).is_some_and(|id| {
                            self.slots[&id].parameter && matches!(self.slots[&id].ty, Ty::Function)
                        })
                    {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "returning a callable parameter could hide borrowed captures",
                            value.span,
                        )
                        .with_help(
                            "return a newly created `move` closure or an owned data value",
                        ));
                    }
                    if let Some((origin, _)) = closure_origins.first() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "closure borrowing `{}` cannot escape its owner's scope",
                                self.slots[&origin.root].name
                            ),
                            value.span,
                        )
                        .with_help("return a `move` closure that owns every captured value"));
                    }
                    let ty = self.check_expr(value, UseMode::Consume)?;
                    if self.return_has_reference
                        || self.ty_contains_reference(&ty)
                        || !borrow_origins.is_empty()
                    {
                        let roots = borrow_origins
                            .iter()
                            .map(|(origin, _)| origin.root)
                            .collect::<HashSet<_>>();
                        let borrowed_reference_parameter = roots.len() == 1
                            && roots.iter().all(|root| {
                                self.slots[root].parameter
                                    && ty_is_borrowed_view(&self.slots[root].ty)
                            });
                        if !borrowed_reference_parameter {
                            let local = borrow_origins
                                .first()
                                .map(|(origin, _)| self.slots[&origin.root].name.clone());
                            let message = local.map_or_else(
                                || "returned reference has no provable live origin".to_string(),
                                |name| format!("cannot return a reference to local `{name}`"),
                            );
                            return Err(Diagnostic::new(DiagnosticKind::Type, message, value.span)
                                .with_help(
                                    "return an owned value or borrow from a reference parameter",
                                ));
                        }
                    }
                }
                self.record_live_drops(span, DropReason::Return);
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.check_expr(condition, UseMode::Read)?;
                let before = self.clone();
                let mut then_state = before.clone();
                then_state.check_block(then_branch)?;
                let mut else_state = before.clone();
                if let Some(else_branch) = else_branch {
                    else_state.check_block(else_branch)?;
                }
                self.merge_from(&then_state, &else_state);
                self.report.drops.extend(
                    then_state
                        .report
                        .drops
                        .into_iter()
                        .skip(before.report.drops.len()),
                );
                self.report.drops.extend(
                    else_state
                        .report
                        .drops
                        .into_iter()
                        .skip(before.report.drops.len()),
                );
            }
            Statement::While { condition, body } => {
                self.check_expr(condition, UseMode::Read)?;
                let before = self.clone();
                let mut body_state = before.clone();
                body_state.check_block(body)?;
                let mut repeated_entry = before.clone();
                repeated_entry.merge_loop(&before, &body_state);
                repeated_entry.check_expr(condition, UseMode::Read)?;
                let mut repeated_body = repeated_entry.clone();
                repeated_body.check_block(body)?;
                self.merge_loop(&before, &repeated_body);
                self.report.drops.extend(
                    repeated_body
                        .report
                        .drops
                        .into_iter()
                        .skip(before.report.drops.len()),
                );
            }
            Statement::For {
                name,
                name_span,
                start,
                end,
                body,
                ..
            } => {
                self.check_expr(start, UseMode::Read)?;
                self.check_expr(end, UseMode::Read)?;
                let before = self.clone();
                let mut body_state = before.clone();
                body_state.push_scope();
                body_state.declare(name, Ty::Copy, *name_span, false, true, None)?;
                body_state.check_block_contents(body)?;
                body_state.pop_scope(body.span, DropReason::ScopeEnd);
                let mut repeated_entry = before.clone();
                repeated_entry.merge_loop(&before, &body_state);
                let mut repeated_body = repeated_entry.clone();
                repeated_body.push_scope();
                repeated_body.declare(name, Ty::Copy, *name_span, false, true, None)?;
                repeated_body.check_block_contents(body)?;
                repeated_body.pop_scope(body.span, DropReason::ScopeEnd);
                self.merge_loop(&before, &repeated_body);
                self.report.drops.extend(
                    repeated_body
                        .report
                        .drops
                        .into_iter()
                        .skip(before.report.drops.len()),
                );
            }
            Statement::ForEach {
                name,
                name_span,
                iterable,
                body,
            } => {
                let iterable_ty = self.check_expr(iterable, UseMode::Read)?;
                let element = match iterable_ty {
                    Ty::Array(element)
                    | Ty::Slice(element)
                    | Ty::List(element)
                    | Ty::Set(element) => *element,
                    Ty::Reference(inner, _) => match *inner {
                        Ty::Array(element)
                        | Ty::Slice(element)
                        | Ty::List(element)
                        | Ty::Set(element) => *element,
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "iteration requires an array, slice, or List",
                                iterable.span,
                            ));
                        }
                    },
                    _ => {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "iteration requires an array, slice, or List",
                            iterable.span,
                        ));
                    }
                };
                let origin = self.place(iterable).ok().or_else(|| {
                    if let Expression::Call { callee, .. } = &iterable.node
                        && let Expression::FieldAccess { object, field, .. } = &callee.node
                        && matches!(field.as_str(), "iter" | "keys" | "values")
                    {
                        self.place(object).ok()
                    } else {
                        None
                    }
                });
                let before = self.clone();
                let mut body_state = before.clone();
                body_state.push_scope();
                let item_ty = if body_state.ty_is_copy(&element) {
                    element.clone()
                } else {
                    Ty::Reference(Box::new(element.clone()), false)
                };
                let item =
                    body_state.declare(name, item_ty, *name_span, false, true, origin.clone())?;
                if let Some(place) = origin.clone() {
                    body_state.loans.push(Loan {
                        place,
                        mutable: false,
                        borrower: Some(item),
                        at: iterable.span,
                    });
                }
                body_state.check_block_contents(body)?;
                body_state.pop_scope(body.span, DropReason::ScopeEnd);
                let mut repeated_entry = before.clone();
                repeated_entry.merge_loop(&before, &body_state);
                let mut repeated_body = repeated_entry.clone();
                repeated_body.push_scope();
                let item_ty = if repeated_body.ty_is_copy(&element) {
                    element
                } else {
                    Ty::Reference(Box::new(element), false)
                };
                let item = repeated_body.declare(
                    name,
                    item_ty,
                    *name_span,
                    false,
                    true,
                    origin.clone(),
                )?;
                if let Some(place) = origin {
                    repeated_body.loans.push(Loan {
                        place,
                        mutable: false,
                        borrower: Some(item),
                        at: iterable.span,
                    });
                }
                repeated_body.check_block_contents(body)?;
                repeated_body.pop_scope(body.span, DropReason::ScopeEnd);
                self.merge_loop(&before, &repeated_body);
                self.report.drops.extend(
                    repeated_body
                        .report
                        .drops
                        .into_iter()
                        .skip(before.report.drops.len()),
                );
            }
            Statement::Loop(body) => {
                let before = self.clone();
                let mut body_state = before.clone();
                body_state.check_block(body)?;
                let mut repeated_entry = before.clone();
                repeated_entry.merge_loop(&before, &body_state);
                let mut repeated_body = repeated_entry.clone();
                repeated_body.check_block(body)?;
                self.merge_loop(&before, &repeated_body);
                self.report.drops.extend(
                    repeated_body
                        .report
                        .drops
                        .into_iter()
                        .skip(before.report.drops.len()),
                );
            }
            Statement::Unsafe { body, .. } => self.check_block(body)?,
            Statement::Break => self.record_live_drops(span, DropReason::Break),
            Statement::Continue => self.record_live_drops(span, DropReason::Continue),
        }
        Ok(())
    }

    fn check_assignment(
        &mut self,
        place: &Place,
        value: &Expr,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let slot = self
            .slots
            .get(&place.root)
            .cloned()
            .expect("resolved place");
        let through_mutable_reference =
            place.fields.first().is_some_and(|field| field == "<deref>")
                && matches!(slot.ty, Ty::Reference(_, true) | Ty::MutexGuard(_));
        if !slot.mutable
            && !through_mutable_reference
            && matches!(
                slot.state,
                InitState::Initialized | InitState::Partial { .. }
            )
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("cannot assign to immutable `{}`", slot.name),
                span,
            ));
        }
        self.ensure_no_conflicting_loan(place, true, span)?;
        let value_origins = self.borrow_origins(value);
        self.check_expr(value, UseMode::Consume)?;
        for (origin, _) in &value_origins {
            let target_depth = self.slots[&place.root].scope_depth;
            let origin_depth = self.slots[&origin.root].scope_depth;
            if target_depth < origin_depth {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "reference to `{}` escapes its owner's scope",
                        self.slots[&origin.root].name
                    ),
                    span,
                ));
            }
        }
        let whole_assignment = place.fields.is_empty();
        let mut carried_origins = if whole_assignment {
            Vec::new()
        } else {
            self.slots[&place.root].borrow_origins.clone()
        };
        let adds_origins = !value_origins.is_empty();
        carried_origins.extend(value_origins);
        let slot = self.slots.get_mut(&place.root).unwrap();
        if place.fields.is_empty() {
            slot.state = InitState::Initialized;
        } else if let InitState::Partial { fields } = &mut slot.state {
            fields.remove(&place.fields[0]);
            if fields.is_empty() {
                slot.state = InitState::Initialized;
            }
        }
        if whole_assignment || adds_origins {
            self.loans.retain(|loan| loan.borrower != Some(place.root));
            if carried_origins.is_empty() {
                let slot = self.slots.get_mut(&place.root).unwrap();
                slot.reference_origin = None;
                slot.borrow_origins.clear();
            } else {
                self.attach_borrow_origins(place.root, carried_origins, span)?;
            }
        }
        Ok(())
    }

    fn check_expr(&mut self, expression: &Expr, mode: UseMode) -> Result<Ty, Diagnostic> {
        match &expression.node {
            Expression::Array(values) => {
                let mut element = Ty::Unit;
                for value in values {
                    element = self.check_expr(value, UseMode::Consume)?;
                }
                Ok(Ty::Array(Box::new(element)))
            }
            Expression::DataWrite { value, store, .. } => {
                self.check_expr(value, UseMode::Read)?;
                self.check_data_store(store)?;
                Ok(Ty::Result(
                    Box::new(Ty::Copy),
                    Box::new(Ty::Owned("DataError".into())),
                ))
            }
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    self.check_expr(path, UseMode::Read)?;
                }
                Ok(Ty::Result(
                    Box::new(Ty::Owned("DataStore".into())),
                    Box::new(Ty::Owned("DataError".into())),
                ))
            }
            Expression::DataQuery {
                kind,
                schema,
                aggregate,
                store,
                predicate,
                order,
                limit,
                ..
            } => {
                self.check_data_store(store)?;
                if let Some(aggregate) = aggregate {
                    self.check_data_expression(schema, aggregate)?;
                }
                if let Some(predicate) = predicate {
                    self.check_data_expression(schema, predicate)?;
                }
                if let Some(order) = order {
                    self.check_data_expression(schema, &order.key)?;
                }
                if let Some(limit) = limit {
                    self.check_expr(limit, UseMode::Read)?;
                }
                let value = match kind {
                    DataQueryKind::Rows => Ty::List(Box::new(Ty::Owned(schema.clone()))),
                    DataQueryKind::Count
                    | DataQueryKind::Exists
                    | DataQueryKind::Sum
                    | DataQueryKind::Average
                    | DataQueryKind::Min
                    | DataQueryKind::Max => Ty::Copy,
                };
                Ok(Ty::Result(
                    Box::new(value),
                    Box::new(Ty::Owned("DataError".into())),
                ))
            }
            Expression::DataRemove {
                schema,
                store,
                predicate,
                ..
            } => {
                self.check_data_store(store)?;
                self.check_data_expression(schema, predicate)?;
                Ok(Ty::Result(
                    Box::new(Ty::Copy),
                    Box::new(Ty::Owned("DataError".into())),
                ))
            }
            Expression::Closure {
                move_captures,
                parameters,
                body,
                ..
            } => {
                let captures = crate::ast::closure_capture_uses(parameters, body);
                let mut nested = self.clone();
                for (name, usage) in &captures {
                    let Some(root) = self.lookup(name) else {
                        continue;
                    };
                    let place = Place {
                        root,
                        fields: vec![],
                    };
                    let ty = self.slots[&root].ty.clone();
                    if usage.consumed && !self.ty_is_copy(&ty) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "cannot move captured `{name}` out of a reusable closure"
                            ),
                            usage.span,
                        )
                        .with_help(
                            "return or pass a borrow, or move a separately owned value into the called operation",
                        ));
                    }
                    if *move_captures {
                        self.use_place(&place, UseMode::Consume, usage.span)?;
                    } else {
                        self.check_borrow(&place, usage.mutated, usage.span)?;
                        self.loans.push(Loan {
                            place,
                            mutable: usage.mutated,
                            borrower: None,
                            at: usage.span,
                        });
                    }
                }
                nested.push_scope();
                for parameter in parameters {
                    let parameter_ty = nested.ty_from_name(&parameter.ty);
                    let id = nested.declare(
                        &parameter.name,
                        parameter_ty.clone(),
                        parameter.name_span,
                        false,
                        true,
                        None,
                    )?;
                    if matches!(parameter_ty, Ty::Function) {
                        nested.slots.get_mut(&id).unwrap().closure_origins.push((
                            Place {
                                root: id,
                                fields: vec![],
                            },
                            false,
                        ));
                    }
                }
                match body {
                    crate::ast::ClosureBody::Expression(value) => {
                        if let Some((origin, _)) = nested.closure_origins(value).first() {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "closure borrowing `{}` cannot escape through another closure",
                                    nested.slots[&origin.root].name
                                ),
                                value.span,
                            )
                            .with_help("return a `move` closure that owns every captured value"));
                        }
                        nested.check_expr(value, UseMode::Consume)?;
                    }
                    crate::ast::ClosureBody::Block(block) => {
                        nested.check_block_contents(block)?;
                    }
                }
                nested.pop_scope(expression.span, DropReason::ScopeEnd);
                Ok(Ty::Function)
            }
            Expression::Index { object: _, index } => {
                self.check_expr(index, UseMode::Read)?;
                let place = self.place(expression)?;
                let ty = self.place_ty(&place)?;
                if matches!(mode, UseMode::Consume) && !self.ty_is_copy(&ty) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "cannot move a non-Copy element through dynamic indexing",
                        expression.span,
                    )
                    .with_help(
                        "borrow the element or remove it through an owning collection API",
                    ));
                }
                self.use_place(&place, UseMode::Read, expression.span)?;
                Ok(ty)
            }
            Expression::Subslice { object, start, end } => {
                self.check_expr(start, UseMode::Read)?;
                self.check_expr(end, UseMode::Read)?;
                let base = self.check_expr(object, UseMode::Read)?;
                Ok(match base {
                    Ty::Array(element) | Ty::Slice(element) | Ty::List(element) => {
                        Ty::Slice(element)
                    }
                    Ty::Owned(name) if name == "String" => Ty::Str,
                    Ty::Str => Ty::Str,
                    _ => Ty::Owned("Slice".into()),
                })
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::Character(_)
            | Expression::Bool(_) => Ok(Ty::Copy),
            Expression::String(_) => Ok(Ty::Owned("String".into())),
            Expression::Identifier(name) => {
                let Some(id) = self.lookup(name) else {
                    if name == "None" {
                        return Ok(Ty::Option(Box::new(Ty::Owned("inferred".into()))));
                    }
                    if matches!(name.as_str(), "Some" | "Ok" | "Err")
                        || is_numeric_type_name(name)
                        || self
                            .program
                            .functions
                            .iter()
                            .any(|function| function.name == *name)
                        || self
                            .program
                            .enums
                            .iter()
                            .any(|owner| owner.variants.iter().any(|variant| variant.name == *name))
                    {
                        return Ok(Ty::Owned("callable".into()));
                    }
                    return Err(self.error_unknown(name, expression.span));
                };
                self.use_place(
                    &Place {
                        root: id,
                        fields: vec![],
                    },
                    mode,
                    expression.span,
                )
            }
            Expression::Move(target) => {
                let place = self.place(target)?;
                self.use_place(&place, UseMode::Consume, expression.span)
            }
            Expression::Borrow { mutable, target } => {
                let place = self.place(target)?;
                self.check_borrow(&place, *mutable, expression.span)?;
                let ty = self.place_ty(&place)?;
                self.loans.push(Loan {
                    place,
                    mutable: *mutable,
                    borrower: None,
                    at: expression.span,
                });
                Ok(Ty::Reference(Box::new(ty), *mutable))
            }
            Expression::Dereference(target) => match self.check_expr(target, UseMode::Read)? {
                Ty::Reference(inner, _)
                | Ty::RawPointer(inner, _)
                | Ty::MemoryPointer(inner, _)
                | Ty::MutexGuard(inner) => Ok(*inner),
                _ => Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "cannot dereference this value",
                    expression.span,
                )),
            },
            Expression::StructConstruct { name, fields, .. } => {
                let actual_fields = fields
                    .iter()
                    .map(|field| {
                        self.check_expr(&field.value, UseMode::Consume)
                            .map(|ty| (field.name.as_str(), ty))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let components = self
                    .program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name)
                    .map(|declaration| {
                        declaration
                            .fields
                            .iter()
                            .filter_map(|field| {
                                actual_fields
                                    .iter()
                                    .find(|(actual_name, _)| *actual_name == field.name)
                                    .map(|(_, actual)| (field.ty.clone(), actual.clone()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(self.instantiated_nominal_ty(name, &components))
            }
            Expression::FieldAccess { object, field, .. } => {
                if matches!(&object.node, Expression::Identifier(name) if self.program.enums.iter().any(|declaration| declaration.name == *name))
                {
                    return Ok(Ty::Owned(match &object.node {
                        Expression::Identifier(name) => name.clone(),
                        _ => unreachable!(),
                    }));
                }
                let place = self.place(expression)?;
                let _ = field;
                self.use_place(&place, mode, expression.span)
            }
            Expression::Unary { operand, .. } => {
                self.check_expr(operand, UseMode::Read)?;
                Ok(Ty::Copy)
            }
            Expression::Binary { left, right, .. } => {
                self.check_expr(left, UseMode::Read)?;
                self.check_expr(right, UseMode::Read)?;
                Ok(Ty::Copy)
            }
            Expression::Call { callee, arguments } => {
                self.check_call(callee, arguments, expression.span)
            }
            Expression::Spawn(task) => {
                let Expression::Call { callee, arguments } = &task.node else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`spawn` requires a direct function call",
                        task.span,
                    ));
                };
                Ok(Ty::Thread(Box::new(
                    self.check_call(callee, arguments, task.span)?,
                )))
            }
            Expression::Await(future) => match self.check_expr(future, UseMode::Consume)? {
                Ty::Future(output) | Ty::Task(output) => Ok(*output),
                _ => Ok(Ty::Owned("future-output".into())),
            },
            Expression::Match { value, arms } => {
                let matched_origins = self.borrow_origins(value);
                let matched = self.check_expr(value, UseMode::Consume)?;
                let before = self.clone();
                let mut branch_states = Vec::new();
                let mut result = Ty::Unit;
                for arm in arms {
                    let mut arm_state = before.clone();
                    arm_state.push_scope();
                    arm_state.bind_pattern(
                        &arm.pattern.node,
                        &matched,
                        &matched_origins,
                        arm.pattern.span,
                    )?;
                    if let Some(guard) = &arm.guard {
                        let loan_start = arm_state.loans.len();
                        arm_state
                            .loans
                            .extend(arm_state.slots.keys().copied().map(|root| Loan {
                                place: Place {
                                    root,
                                    fields: vec![],
                                },
                                mutable: false,
                                borrower: None,
                                at: guard.span,
                            }));
                        let guard_result = arm_state.check_expr(guard, UseMode::Read);
                        arm_state.loans.truncate(loan_start);
                        guard_result?;
                    }
                    result = arm_state.check_expr(&arm.value, mode)?;
                    arm_state.pop_scope(arm.span, DropReason::ScopeEnd);
                    branch_states.push(arm_state);
                }
                if let Some(first) = branch_states.first().cloned() {
                    let mut merged = first;
                    for branch in branch_states.iter().skip(1) {
                        merged.merge_from(&merged.clone(), branch);
                    }
                    self.merge_from(&before, &merged);
                }
                Ok(result)
            }
            Expression::Try(operand) => {
                let ty = self.check_expr(operand, UseMode::Consume)?;
                self.record_live_drops(expression.span, DropReason::Propagation);
                Ok(match ty {
                    Ty::Option(value) => *value,
                    Ty::Result(value, _) => *value,
                    _ => Ty::Owned("try-output".into()),
                })
            }
        }
    }

    fn check_data_store(&mut self, store: &Expr) -> Result<(), Diagnostic> {
        if let Ok(place) = self.place(store) {
            self.check_borrow(&place, true, store.span)?;
            self.use_place(&place, UseMode::Read, store.span)?;
        } else {
            self.check_expr(store, UseMode::Read)?;
        }
        Ok(())
    }

    fn check_data_expression(&mut self, schema: &str, expression: &Expr) -> Result<(), Diagnostic> {
        match &expression.node {
            Expression::Identifier(name)
                if self.lookup(name).is_none()
                    && self
                        .program
                        .structs
                        .iter()
                        .find(|item| item.name == schema && item.data)
                        .is_some_and(|item| {
                            item.fields.iter().any(|field| field.name == *name)
                        }) =>
            {
                Ok(())
            }
            Expression::Unary { operand, .. } => self.check_data_expression(schema, operand),
            Expression::Binary { left, right, .. } => {
                self.check_data_expression(schema, left)?;
                self.check_data_expression(schema, right)
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => Ok(()),
            Expression::Identifier(_) => {
                self.check_expr(expression, UseMode::Read)?;
                Ok(())
            }
            _ => Err(Diagnostic::new(
                DiagnosticKind::Type,
                "unsupported expression in a DISP Data plan",
                expression.span,
            )),
        }
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        arguments: &[Expr],
        span: Span,
    ) -> Result<Ty, Diagnostic> {
        let temporary_start = self.loans.len();
        if let Expression::Identifier(name) = &callee.node {
            if name == "String" {
                return Ok(Ty::Owned("String".into()));
            }
            if name == "Path" {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Path);
            }
            if name == "Url" {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Result(
                    Box::new(Ty::Url),
                    Box::new(Ty::Owned("NetworkError".into())),
                ));
            }
            if name == "Json" {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Result(
                    Box::new(Ty::Json),
                    Box::new(Ty::Owned("ConversionError".into())),
                ));
            }
            if name == "SocketAddress" {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::SocketAddress);
            }
            if name == "print" {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                self.loans.truncate(temporary_start);
                return Ok(Ty::Unit);
            }
            if let Some(function) = self
                .program
                .functions
                .iter()
                .find(|function| function.name == *name)
            {
                for (argument, parameter) in arguments.iter().zip(&function.parameters) {
                    if parameter.ty.qualifier == TypeQualifier::SharedReference
                        && !matches!(self.expr_ty(argument), Ok(Ty::Reference(_, _)))
                    {
                        let place = self.place(argument)?;
                        self.check_borrow(&place, false, argument.span)?;
                        self.use_place(&place, UseMode::Read, argument.span)?;
                        self.loans.push(Loan {
                            place,
                            mutable: false,
                            borrower: None,
                            at: argument.span,
                        });
                    } else {
                        self.check_expr(
                            argument,
                            if parameter.ty.qualifier == TypeQualifier::Owned {
                                UseMode::Consume
                            } else {
                                UseMode::Read
                            },
                        )?;
                    }
                }
                self.loans.truncate(temporary_start);
                let result = function
                    .return_type
                    .as_ref()
                    .map(|ty| self.ty_from_name(ty))
                    .unwrap_or(Ty::Unit);
                return Ok(if function.asynchronous {
                    Ty::Future(Box::new(result))
                } else {
                    result
                });
            }
            if matches!(name.as_str(), "Some" | "Ok" | "Err") {
                let payload = arguments
                    .first()
                    .map(|argument| self.check_expr(argument, UseMode::Consume))
                    .transpose()?
                    .unwrap_or_else(|| Ty::Owned("inferred".into()));
                self.loans.truncate(temporary_start);
                return Ok(match name.as_str() {
                    "Some" => Ty::Option(Box::new(payload)),
                    "Ok" => Ty::Result(Box::new(payload), Box::new(Ty::Owned("inferred".into()))),
                    "Err" => Ty::Result(Box::new(Ty::Owned("inferred".into())), Box::new(payload)),
                    _ => unreachable!(),
                });
            }
        }
        if let Expression::FieldAccess { object, field, .. } = &callee.node {
            if matches!(&object.node, Expression::Identifier(name) if name == "Async") {
                return match field.as_str() {
                    "yield" => Ok(Ty::Future(Box::new(Ty::Unit))),
                    "spawn" => match self.check_expr(&arguments[0], UseMode::Consume)? {
                        Ty::Future(output) => Ok(Ty::Task(output)),
                        _ => Ok(Ty::Task(Box::new(Ty::Owned("task-output".into())))),
                    },
                    "sleep" => {
                        self.check_expr(&arguments[0], UseMode::Read)?;
                        Ok(Ty::Future(Box::new(Ty::Unit)))
                    }
                    "connect" => {
                        self.check_expr(&arguments[0], UseMode::Consume)?;
                        Ok(Ty::Future(Box::new(Ty::Result(
                            Box::new(Ty::TcpStream),
                            Box::new(Ty::Owned("NetworkError".into())),
                        ))))
                    }
                    "connect_timeout" => {
                        self.check_expr(&arguments[0], UseMode::Consume)?;
                        self.check_expr(&arguments[1], UseMode::Read)?;
                        Ok(Ty::Future(Box::new(Ty::Result(
                            Box::new(Ty::TcpStream),
                            Box::new(Ty::Owned("NetworkError".into())),
                        ))))
                    }
                    "resolve" | "resolve_timeout" => {
                        self.check_expr(&arguments[0], UseMode::Read)?;
                        if field == "resolve_timeout" {
                            self.check_expr(&arguments[1], UseMode::Read)?;
                        }
                        Ok(Ty::Future(Box::new(Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::IpAddress))),
                            Box::new(Ty::Owned("NetworkError".into())),
                        ))))
                    }
                    "read_text" | "read_bytes" => {
                        self.check_expr(&arguments[0], UseMode::Consume)?;
                        let value = if field == "read_text" {
                            Ty::Owned("String".into())
                        } else {
                            Ty::List(Box::new(Ty::Copy))
                        };
                        Ok(Ty::Future(Box::new(Ty::Result(
                            Box::new(value),
                            Box::new(Ty::Owned("IoError".into())),
                        ))))
                    }
                    "write_text" | "write_bytes" => {
                        self.check_expr(&arguments[0], UseMode::Consume)?;
                        self.check_expr(&arguments[1], UseMode::Consume)?;
                        Ok(Ty::Future(Box::new(Ty::Result(
                            Box::new(Ty::Unit),
                            Box::new(Ty::Owned("IoError".into())),
                        ))))
                    }
                    _ => Ok(Ty::Unit),
                };
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "TcpListener")
                && field == "bind"
            {
                self.check_expr(&arguments[0], UseMode::Consume)?;
                return Ok(Ty::Result(
                    Box::new(Ty::TcpListener),
                    Box::new(Ty::Owned("NetworkError".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "UdpSocket")
                && field == "bind"
            {
                self.check_expr(&arguments[0], UseMode::Consume)?;
                return Ok(Ty::Result(
                    Box::new(Ty::UdpSocket),
                    Box::new(Ty::Owned("NetworkError".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "IpAddress")
                && field == "parse"
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                return Ok(Ty::Result(
                    Box::new(Ty::IpAddress),
                    Box::new(Ty::Owned("NetworkError".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Dns")
                && field == "resolve"
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                return Ok(Ty::Result(
                    Box::new(Ty::List(Box::new(Ty::IpAddress))),
                    Box::new(Ty::Owned("NetworkError".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Tls")
                && matches!(field.as_str(), "connect" | "connect_timeout")
            {
                self.check_expr(&arguments[0], UseMode::Consume)?;
                self.check_expr(&arguments[1], UseMode::Read)?;
                if field == "connect_timeout" {
                    self.check_expr(&arguments[2], UseMode::Read)?;
                }
                return Ok(Ty::Future(Box::new(Ty::Result(
                    Box::new(Ty::TlsStream),
                    Box::new(Ty::Owned("NetworkError".into())),
                ))));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Http")
                && matches!(
                    field.as_str(),
                    "get"
                        | "get_timeout"
                        | "post"
                        | "post_timeout"
                        | "post_json"
                        | "post_json_timeout"
                        | "put"
                        | "put_timeout"
                        | "patch"
                        | "patch_timeout"
                        | "delete"
                        | "delete_timeout"
                        | "request"
                )
            {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                if field == "request" {
                    return Ok(Ty::Result(
                        Box::new(Ty::HttpRequest),
                        Box::new(Ty::Owned("HttpError".into())),
                    ));
                }
                return Ok(Ty::Future(Box::new(Ty::Result(
                    Box::new(Ty::HttpResponse),
                    Box::new(Ty::Owned("HttpError".into())),
                ))));
            }
            if let Expression::Identifier(owner) = &object.node
                && matches!(
                    owner.as_str(),
                    "Path"
                        | "File"
                        | "Directory"
                        | "Time"
                        | "Duration"
                        | "Environment"
                        | "Process"
                        | "Database"
                        | "DataStore"
                        | "Crypto"
                        | "Port"
                        | "Mmio"
                )
            {
                for argument in arguments {
                    self.check_expr(
                        argument,
                        if (owner == "Process" && field == "command")
                            || (owner == "Crypto" && field == "import_secret")
                        {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(match (owner.as_str(), field.as_str()) {
                    ("Path", _) => Ty::Path,
                    ("File", "read_text") => Ty::Result(
                        Box::new(Ty::Owned("String".into())),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    ("File", "read_bytes") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    ("File", "size" | "modified_seconds") => {
                        Ty::Result(Box::new(Ty::Copy), Box::new(Ty::Owned("IoError".into())))
                    }
                    ("File", "exists") | ("Directory", "exists") => Ty::Copy,
                    ("Directory", "read") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Path))),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    ("File", _) | ("Directory", _) => {
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Owned("IoError".into())))
                    }
                    ("Time", "now") => Ty::Instant,
                    ("Time", "unix_seconds") => Ty::Copy,
                    ("Time", "ticks") => Ty::Copy,
                    ("Time", "sleep") => Ty::Unit,
                    ("Duration", _) => Ty::Duration,
                    ("Environment", "arguments") => Ty::List(Box::new(Ty::Owned("String".into()))),
                    ("Environment", "get") => Ty::Option(Box::new(Ty::Owned("String".into()))),
                    ("Process", "run") => Ty::Result(
                        Box::new(Ty::Owned("ProcessOutput".into())),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    ("Process", "command") => Ty::Owned("ProcessCommand".into()),
                    ("Database", "open" | "memory") => Ty::Result(
                        Box::new(Ty::Owned("Database".into())),
                        Box::new(Ty::Owned("DataError".into())),
                    ),
                    ("Crypto", "random_bytes") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "random_secret" | "import_secret") => Ty::Result(
                        Box::new(Ty::Owned("SecretBytes".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "sha256" | "hmac_sha256") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "hmac_sha256_verify") => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "hkdf_sha256") => Ty::Result(
                        Box::new(Ty::Owned("SecretBytes".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "aes256_gcm_siv_seal") => Ty::Result(
                        Box::new(Ty::Owned("AeadEnvelope".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "aes256_gcm_siv_open") => Ty::Result(
                        Box::new(Ty::Owned("SecretBytes".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "encode_aead_envelope") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "decode_aead_envelope") => Ty::Result(
                        Box::new(Ty::Owned("AeadEnvelope".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "ed25519_generate") => Ty::Result(
                        Box::new(Ty::Owned("Ed25519SigningKey".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "ed25519_public_key" | "ed25519_sign") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "ed25519_verify") => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "ed25519_key_id") => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "ed25519_verify_keyed") => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "ed25519_verify_lifecycle") => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    (
                        "Crypto",
                        "encode_ed25519_public_key"
                        | "decode_ed25519_public_key"
                        | "encode_ed25519_signature"
                        | "decode_ed25519_signature",
                    ) => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "argon2id_hash_password") => Ty::Result(
                        Box::new(Ty::Owned("String".into())),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Crypto", "argon2id_verify_password") => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("CryptoError".into())),
                    ),
                    ("Port", "read_u8") => Ty::Copy,
                    ("Port", "write_u8") => Ty::Unit,
                    ("Mmio", "read_u8" | "read_u16" | "read_u32") => Ty::Copy,
                    ("Mmio", "write_u8" | "write_u16" | "write_u32") => Ty::Unit,
                    _ => Ty::Unit,
                });
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Json") {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "null" | "bool" | "int" | "uint" => Ty::Json,
                    "float" | "string" | "array" | "object" | "from" => Ty::Result(
                        Box::new(Ty::Json),
                        Box::new(Ty::Owned("ConversionError".into())),
                    ),
                    _ => Ty::Unit,
                });
            }
            if field == "from_json"
                && let Expression::Identifier(owner) = &object.node
                && (self
                    .program
                    .structs
                    .iter()
                    .any(|declaration| declaration.name == *owner)
                    || self
                        .program
                        .enums
                        .iter()
                        .any(|declaration| declaration.name == *owner))
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                let Expression::Identifier(owner) = &object.node else {
                    unreachable!()
                };
                return Ok(Ty::Result(
                    Box::new(Ty::Owned(owner.clone())),
                    Box::new(Ty::Owned("ConversionError".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "String") {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Owned("String".into()));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "CString")
                && field == "new"
                && arguments.len() == 1
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                return Ok(Ty::Result(
                    Box::new(Ty::CString),
                    Box::new(Ty::Owned("String".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "CExport")
                && field == "callback"
                && arguments.len() == 1
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                return Ok(Ty::Copy);
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "CRegistration")
                && ((field == "adopt" && arguments.len() == 2)
                    || (field == "adopt_async" && arguments.len() == 3)
                    || (field == "register_async" && arguments.len() == 4))
            {
                for (index, argument) in arguments.iter().enumerate() {
                    self.check_expr(
                        argument,
                        if field == "register_async" && index == 0 {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(Ty::CRegistration);
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Memory")
                && field == "allocate"
                && arguments.len() == 2
            {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Result(
                    Box::new(Ty::Memory),
                    Box::new(Ty::Owned("String".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "List") {
                for argument in arguments {
                    self.check_expr(
                        argument,
                        if field == "of" {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(Ty::List(Box::new(Ty::Owned("inferred".into()))));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Mutex")
                && field == "new"
                && arguments.len() == 1
            {
                let value = self.check_expr(&arguments[0], UseMode::Consume)?;
                return Ok(Ty::Mutex(Box::new(value)));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "AtomicInt")
                && field == "new"
                && arguments.len() == 1
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                return Ok(Ty::AtomicInt);
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Channel")
                && field == "bounded"
                && arguments.len() == 1
            {
                self.check_expr(&arguments[0], UseMode::Read)?;
                return Ok(Ty::Result(
                    Box::new(Ty::Channel(Box::new(Ty::Owned("inferred".into())))),
                    Box::new(Ty::Owned("String".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Map") {
                for argument in arguments {
                    self.check_expr(
                        argument,
                        if field == "of" {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(Ty::Map(
                    Box::new(Ty::Owned("key".into())),
                    Box::new(Ty::Owned("value".into())),
                ));
            }
            if matches!(&object.node, Expression::Identifier(name) if name == "Set") {
                for argument in arguments {
                    self.check_expr(
                        argument,
                        if field == "of" {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(Ty::Set(Box::new(Ty::Owned("element".into()))));
            }
            if let Ok(Ty::Array(element) | Ty::Slice(element)) = self.expr_ty(object)
                && field == "iter"
            {
                self.check_expr(object, UseMode::Read)?;
                return Ok(Ty::Slice(element));
            }
            if matches!(self.expr_ty(object), Ok(Ty::Path)) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "join" => Ty::Path,
                    "as_string" => Ty::Owned("String".into()),
                    "name" | "extension" => Ty::Option(Box::new(Ty::Owned("String".into()))),
                    "parent" => Ty::Option(Box::new(Ty::Path)),
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "ProcessOutput") {
                self.check_expr(object, UseMode::Read)?;
                return Ok(match field.as_str() {
                    "status" | "success" => Ty::Copy,
                    "stdout" | "stderr" => Ty::List(Box::new(Ty::Copy)),
                    _ => Ty::Result(
                        Box::new(Ty::Owned("String".into())),
                        Box::new(Ty::Owned("ConversionError".into())),
                    ),
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "ProcessCommand") {
                self.check_expr(object, UseMode::Consume)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Consume)?;
                }
                return Ok(if field == "run" {
                    Ty::Result(
                        Box::new(Ty::Owned("ProcessOutput".into())),
                        Box::new(Ty::Owned("IoError".into())),
                    )
                } else if field == "start" {
                    Ty::Result(
                        Box::new(Ty::Owned("ChildProcess".into())),
                        Box::new(Ty::Owned("IoError".into())),
                    )
                } else {
                    Ty::Owned("ProcessCommand".into())
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "ChildProcess") {
                if field == "wait" {
                    self.check_expr(object, UseMode::Consume)?;
                } else {
                    let place = self.place(object)?;
                    self.check_borrow(&place, true, object.span)?;
                    self.use_place(&place, UseMode::Read, object.span)?;
                }
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "read_stdout" | "read_stderr" => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    "try_wait" => Ty::Result(
                        Box::new(Ty::Option(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    "wait" => Ty::Result(
                        Box::new(Ty::Owned("ProcessOutput".into())),
                        Box::new(Ty::Owned("IoError".into())),
                    ),
                    _ => Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Owned("IoError".into()))),
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "Database") {
                if field == "close" {
                    self.check_expr(object, UseMode::Consume)?;
                } else {
                    let place = self.place(object)?;
                    self.check_borrow(
                        &place,
                        !matches!(field.as_str(), "changes" | "last_insert_id"),
                        object.span,
                    )?;
                    self.use_place(&place, UseMode::Read, object.span)?;
                }
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "execute" => {
                        Ty::Result(Box::new(Ty::Copy), Box::new(Ty::Owned("DataError".into())))
                    }
                    "query" => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Json))),
                        Box::new(Ty::Owned("DataError".into())),
                    ),
                    "begin" | "commit" | "rollback" | "close" => {
                        Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Owned("DataError".into())))
                    }
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Url)) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "as_string" | "scheme" | "path" => Ty::Owned("String".into()),
                    "host" | "query" => Ty::Option(Box::new(Ty::Owned("String".into()))),
                    "port" => Ty::Option(Box::new(Ty::Copy)),
                    "join_path" | "query_param" => Ty::Result(
                        Box::new(Ty::Url),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Json)) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "as_string" | "kind" => Ty::Owned("String".into()),
                    "get" | "at" => Ty::Option(Box::new(Ty::Json)),
                    "as_text" => Ty::Result(
                        Box::new(Ty::Owned("String".into())),
                        Box::new(Ty::Owned("ConversionError".into())),
                    ),
                    "as_bool" | "as_int" | "as_uint" | "as_f64" => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("ConversionError".into())),
                    ),
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::TcpStream)) {
                let place = self.place(object)?;
                self.check_borrow(&place, true, object.span)?;
                self.use_place(&place, UseMode::Read, object.span)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "read" => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    "read_async" | "read_async_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ))),
                    "write" => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    "write_async" | "write_async_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ))),
                    "shutdown_read" | "shutdown_write" => Ty::Result(
                        Box::new(Ty::Unit),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    _ => Ty::Unit,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::TlsStream)) {
                let place = self.place(object)?;
                self.check_borrow(&place, true, object.span)?;
                self.use_place(&place, UseMode::Read, object.span)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "read" => Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    "read_async" | "read_async_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::List(Box::new(Ty::Copy))),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ))),
                    "write" => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    "write_async" | "write_async_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ))),
                    _ => Ty::Unit,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::HttpResponse)) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "body" => Ty::List(Box::new(Ty::Copy)),
                    "text" => Ty::Result(
                        Box::new(Ty::Owned("String".into())),
                        Box::new(Ty::Owned("HttpError".into())),
                    ),
                    "json" => {
                        Ty::Result(Box::new(Ty::Json), Box::new(Ty::Owned("HttpError".into())))
                    }
                    "url" => Ty::Owned("String".into()),
                    "header" => Ty::Option(Box::new(Ty::Owned("String".into()))),
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::HttpRequest)) {
                self.check_expr(object, UseMode::Consume)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "header" | "text" | "bytes" | "json" => Ty::Result(
                        Box::new(Ty::HttpRequest),
                        Box::new(Ty::Owned("HttpError".into())),
                    ),
                    "send" | "send_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::HttpResponse),
                        Box::new(Ty::Owned("HttpError".into())),
                    ))),
                    _ => Ty::Unit,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::TcpListener)) {
                if field == "close" {
                    let place = self.place(object)?;
                    self.check_borrow(&place, true, object.span)?;
                    self.use_place(&place, UseMode::Read, object.span)?;
                } else {
                    self.check_expr(object, UseMode::Read)?;
                }
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "accept" | "accept_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::TcpStream),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ))),
                    "local_port" => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    _ => Ty::Unit,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::UdpSocket)) {
                let place = self.place(object)?;
                self.check_borrow(&place, true, object.span)?;
                self.use_place(&place, UseMode::Read, object.span)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "receive_from" => Ty::Result(
                        Box::new(Ty::UdpDatagram),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    "receive_from_async" | "receive_from_async_timeout" => {
                        Ty::Future(Box::new(Ty::Result(
                            Box::new(Ty::UdpDatagram),
                            Box::new(Ty::Owned("NetworkError".into())),
                        )))
                    }
                    "send_to" | "local_port" => Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ),
                    "send_to_async" | "send_to_async_timeout" => Ty::Future(Box::new(Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("NetworkError".into())),
                    ))),
                    _ => Ty::Unit,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::UdpDatagram)) {
                self.check_expr(object, UseMode::Read)?;
                return Ok(match field.as_str() {
                    "bytes" => Ty::List(Box::new(Ty::Copy)),
                    "source" => Ty::SocketAddress,
                    "len" | "is_empty" => Ty::Copy,
                    _ => Ty::Unit,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::IpAddress)) {
                self.check_expr(object, UseMode::Read)?;
                return Ok(match field.as_str() {
                    "as_string" => Ty::Owned("String".into()),
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Instant)) {
                self.check_expr(object, UseMode::Read)?;
                return Ok(Ty::Duration);
            }
            if matches!(self.expr_ty(object), Ok(Ty::Duration)) {
                self.check_expr(object, UseMode::Read)?;
                return Ok(Ty::Copy);
            }
            if let Ok(Ty::Thread(result)) = self.expr_ty(object)
                && field == "join"
                && arguments.is_empty()
            {
                self.check_expr(object, UseMode::Consume)?;
                return Ok(*result);
            }
            if matches!(self.expr_ty(object), Ok(Ty::Task(_))) && arguments.is_empty() {
                if field == "cancel" {
                    self.check_expr(object, UseMode::Consume)?;
                    return Ok(Ty::Unit);
                }
                if field == "is_finished" {
                    self.check_expr(object, UseMode::Read)?;
                    return Ok(Ty::Copy);
                }
            }
            if let Ok(Ty::Mutex(value)) = self.expr_ty(object) {
                if field == "share" && arguments.is_empty() {
                    self.check_expr(object, UseMode::Read)?;
                    return Ok(Ty::Mutex(value));
                }
                if field == "lock" && arguments.is_empty() {
                    self.check_expr(object, UseMode::Read)?;
                    return Ok(Ty::MutexGuard(value));
                }
            }
            if let Ok(Ty::Channel(value)) = self.expr_ty(object) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(
                        argument,
                        if field == "send" {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(match field.as_str() {
                    "share" => Ty::Channel(value),
                    "receive" => Ty::Option(value),
                    "close" => Ty::Unit,
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::AtomicInt)) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "share" => Ty::AtomicInt,
                    name if name.starts_with("store") => Ty::Unit,
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::CString)) {
                let place = self.place(object)?;
                self.use_place(&place, UseMode::Read, object.span)?;
                if field == "as_c_str" && arguments.is_empty() {
                    self.check_borrow(&place, false, object.span)?;
                    self.loans.push(Loan {
                        place,
                        mutable: false,
                        borrower: None,
                        at: object.span,
                    });
                    return Ok(Ty::CStr);
                }
                return Ok(match field.as_str() {
                    "to_string" => Ty::Owned("String".into()),
                    _ => Ty::Copy,
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::CRegistration)) {
                self.check_expr(
                    object,
                    if field == "close" {
                        UseMode::Consume
                    } else {
                        UseMode::Read
                    },
                )?;
                return Ok(if field == "close" { Ty::Unit } else { Ty::Copy });
            }
            if matches!(self.expr_ty(object), Ok(Ty::CStr)) {
                self.check_expr(object, UseMode::Read)?;
                return Ok(if field == "to_string" {
                    Ty::Owned("String".into())
                } else {
                    Ty::Copy
                });
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(name)) if name == "SecretBytes") {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Copy);
            }
            if matches!(self.expr_ty(object), Ok(Ty::Memory)) {
                if matches!(
                    field.as_str(),
                    "write" | "fill" | "copy_from" | "as_mut_ptr"
                ) {
                    let place = self.place(object)?;
                    self.check_borrow(&place, true, object.span)?;
                } else {
                    self.check_expr(object, UseMode::Read)?;
                }
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "as_ptr" => Ty::MemoryPointer(Box::new(Ty::Copy), false),
                    "as_mut_ptr" => Ty::MemoryPointer(Box::new(Ty::Copy), true),
                    _ => Ty::Copy,
                });
            }
            if let Ok(Ty::RawPointer(inner, mutable) | Ty::MemoryPointer(inner, mutable)) =
                self.expr_ty(object)
                && matches!(field.as_str(), "offset" | "read" | "write")
            {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "offset" => match self.expr_ty(object)? {
                        Ty::MemoryPointer(_, _) => Ty::MemoryPointer(inner, mutable),
                        _ => Ty::RawPointer(inner, mutable),
                    },
                    "read" => *inner,
                    _ => Ty::Unit,
                });
            }
            if let Ok(Ty::Map(key, value)) = self.expr_ty(object) {
                let method = match field.as_str() {
                    "count" => "len",
                    "empty" => "is_empty",
                    "contains_key" => "has",
                    "insert" => "set",
                    other => other,
                };
                if matches!(
                    method,
                    "len" | "capacity" | "is_empty" | "has" | "get" | "keys" | "values"
                ) {
                    self.check_expr(object, UseMode::Read)?;
                    for argument in arguments {
                        self.check_expr(argument, UseMode::Read)?;
                    }
                    return Ok(match method {
                        "get" => Ty::Option(Box::new(Ty::Reference(value, false))),
                        "keys" => Ty::Slice(key),
                        "values" => Ty::Slice(value),
                        _ => Ty::Copy,
                    });
                }
                let place = self.place(object)?;
                self.check_borrow(&place, true, object.span)?;
                for (index, argument) in arguments.iter().enumerate() {
                    self.check_expr(
                        argument,
                        if method == "set" && index == 1 {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(match method {
                    "get_mut" => Ty::Option(Box::new(Ty::Reference(value, true))),
                    "set" | "remove" => Ty::Option(value),
                    _ => Ty::Unit,
                });
            }
            if let Ok(Ty::Set(element)) = self.expr_ty(object) {
                let method = match field.as_str() {
                    "count" => "len",
                    "empty" => "is_empty",
                    "contains" => "has",
                    "insert" => "add",
                    other => other,
                };
                if matches!(method, "len" | "capacity" | "is_empty" | "has" | "iter") {
                    self.check_expr(object, UseMode::Read)?;
                    for argument in arguments {
                        self.check_expr(argument, UseMode::Read)?;
                    }
                    return Ok(if method == "iter" {
                        Ty::Slice(element)
                    } else {
                        Ty::Copy
                    });
                }
                let place = self.place(object)?;
                self.check_borrow(&place, true, object.span)?;
                for argument in arguments {
                    self.check_expr(
                        argument,
                        if method == "add" {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                return Ok(if matches!(method, "add" | "remove") {
                    Ty::Copy
                } else {
                    Ty::Unit
                });
            }
            if let Ok(Ty::List(element)) = self.expr_ty(object) {
                let field = match field.as_str() {
                    "add" => "push",
                    "count" => "len",
                    "empty" => "is_empty",
                    other => other,
                };
                if matches!(field, "len" | "capacity" | "is_empty") {
                    self.check_expr(object, UseMode::Read)?;
                    return Ok(Ty::Copy);
                }
                if field == "iter" {
                    self.check_expr(object, UseMode::Read)?;
                    return Ok(Ty::Slice(element));
                }
                if matches!(
                    field,
                    "push" | "pop" | "get_mut" | "insert" | "remove" | "clear"
                ) {
                    let place = self.place(object)?;
                    self.check_borrow(&place, true, object.span)?;
                    for (argument_index, argument) in arguments.iter().enumerate() {
                        let consumes =
                            field == "push" || (field == "insert" && argument_index == 1);
                        self.check_expr(
                            argument,
                            if consumes {
                                UseMode::Consume
                            } else {
                                UseMode::Read
                            },
                        )?;
                    }
                    return Ok(match field {
                        "pop" => Ty::Option(element),
                        "get_mut" => Ty::Option(Box::new(Ty::Reference(element, true))),
                        "remove" => *element,
                        _ => Ty::Unit,
                    });
                }
                if field == "get" {
                    self.check_expr(object, UseMode::Read)?;
                    for argument in arguments {
                        self.check_expr(argument, UseMode::Read)?;
                    }
                    return Ok(Ty::Option(Box::new(Ty::Reference(element, false))));
                }
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "String")
                && matches!(field.as_str(), "len" | "capacity" | "is_empty")
            {
                self.check_expr(object, UseMode::Read)?;
                return Ok(Ty::Copy);
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "String")
                && matches!(field.as_str(), "contains" | "starts_with" | "ends_with")
            {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Copy);
            }
            if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "String")
                && matches!(
                    field.as_str(),
                    "push" | "push_str" | "append" | "add" | "clear"
                )
            {
                let place = self.place(object)?;
                self.check_borrow(&place, true, object.span)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(Ty::Unit);
            }
            if matches!(&object.node, Expression::Identifier(name) if self.program.enums.iter().any(|declaration| declaration.name == *name))
            {
                let actual_payload = arguments
                    .iter()
                    .map(|argument| self.check_expr(argument, UseMode::Consume))
                    .collect::<Result<Vec<_>, _>>()?;
                self.loans.truncate(temporary_start);
                let owner = match &object.node {
                    Expression::Identifier(name) => name.clone(),
                    _ => unreachable!(),
                };
                let components = self
                    .program
                    .enums
                    .iter()
                    .find(|declaration| declaration.name == owner)
                    .and_then(|declaration| {
                        declaration
                            .variants
                            .iter()
                            .find(|variant| variant.name == *field)
                    })
                    .map(|variant| {
                        variant
                            .payload
                            .iter()
                            .cloned()
                            .zip(actual_payload)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                return Ok(self.instantiated_nominal_ty(&owner, &components));
            }
            if matches!(&object.node, Expression::Identifier(name) if is_numeric_type_name(name)) {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Consume)?;
                }
                self.loans.truncate(temporary_start);
                return Ok(if field == "try_from" {
                    Ty::Result(
                        Box::new(Ty::Copy),
                        Box::new(Ty::Owned("ConversionError".into())),
                    )
                } else {
                    Ty::Copy
                });
            }
            let receiver_ty = self.expr_ty(object)?;
            if let Some(method) = self.find_method(&receiver_ty, field) {
                let receiver_mode = method
                    .parameters
                    .first()
                    .map(|parameter| parameter.ty.qualifier)
                    .unwrap_or(TypeQualifier::Owned);
                match receiver_mode {
                    TypeQualifier::Owned => {
                        self.check_expr(object, UseMode::Consume)?;
                    }
                    TypeQualifier::SharedReference => {
                        let place = self.place(object)?;
                        self.check_borrow(&place, false, object.span)?;
                    }
                    TypeQualifier::MutableReference => {
                        let place = self.place(object)?;
                        self.check_borrow(&place, true, object.span)?;
                    }
                    TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer => {
                        self.check_expr(object, UseMode::Read)?;
                    }
                }
                for (argument, parameter) in arguments.iter().zip(method.parameters.iter().skip(1))
                {
                    self.check_expr(
                        argument,
                        if parameter.ty.qualifier == TypeQualifier::Owned {
                            UseMode::Consume
                        } else {
                            UseMode::Read
                        },
                    )?;
                }
                self.loans.truncate(temporary_start);
                let result = method
                    .return_type
                    .as_ref()
                    .map(|ty| self.ty_from_name(ty))
                    .unwrap_or(Ty::Unit);
                return Ok(if method.asynchronous {
                    Ty::Future(Box::new(result))
                } else {
                    result
                });
            }
            self.check_expr(object, UseMode::Read)?;
        } else {
            self.check_expr(callee, UseMode::Read)?;
        }
        for argument in arguments {
            self.check_expr(argument, UseMode::Consume)?;
        }
        self.loans.truncate(temporary_start);
        let _ = span;
        Ok(Ty::Owned("call-result".into()))
    }

    fn use_place(&mut self, place: &Place, mode: UseMode, span: Span) -> Result<Ty, Diagnostic> {
        let slot = self.slots[&place.root].clone();
        match &slot.state {
            InitState::Uninitialized => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("use of uninitialized value `{}`", slot.name),
                    span,
                )
                .with_help(format!(
                    "`{}` was declared at {}:{}",
                    slot.name, slot.defined.start.line, slot.defined.start.column
                )));
            }
            InitState::Moved { at } => return Err(self.moved_error(&slot, *at, span)),
            InitState::Partial { fields } if place.fields.is_empty() => {
                let (_, moved) = fields.iter().next().unwrap();
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("use of partially moved value `{}`", slot.name),
                    span,
                )
                .with_help(format!(
                    "a field was moved at {}:{}",
                    moved.start.line, moved.start.column
                )));
            }
            InitState::Partial { fields } if fields.contains_key(&place.fields[0]) => {
                return Err(self.moved_error(&slot, fields[&place.fields[0]], span));
            }
            _ => {}
        }
        self.ensure_no_conflicting_loan(place, false, span)?;
        let ty = self.place_ty(place)?;
        if matches!(mode, UseMode::Consume) && !self.ty_is_copy(&ty) {
            if place.fields.first().is_some_and(|field| field == "<deref>") {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "cannot move a non-Copy value out of a reference",
                    span,
                )
                .with_help("borrow the field or return a Copy value instead"));
            }
            self.ensure_no_conflicting_loan(place, true, span)?;
            let slot = self.slots.get_mut(&place.root).unwrap();
            if place.fields.is_empty() {
                slot.state = InitState::Moved { at: span };
            } else {
                let field = place.fields[0].clone();
                match &mut slot.state {
                    InitState::Partial { fields } => {
                        fields.insert(field, span);
                    }
                    _ => {
                        slot.state = InitState::Partial {
                            fields: HashMap::from([(field, span)]),
                        };
                    }
                }
            }
        }
        Ok(ty)
    }

    fn check_borrow(&self, place: &Place, mutable: bool, span: Span) -> Result<(), Diagnostic> {
        let slot = &self.slots[&place.root];
        if mutable && !slot.mutable {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("cannot mutably borrow immutable `{}`", slot.name),
                span,
            ));
        }
        self.ensure_initialized(place, span)?;
        for loan in &self.loans {
            if places_overlap(&loan.place, place) && (mutable || loan.mutable) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    if mutable && loan.mutable {
                        "cannot create a second mutable borrow"
                    } else {
                        "shared and mutable borrows overlap"
                    },
                    span,
                )
                .with_help(format!(
                    "conflicting borrow began at {}:{}",
                    loan.at.start.line, loan.at.start.column
                )));
            }
        }
        Ok(())
    }

    fn ensure_no_conflicting_loan(
        &self,
        place: &Place,
        mutation: bool,
        span: Span,
    ) -> Result<(), Diagnostic> {
        for loan in &self.loans {
            if places_overlap(&loan.place, place) && (mutation || loan.mutable) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    if mutation {
                        "cannot mutate or move a borrowed value"
                    } else {
                        "cannot access a value through an active mutable borrow"
                    },
                    span,
                )
                .with_help(format!(
                    "borrow began at {}:{}",
                    loan.at.start.line, loan.at.start.column
                )));
            }
        }
        Ok(())
    }

    fn ensure_initialized(&self, place: &Place, span: Span) -> Result<(), Diagnostic> {
        let slot = &self.slots[&place.root];
        match &slot.state {
            InitState::Initialized => Ok(()),
            InitState::Partial { fields }
                if !place.fields.is_empty() && !fields.contains_key(&place.fields[0]) =>
            {
                Ok(())
            }
            InitState::Moved { at } => Err(self.moved_error(slot, *at, span)),
            InitState::Partial { .. } => Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("use of partially moved value `{}`", slot.name),
                span,
            )),
            InitState::Uninitialized => Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("use of uninitialized value `{}`", slot.name),
                span,
            )),
        }
    }

    fn place(&self, expression: &Expr) -> Result<Place, Diagnostic> {
        match &expression.node {
            Expression::Identifier(name) => self
                .lookup(name)
                .map(|root| Place {
                    root,
                    fields: vec![],
                })
                .ok_or_else(|| self.error_unknown(name, expression.span)),
            Expression::FieldAccess { object, field, .. } => {
                if let Expression::Identifier(name) = &object.node
                    && self
                        .program
                        .enums
                        .iter()
                        .any(|declaration| declaration.name == *name)
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "enum constructor is not a storage place",
                        expression.span,
                    ));
                }
                let mut place = if let Expression::Identifier(name) = &object.node {
                    if let Some(id) = self.lookup(name) {
                        if ty_is_borrowed_view(&self.slots[&id].ty)
                            && let Some(origin) = self.slots[&id].reference_origin.clone()
                        {
                            origin
                        } else if matches!(self.slots[&id].ty, Ty::Reference(_, _)) {
                            Place {
                                root: id,
                                fields: vec!["<deref>".into()],
                            }
                        } else {
                            self.place(object)?
                        }
                    } else {
                        self.place(object)?
                    }
                } else {
                    self.place(object)?
                };
                place.fields.push(field.clone());
                Ok(place)
            }
            Expression::Dereference(target) => {
                if let Expression::Identifier(name) = &target.node {
                    let root = self
                        .lookup(name)
                        .ok_or_else(|| self.error_unknown(name, target.span))?;
                    // A `str` view is only a pointer and length. Places derived
                    // from it must retain the original String as their owner.
                    if matches!(
                        self.slots[&root].ty,
                        Ty::Reference(ref inner, false) if matches!(&**inner, Ty::Str)
                    ) {
                        if let Some(origin) = self.slots[&root].reference_origin.clone() {
                            Ok(origin)
                        } else {
                            Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "borrowed `str` has no tracked owner",
                                expression.span,
                            ))
                        }
                    } else if matches!(
                        self.slots[&root].ty,
                        Ty::Reference(_, _) | Ty::MutexGuard(_)
                    ) {
                        Ok(Place {
                            root,
                            fields: vec!["<deref>".into()],
                        })
                    } else {
                        Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "dereference target has no tracked origin",
                            expression.span,
                        ))
                    }
                } else if let Some(origin) = self.reference_origin(target) {
                    Ok(origin)
                } else {
                    Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "dereference target has no tracked origin",
                        expression.span,
                    ))
                }
            }
            Expression::Index { object, index } => {
                let mut place = self.place(object)?;
                let region = match index.node {
                    Expression::Integer(value) => format!("@i:{value}"),
                    _ => "@i:*".into(),
                };
                place.fields.push(region);
                Ok(place)
            }
            Expression::Subslice { object, start, end } => {
                let mut place = self.place(object)?;
                let region = match (&start.node, &end.node) {
                    (Expression::Integer(a), Expression::Integer(b)) => format!("@r:{a}:{b}"),
                    _ => "@r:*".into(),
                };
                place.fields.push(region);
                Ok(place)
            }
            _ => Err(Diagnostic::new(
                DiagnosticKind::Type,
                "expression is not a storage place",
                expression.span,
            )),
        }
    }

    fn place_ty(&self, place: &Place) -> Result<Ty, Diagnostic> {
        let mut ty = self.slots[&place.root].ty.clone();
        for field in &place.fields {
            if field == "<deref>" {
                ty = match ty {
                    Ty::Reference(inner, _)
                    | Ty::RawPointer(inner, _)
                    | Ty::MemoryPointer(inner, _)
                    | Ty::MutexGuard(inner) => *inner,
                    other => other,
                };
                continue;
            }
            if field.starts_with("@i:") {
                ty = match ty {
                    Ty::Array(element) | Ty::Slice(element) | Ty::List(element) => *element,
                    other => other,
                };
                continue;
            }
            if field.starts_with("@k:") {
                ty = match ty {
                    Ty::Map(_, value) => *value,
                    other => other,
                };
                continue;
            }
            if field.starts_with("@r:") {
                ty = match ty {
                    Ty::Array(element) | Ty::Slice(element) | Ty::List(element) => {
                        Ty::Slice(element)
                    }
                    Ty::Owned(name) if name == "String" => Ty::Str,
                    Ty::Str => Ty::Str,
                    other => other,
                };
                continue;
            }
            let (name, arguments) = match ty {
                Ty::Owned(name) => (name, Vec::new()),
                Ty::Nominal(name, arguments) => (name, arguments),
                _ => return Ok(Ty::Owned("field".into())),
            };
            let declaration = self
                .program
                .structs
                .iter()
                .find(|declaration| declaration.name == name)
                .ok_or_else(|| {
                    Diagnostic::new(
                        DiagnosticKind::Type,
                        "field access requires a struct",
                        self.slots[&place.root].defined,
                    )
                })?;
            let substitutions = declaration
                .generics
                .iter()
                .map(|generic| generic.name.clone())
                .zip(arguments)
                .collect::<HashMap<_, _>>();
            ty = declaration
                .fields
                .iter()
                .find(|candidate| candidate.name == *field)
                .map(|field| substitute_ownership_ty(self.ty_from_name(&field.ty), &substitutions))
                .unwrap_or(Ty::Owned("field".into()));
        }
        Ok(ty)
    }

    fn expr_ty(&self, expression: &Expr) -> Result<Ty, Diagnostic> {
        match &expression.node {
            Expression::Identifier(name) => self
                .lookup(name)
                .map(|id| self.slots[&id].ty.clone())
                .ok_or_else(|| self.error_unknown(name, expression.span)),
            Expression::FieldAccess { .. } => self
                .place(expression)
                .and_then(|place| self.place_ty(&place)),
            Expression::Borrow { mutable, target } => Ok(Ty::Reference(
                Box::new(self.place(target).and_then(|place| self.place_ty(&place))?),
                *mutable,
            )),
            Expression::Move(target) => self.expr_ty(target),
            Expression::Dereference(target) => match self.expr_ty(target)? {
                Ty::MutexGuard(inner) => Ok(*inner),
                other => Ok(other),
            },
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::Character(_)
            | Expression::Bool(_) => Ok(Ty::Copy),
            Expression::String(_) => Ok(Ty::Owned("String".into())),
            Expression::StructConstruct { name, fields, .. } => {
                let actual_fields = fields
                    .iter()
                    .map(|field| {
                        self.expr_ty(&field.value)
                            .map(|ty| (field.name.as_str(), ty))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let components = self
                    .program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name)
                    .map(|declaration| {
                        declaration
                            .fields
                            .iter()
                            .filter_map(|field| {
                                actual_fields
                                    .iter()
                                    .find(|(actual_name, _)| *actual_name == field.name)
                                    .map(|(_, actual)| (field.ty.clone(), actual.clone()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(self.instantiated_nominal_ty(name, &components))
            }
            Expression::Call { callee, .. } => {
                let Expression::FieldAccess { object, field, .. } = &callee.node else {
                    return Ok(Ty::Owned("expression".into()));
                };
                if matches!(&object.node, Expression::Identifier(owner) if owner == "Process")
                    && field == "command"
                {
                    return Ok(Ty::Owned("ProcessCommand".into()));
                }
                if matches!(&object.node, Expression::Identifier(owner) if owner == "Database")
                    && matches!(field.as_str(), "open" | "memory")
                {
                    return Ok(Ty::Result(
                        Box::new(Ty::Owned("Database".into())),
                        Box::new(Ty::Owned("DataError".into())),
                    ));
                }
                if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "ProcessCommand")
                {
                    return Ok(if field == "run" {
                        Ty::Result(
                            Box::new(Ty::Owned("ProcessOutput".into())),
                            Box::new(Ty::Owned("IoError".into())),
                        )
                    } else if field == "start" {
                        Ty::Result(
                            Box::new(Ty::Owned("ChildProcess".into())),
                            Box::new(Ty::Owned("IoError".into())),
                        )
                    } else {
                        Ty::Owned("ProcessCommand".into())
                    });
                }
                if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "ChildProcess")
                {
                    return Ok(match field.as_str() {
                        "read_stdout" | "read_stderr" => Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::Copy))),
                            Box::new(Ty::Owned("IoError".into())),
                        ),
                        "try_wait" => Ty::Result(
                            Box::new(Ty::Option(Box::new(Ty::Copy))),
                            Box::new(Ty::Owned("IoError".into())),
                        ),
                        "wait" => Ty::Result(
                            Box::new(Ty::Owned("ProcessOutput".into())),
                            Box::new(Ty::Owned("IoError".into())),
                        ),
                        _ => Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Owned("IoError".into()))),
                    });
                }
                if matches!(self.expr_ty(object), Ok(Ty::Owned(ref name)) if name == "Database") {
                    return Ok(match field.as_str() {
                        "execute" => {
                            Ty::Result(Box::new(Ty::Copy), Box::new(Ty::Owned("DataError".into())))
                        }
                        "query" => Ty::Result(
                            Box::new(Ty::List(Box::new(Ty::Json))),
                            Box::new(Ty::Owned("DataError".into())),
                        ),
                        "begin" | "commit" | "rollback" | "close" => {
                            Ty::Result(Box::new(Ty::Unit), Box::new(Ty::Owned("DataError".into())))
                        }
                        _ => Ty::Copy,
                    });
                }
                Ok(Ty::Owned("expression".into()))
            }
            _ => Ok(Ty::Owned("expression".into())),
        }
    }

    fn reference_origin(&self, expression: &Expr) -> Option<Place> {
        self.borrow_origins(expression)
            .first()
            .map(|(place, _)| place.clone())
    }

    fn borrow_origins(&self, expression: &Expr) -> Vec<(Place, bool)> {
        let mut origins = match &expression.node {
            Expression::Borrow { mutable, target } => self
                .place(target)
                .ok()
                .map(|place| vec![(place, *mutable)])
                .unwrap_or_default(),
            Expression::Identifier(name) => self
                .lookup(name)
                .map(|id| self.slots[&id].borrow_origins.clone())
                .unwrap_or_default(),
            Expression::Try(value) | Expression::Move(value) => self.borrow_origins(value),
            Expression::Array(values) => values
                .iter()
                .flat_map(|value| self.borrow_origins(value))
                .collect(),
            Expression::StructConstruct { fields, .. } => fields
                .iter()
                .flat_map(|field| self.borrow_origins(&field.value))
                .collect(),
            Expression::FieldAccess { object, .. }
            | Expression::Index { object, .. }
            | Expression::Subslice { object, .. }
            | Expression::Dereference(object) => self.borrow_origins(object),
            Expression::Match { value, arms } => {
                let mut origins = self.borrow_origins(value);
                origins.extend(arms.iter().flat_map(|arm| self.borrow_origins(&arm.value)));
                origins
            }
            Expression::Call { callee, arguments }
                if matches!(
                    &callee.node,
                    Expression::Identifier(name)
                        if matches!(name.as_str(), "Some" | "Ok" | "Err")
                ) || matches!(
                    &callee.node,
                    Expression::FieldAccess { object, field, .. }
                        if matches!(&object.node, Expression::Identifier(name)
                            if matches!(name.as_str(), "List" | "Map" | "Set" | "Mutex"))
                            && matches!(field.as_str(), "of" | "new")
                ) || matches!(
                    &callee.node,
                    Expression::FieldAccess { object, .. }
                        if matches!(&object.node, Expression::Identifier(name)
                            if self.program.enums.iter().any(|declaration| declaration.name == *name))
                ) =>
            {
                arguments
                    .iter()
                    .flat_map(|argument| self.borrow_origins(argument))
                    .collect()
            }
            Expression::Call { callee, arguments }
                if arguments.is_empty()
                    && matches!(
                        &callee.node,
                        Expression::FieldAccess { object, field, .. }
                            if matches!(field.as_str(), "as_ptr" | "as_mut_ptr")
                                && matches!(self.expr_ty(object), Ok(Ty::Memory))
                    ) =>
            {
                let Expression::FieldAccess { object, field, .. } = &callee.node else {
                    unreachable!()
                };
                self.place(object)
                    .ok()
                    .map(|place| vec![(place, field == "as_mut_ptr")])
                    .unwrap_or_default()
            }
            Expression::Call { callee, arguments }
                if arguments.len() == 1
                    && matches!(
                        &callee.node,
                        Expression::FieldAccess { field, .. } if field == "offset"
                    ) =>
            {
                let Expression::FieldAccess { object, .. } = &callee.node else {
                    unreachable!()
                };
                self.borrow_origins(object)
            }
            Expression::Call { callee, arguments }
                if matches!(
                    &callee.node,
                    Expression::FieldAccess { field, .. }
                        if matches!(field.as_str(), "get" | "get_mut")
                ) && arguments.len() == 1 =>
            {
                let Expression::FieldAccess { object, field, .. } = &callee.node else {
                    unreachable!()
                };
                let mut origins = self.borrow_origins(object);
                if origins.is_empty()
                    && let Ok(mut place) = self.place(object)
                {
                    place.fields.push(match arguments[0].node {
                        Expression::Integer(value) => format!("@i:{value}"),
                        _ => "@i:*".into(),
                    });
                    origins.push((place, field == "get_mut"));
                }
                origins
            }
            Expression::Call { callee, arguments }
                if matches!(
                    &callee.node,
                    Expression::FieldAccess { field, .. } if field == "as_c_str"
                ) && arguments.is_empty() =>
            {
                let Expression::FieldAccess { object, .. } = &callee.node else {
                    unreachable!()
                };
                self.place(object)
                    .ok()
                    .map(|place| vec![(place, false)])
                    .unwrap_or_default()
            }
            Expression::Call { callee, arguments } => {
                let Expression::Identifier(name) = &callee.node else {
                    return vec![];
                };
                let Some(function) = self
                    .program
                    .functions
                    .iter()
                    .find(|function| function.name == *name)
                else {
                    return vec![];
                };
                if !function
                    .return_type
                    .as_ref()
                    .is_some_and(|ty| self.type_name_contains_reference(ty))
                {
                    return vec![];
                }
                function
                    .parameters
                    .iter()
                    .zip(arguments)
                    .filter(|(parameter, _)| self.type_name_contains_reference(&parameter.ty))
                    .flat_map(|(parameter, argument)| {
                        let mut origins = self.borrow_origins(argument);
                        if origins.is_empty()
                            && let Ok(place) = self.place(argument)
                        {
                            origins.push((
                                place,
                                self.ty_contains_mutable_reference(
                                    &self.ty_from_name(&parameter.ty),
                                ),
                            ));
                        }
                        origins
                    })
                    .collect()
            }
            _ => vec![],
        };
        origins.sort_by(|(left, left_mutable), (right, right_mutable)| {
            left.root
                .0
                .cmp(&right.root.0)
                .then_with(|| left.fields.cmp(&right.fields))
                .then_with(|| left_mutable.cmp(right_mutable))
        });
        origins.dedup();
        origins
    }

    fn closure_origins(&self, expression: &Expr) -> Vec<(Place, bool)> {
        let mut origins = match &expression.node {
            Expression::Closure {
                move_captures,
                parameters,
                body,
                ..
            } => crate::ast::closure_capture_uses(parameters, body)
                .into_iter()
                .flat_map(|(name, usage)| {
                    let Some(root) = self.lookup(&name) else {
                        return vec![];
                    };
                    let slot = &self.slots[&root];
                    if !slot.closure_origins.is_empty() {
                        return slot.closure_origins.clone();
                    }
                    if self.ty_contains_reference(&slot.ty) {
                        if !slot.borrow_origins.is_empty() {
                            return slot
                                .borrow_origins
                                .iter()
                                .map(|(place, mutable)| (place.clone(), *mutable || usage.mutated))
                                .collect();
                        }
                        return vec![(
                            Place {
                                root,
                                fields: vec![],
                            },
                            usage.mutated || self.ty_contains_mutable_reference(&slot.ty),
                        )];
                    }
                    if *move_captures {
                        vec![]
                    } else {
                        vec![(
                            Place {
                                root,
                                fields: vec![],
                            },
                            usage.mutated,
                        )]
                    }
                })
                .collect(),
            Expression::Identifier(name) => self
                .lookup(name)
                .map(|id| self.slots[&id].closure_origins.clone())
                .unwrap_or_default(),
            Expression::Move(value) => self.closure_origins(value),
            Expression::Array(values) => values
                .iter()
                .flat_map(|value| self.closure_origins(value))
                .collect(),
            Expression::StructConstruct { fields, .. } => fields
                .iter()
                .flat_map(|field| self.closure_origins(&field.value))
                .collect(),
            Expression::FieldAccess { object, .. } | Expression::Index { object, .. }
                if self
                    .expr_ty(expression)
                    .is_ok_and(|ty| self.ty_contains_function(&ty)) =>
            {
                self.closure_origins(object)
            }
            Expression::Match { value, arms } => {
                let mut origins = self.closure_origins(value);
                origins.extend(arms.iter().flat_map(|arm| {
                    arm.guard
                        .as_ref()
                        .into_iter()
                        .flat_map(|guard| self.closure_origins(guard))
                }));
                origins.extend(arms.iter().flat_map(|arm| self.closure_origins(&arm.value)));
                origins
            }
            Expression::Try(value) => self.closure_origins(value),
            Expression::Call { callee, arguments }
                if matches!(
                    &callee.node,
                    Expression::Identifier(name)
                        if matches!(name.as_str(), "Some" | "Ok" | "Err")
                ) || matches!(
                    &callee.node,
                    Expression::FieldAccess { object, field, .. }
                        if matches!(&object.node, Expression::Identifier(name)
                            if matches!(name.as_str(), "List" | "Map" | "Set" | "Mutex"))
                            && matches!(field.as_str(), "of" | "new")
                ) =>
            {
                arguments
                    .iter()
                    .flat_map(|argument| self.closure_origins(argument))
                    .collect()
            }
            Expression::Call { callee, .. }
                if matches!(
                    &callee.node,
                    Expression::FieldAccess { field, .. }
                        if matches!(field.as_str(), "pop" | "remove")
                ) =>
            {
                let Expression::FieldAccess { object, .. } = &callee.node else {
                    unreachable!()
                };
                self.closure_origins(object)
            }
            _ => vec![],
        };
        origins.sort_by(|(left, mutable_left), (right, mutable_right)| {
            left.root
                .0
                .cmp(&right.root.0)
                .then_with(|| left.fields.cmp(&right.fields))
                .then_with(|| mutable_left.cmp(mutable_right))
        });
        origins.dedup();
        origins
    }

    fn ty_contains_function(&self, ty: &Ty) -> bool {
        self.ty_contains_function_inner(ty, &mut HashSet::new())
    }

    fn ty_contains_reference(&self, ty: &Ty) -> bool {
        self.ty_contains_reference_inner(ty, &mut HashSet::new())
    }

    fn instantiated_nominal_ty(&self, name: &str, components: &[(TypeName, Ty)]) -> Ty {
        let generics = self
            .program
            .structs
            .iter()
            .find(|declaration| declaration.name == name)
            .map(|declaration| &declaration.generics)
            .or_else(|| {
                self.program
                    .enums
                    .iter()
                    .find(|declaration| declaration.name == name)
                    .map(|declaration| &declaration.generics)
            });
        let Some(generics) = generics else {
            return Ty::Owned(name.into());
        };
        let mut substitutions = HashMap::new();
        for (template, actual) in components {
            collect_ownership_substitutions(
                &self.ty_from_name(template),
                actual,
                &mut substitutions,
            );
        }
        Ty::Nominal(
            name.into(),
            generics
                .iter()
                .map(|generic| {
                    substitutions
                        .get(&generic.name)
                        .cloned()
                        .unwrap_or_else(|| Ty::Generic(generic.name.clone()))
                })
                .collect(),
        )
    }

    fn nominal_component_types(&self, name: &str, arguments: &[Ty]) -> Vec<Ty> {
        if let Some(declaration) = self
            .program
            .structs
            .iter()
            .find(|declaration| declaration.name == name)
        {
            let substitutions = declaration
                .generics
                .iter()
                .map(|generic| generic.name.clone())
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            return declaration
                .fields
                .iter()
                .map(|field| substitute_ownership_ty(self.ty_from_name(&field.ty), &substitutions))
                .collect();
        }
        if let Some(declaration) = self
            .program
            .enums
            .iter()
            .find(|declaration| declaration.name == name)
        {
            let substitutions = declaration
                .generics
                .iter()
                .map(|generic| generic.name.clone())
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            return declaration
                .variants
                .iter()
                .flat_map(|variant| variant.payload.iter())
                .map(|payload| substitute_ownership_ty(self.ty_from_name(payload), &substitutions))
                .collect();
        }
        Vec::new()
    }

    fn type_name_contains_reference(&self, ty: &TypeName) -> bool {
        self.type_name_contains_reference_inner(ty, &mut HashSet::new())
    }

    fn type_name_contains_reference_inner(
        &self,
        ty: &TypeName,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if matches!(
            ty.qualifier,
            TypeQualifier::SharedReference
                | TypeQualifier::MutableReference
                | TypeQualifier::RawConstPointer
                | TypeQualifier::RawMutPointer
        ) {
            return true;
        }
        if matches!(
            ty.name.as_str(),
            "str" | "[]" | "CStr" | "MemoryPtr" | "MemoryMutPtr"
        ) {
            return true;
        }
        if ty
            .arguments
            .iter()
            .any(|argument| self.type_name_contains_reference_inner(argument, visiting))
        {
            return true;
        }
        if ty.qualifier != TypeQualifier::Owned {
            return false;
        }
        let name = if ty.name == "Self" {
            self.self_type.as_deref().unwrap_or("Self")
        } else {
            &ty.name
        };
        if !visiting.insert(name.to_owned()) {
            return false;
        }
        let contains = self
            .program
            .structs
            .iter()
            .find(|declaration| declaration.name == name)
            .is_some_and(|declaration| {
                declaration
                    .fields
                    .iter()
                    .any(|field| self.type_name_contains_reference_inner(&field.ty, visiting))
            })
            || self
                .program
                .enums
                .iter()
                .find(|declaration| declaration.name == name)
                .is_some_and(|declaration| {
                    declaration.variants.iter().any(|variant| {
                        variant.payload.iter().any(|payload| {
                            self.type_name_contains_reference_inner(payload, visiting)
                        })
                    })
                });
        visiting.remove(name);
        contains
    }

    fn ty_contains_mutable_reference(&self, ty: &Ty) -> bool {
        self.ty_contains_mutable_reference_inner(ty, &mut HashSet::new())
    }

    fn ty_contains_mutable_reference_inner(&self, ty: &Ty, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Ty::Reference(_, true) | Ty::RawPointer(_, true) | Ty::MemoryPointer(_, true) => true,
            Ty::Reference(_, false)
            | Ty::RawPointer(_, false)
            | Ty::MemoryPointer(_, false)
            | Ty::Slice(_)
            | Ty::Str
            | Ty::CStr => false,
            Ty::Option(inner)
            | Ty::Array(inner)
            | Ty::List(inner)
            | Ty::Set(inner)
            | Ty::Thread(inner)
            | Ty::Future(inner)
            | Ty::Task(inner)
            | Ty::Mutex(inner)
            | Ty::Channel(inner) => self.ty_contains_mutable_reference_inner(inner, visiting),
            Ty::Map(key, value) | Ty::Result(key, value) => {
                self.ty_contains_mutable_reference_inner(key, visiting)
                    || self.ty_contains_mutable_reference_inner(value, visiting)
            }
            Ty::Nominal(name, arguments) => {
                if arguments
                    .iter()
                    .any(|argument| self.ty_contains_mutable_reference_inner(argument, visiting))
                {
                    return true;
                }
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let contains = self
                    .nominal_component_types(name, arguments)
                    .iter()
                    .any(|component| self.ty_contains_mutable_reference_inner(component, visiting));
                visiting.remove(name);
                contains
            }
            Ty::Owned(name) => {
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let contains = self
                    .program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name)
                    .is_some_and(|declaration| {
                        declaration.fields.iter().any(|field| {
                            self.ty_contains_mutable_reference_inner(
                                &self.ty_from_name(&field.ty),
                                visiting,
                            )
                        })
                    })
                    || self
                        .program
                        .enums
                        .iter()
                        .find(|declaration| declaration.name == *name)
                        .is_some_and(|declaration| {
                            declaration.variants.iter().any(|variant| {
                                variant.payload.iter().any(|payload| {
                                    self.ty_contains_mutable_reference_inner(
                                        &self.ty_from_name(payload),
                                        visiting,
                                    )
                                })
                            })
                        });
                visiting.remove(name);
                contains
            }
            _ => false,
        }
    }

    fn ty_contains_reference_inner(&self, ty: &Ty, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Ty::Reference(_, _)
            | Ty::RawPointer(_, _)
            | Ty::MemoryPointer(_, _)
            | Ty::Slice(_)
            | Ty::Str
            | Ty::CStr => true,
            Ty::Option(inner)
            | Ty::Array(inner)
            | Ty::List(inner)
            | Ty::Set(inner)
            | Ty::Thread(inner)
            | Ty::Future(inner)
            | Ty::Task(inner)
            | Ty::Mutex(inner)
            | Ty::Channel(inner) => self.ty_contains_reference_inner(inner, visiting),
            Ty::Map(key, value) | Ty::Result(key, value) => {
                self.ty_contains_reference_inner(key, visiting)
                    || self.ty_contains_reference_inner(value, visiting)
            }
            Ty::Nominal(name, arguments) => {
                if arguments
                    .iter()
                    .any(|argument| self.ty_contains_reference_inner(argument, visiting))
                {
                    return true;
                }
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let contains = self
                    .nominal_component_types(name, arguments)
                    .iter()
                    .any(|component| self.ty_contains_reference_inner(component, visiting));
                visiting.remove(name);
                contains
            }
            Ty::Owned(name) => {
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let contains = self
                    .program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name)
                    .is_some_and(|declaration| {
                        declaration.fields.iter().any(|field| {
                            self.ty_contains_reference_inner(
                                &self.ty_from_name(&field.ty),
                                visiting,
                            )
                        })
                    })
                    || self
                        .program
                        .enums
                        .iter()
                        .find(|declaration| declaration.name == *name)
                        .is_some_and(|declaration| {
                            declaration.variants.iter().any(|variant| {
                                variant.payload.iter().any(|payload| {
                                    self.ty_contains_reference_inner(
                                        &self.ty_from_name(payload),
                                        visiting,
                                    )
                                })
                            })
                        });
                visiting.remove(name);
                contains
            }
            _ => false,
        }
    }

    fn ty_contains_function_inner(&self, ty: &Ty, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Ty::Function | Ty::Generic(_) => true,
            Ty::Reference(inner, _)
            | Ty::RawPointer(inner, _)
            | Ty::MemoryPointer(inner, _)
            | Ty::Option(inner)
            | Ty::Array(inner)
            | Ty::Slice(inner)
            | Ty::List(inner)
            | Ty::Set(inner)
            | Ty::Thread(inner)
            | Ty::Future(inner)
            | Ty::Task(inner)
            | Ty::Mutex(inner)
            | Ty::MutexGuard(inner)
            | Ty::Channel(inner) => self.ty_contains_function_inner(inner, visiting),
            Ty::Map(key, value) | Ty::Result(key, value) => {
                self.ty_contains_function_inner(key, visiting)
                    || self.ty_contains_function_inner(value, visiting)
            }
            Ty::Nominal(name, arguments) => {
                if arguments
                    .iter()
                    .any(|argument| self.ty_contains_function_inner(argument, visiting))
                {
                    return true;
                }
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let contains = self
                    .nominal_component_types(name, arguments)
                    .iter()
                    .any(|component| self.ty_contains_function_inner(component, visiting));
                visiting.remove(name);
                contains
            }
            Ty::Owned(name) => {
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let contains = self
                    .program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name)
                    .is_some_and(|declaration| {
                        declaration.fields.iter().any(|field| {
                            self.ty_contains_function_inner(&self.ty_from_name(&field.ty), visiting)
                        })
                    })
                    || self
                        .program
                        .enums
                        .iter()
                        .find(|declaration| declaration.name == *name)
                        .is_some_and(|declaration| {
                            declaration.variants.iter().any(|variant| {
                                variant.payload.iter().any(|payload| {
                                    self.ty_contains_function_inner(
                                        &self.ty_from_name(payload),
                                        visiting,
                                    )
                                })
                            })
                        });
                visiting.remove(name);
                contains
            }
            _ => false,
        }
    }

    fn attach_borrow_origins(
        &mut self,
        id: SlotId,
        mut origins: Vec<(Place, bool)>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        origins.sort_by(|(left, left_mutable), (right, right_mutable)| {
            left.root
                .0
                .cmp(&right.root.0)
                .then_with(|| left.fields.cmp(&right.fields))
                .then_with(|| left_mutable.cmp(right_mutable))
        });
        origins.dedup();
        if origins.is_empty() {
            return Ok(());
        }
        let moved_borrowers = origins
            .iter()
            .filter(|(_, mutable)| *mutable)
            .filter_map(|(origin, _)| {
                self.loans
                    .iter()
                    .find(|loan| loan.place == *origin)
                    .and_then(|loan| loan.borrower)
            })
            .collect::<HashSet<_>>();
        self.loans.retain(|loan| {
            loan.borrower != Some(id)
                && !loan
                    .borrower
                    .is_some_and(|borrower| moved_borrowers.contains(&borrower))
                && !(loan.borrower.is_none()
                    && origins
                        .iter()
                        .any(|(origin, mutable)| *origin == loan.place && *mutable == loan.mutable))
        });
        for (place, mutable) in &origins {
            self.check_borrow(place, *mutable, span)?;
            self.loans.push(Loan {
                place: place.clone(),
                mutable: *mutable,
                borrower: Some(id),
                at: span,
            });
        }
        let slot = self.slots.get_mut(&id).expect("borrow carrier is live");
        slot.reference_origin = origins.first().map(|(place, _)| place.clone());
        slot.borrow_origins = origins;
        Ok(())
    }

    fn attach_closure_origins(&mut self, id: SlotId, origins: Vec<(Place, bool)>, span: Span) {
        if origins.is_empty() {
            return;
        }
        let source_borrowers = origins
            .iter()
            .filter_map(|(origin, _)| {
                self.loans
                    .iter()
                    .find(|loan| loan.place == *origin)
                    .and_then(|loan| loan.borrower)
            })
            .collect::<HashSet<_>>();
        self.loans.retain(|loan| {
            !source_borrowers.contains(&loan.borrower.unwrap_or(SlotId(usize::MAX)))
                && !(loan.borrower.is_none()
                    && origins.iter().any(|(origin, _)| *origin == loan.place))
        });
        for (place, mutable) in &origins {
            self.loans.push(Loan {
                place: place.clone(),
                mutable: *mutable,
                borrower: Some(id),
                at: span,
            });
        }
        self.slots.get_mut(&id).unwrap().closure_origins = origins;
    }

    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        matched: &Ty,
        origins: &[(Place, bool)],
        span: Span,
    ) -> Result<(), Diagnostic> {
        match pattern {
            Pattern::Binding(name) => {
                let id = self.declare(
                    name,
                    matched.clone(),
                    span,
                    false,
                    true,
                    origins.first().map(|(place, _)| place.clone()),
                )?;
                if self.ty_contains_reference(matched) {
                    self.attach_borrow_origins(id, origins.to_vec(), span)?;
                }
            }
            Pattern::Struct { fields, .. } => {
                let (owner, arguments) = match matched {
                    Ty::Owned(owner) => (Some(owner), &[][..]),
                    Ty::Nominal(owner, arguments) => (Some(owner), arguments.as_slice()),
                    _ => (None, &[][..]),
                };
                let declaration = owner.and_then(|owner| {
                    self.program
                        .structs
                        .iter()
                        .find(|declaration| declaration.name == *owner)
                });
                let substitutions = declaration
                    .map(|declaration| {
                        declaration
                            .generics
                            .iter()
                            .map(|generic| generic.name.clone())
                            .zip(arguments.iter().cloned())
                            .collect::<HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                for field in fields {
                    let field_ty = declaration
                        .and_then(|declaration| {
                            declaration
                                .fields
                                .iter()
                                .find(|candidate| candidate.name == field.name)
                        })
                        .map(|declaration| {
                            substitute_ownership_ty(
                                self.ty_from_name(&declaration.ty),
                                &substitutions,
                            )
                        })
                        .unwrap_or_else(|| Ty::Owned("field".into()));
                    let field_origins = if self.ty_contains_reference(&field_ty) {
                        origins
                    } else {
                        &[]
                    };
                    self.bind_pattern(
                        &field.pattern.node,
                        &field_ty,
                        field_origins,
                        field.pattern.span,
                    )?;
                }
            }
            Pattern::Variant {
                variant, arguments, ..
            } => {
                let payload_types = match matched {
                    Ty::Option(inner) if variant == "Some" => vec![(**inner).clone()],
                    Ty::Result(ok, _) if variant == "Ok" => vec![(**ok).clone()],
                    Ty::Result(_, error) if variant == "Err" => vec![(**error).clone()],
                    Ty::Owned(owner) | Ty::Nominal(owner, _) => {
                        let arguments = match matched {
                            Ty::Nominal(_, arguments) => arguments.as_slice(),
                            _ => &[][..],
                        };
                        self.program
                            .enums
                            .iter()
                            .find(|declaration| declaration.name == *owner)
                            .and_then(|declaration| {
                                let substitutions = declaration
                                    .generics
                                    .iter()
                                    .map(|generic| generic.name.clone())
                                    .zip(arguments.iter().cloned())
                                    .collect::<HashMap<_, _>>();
                                declaration
                                    .variants
                                    .iter()
                                    .find(|candidate| candidate.name == *variant)
                                    .map(|variant| {
                                        variant
                                            .payload
                                            .iter()
                                            .map(|ty| {
                                                substitute_ownership_ty(
                                                    self.ty_from_name(ty),
                                                    &substitutions,
                                                )
                                            })
                                            .collect()
                                    })
                            })
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                for (index, argument) in arguments.iter().enumerate() {
                    let payload = payload_types
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| Ty::Owned("payload".into()));
                    let payload_origins = if self.ty_contains_reference(&payload) {
                        origins
                    } else {
                        &[]
                    };
                    self.bind_pattern(&argument.node, &payload, payload_origins, argument.span)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn find_method(&self, receiver: &Ty, name: &str) -> Option<MethodInfo> {
        if let Ty::Generic(generic) = receiver {
            for trait_name in self.generic_traits.get(generic)? {
                if let Some(method) = self
                    .program
                    .traits
                    .iter()
                    .find(|declaration| declaration.name == *trait_name)
                    .and_then(|declaration| {
                        declaration
                            .methods
                            .iter()
                            .find(|method| method.name == name)
                    })
                {
                    return Some(MethodInfo {
                        asynchronous: method.asynchronous,
                        parameters: method.parameters.clone(),
                        return_type: method.return_type.clone(),
                    });
                }
            }
            return None;
        }
        let receiver_name = match receiver {
            Ty::Owned(name) | Ty::Nominal(name, _) => name,
            Ty::Reference(inner, _) => match &**inner {
                Ty::Owned(name) | Ty::Nominal(name, _) => name,
                _ => return None,
            },
            _ => return None,
        };
        self.program
            .implementations
            .iter()
            .find(|implementation| implementation.target.name == *receiver_name)
            .and_then(|implementation| {
                implementation
                    .methods
                    .iter()
                    .find(|method| method.name == name)
            })
            .map(|method| MethodInfo {
                asynchronous: method.asynchronous,
                parameters: method.parameters.clone(),
                return_type: method.return_type.clone(),
            })
    }

    fn ty_from_name(&self, ty: &TypeName) -> Ty {
        let base = match ty.name.as_str() {
            "fn" => Ty::Function,
            "i8" | "i16" | "i32" | "i64" | "i128" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "int" | "uint" | "f32" | "f64" | "bool" | "char" | "Unit" => Ty::Copy,
            "Option" if ty.arguments.len() == 1 => {
                Ty::Option(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Result" if ty.arguments.len() == 2 => Ty::Result(
                Box::new(self.ty_from_name(&ty.arguments[0])),
                Box::new(self.ty_from_name(&ty.arguments[1])),
            ),
            name if name.starts_with("[;") && ty.arguments.len() == 1 => {
                Ty::Array(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "[]" if ty.arguments.len() == 1 => {
                Ty::Slice(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "List" if ty.arguments.len() == 1 => {
                Ty::List(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Map" if ty.arguments.len() == 2 => Ty::Map(
                Box::new(self.ty_from_name(&ty.arguments[0])),
                Box::new(self.ty_from_name(&ty.arguments[1])),
            ),
            "Set" if ty.arguments.len() == 1 => {
                Ty::Set(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Thread" if ty.arguments.len() == 1 => {
                Ty::Thread(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Future" if ty.arguments.len() == 1 => {
                Ty::Future(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Task" if ty.arguments.len() == 1 => {
                Ty::Task(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Mutex" if ty.arguments.len() == 1 => {
                Ty::Mutex(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "MutexGuard" if ty.arguments.len() == 1 => {
                Ty::MutexGuard(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "Channel" if ty.arguments.len() == 1 => {
                Ty::Channel(Box::new(self.ty_from_name(&ty.arguments[0])))
            }
            "AtomicInt" => Ty::AtomicInt,
            "str" => Ty::Str,
            "CString" => Ty::CString,
            "CStr" => Ty::CStr,
            "CRegistration" => Ty::CRegistration,
            "Memory" => Ty::Memory,
            "MemoryPtr" if ty.arguments.len() == 1 => {
                Ty::MemoryPointer(Box::new(self.ty_from_name(&ty.arguments[0])), false)
            }
            "MemoryMutPtr" if ty.arguments.len() == 1 => {
                Ty::MemoryPointer(Box::new(self.ty_from_name(&ty.arguments[0])), true)
            }
            "CInt" | "CUInt" | "CSize" | "CSSize" | "CChar" | "CUChar" | "CShort" | "CUShort"
            | "CLongLong" | "CULongLong" | "CFloat" | "CDouble" => Ty::Copy,
            "Path" => Ty::Path,
            "Url" => Ty::Url,
            "Json" => Ty::Json,
            "IpAddress" => Ty::IpAddress,
            "SocketAddress" => Ty::SocketAddress,
            "TcpStream" => Ty::TcpStream,
            "TlsStream" => Ty::TlsStream,
            "HttpRequest" => Ty::HttpRequest,
            "HttpResponse" => Ty::HttpResponse,
            "TcpListener" => Ty::TcpListener,
            "UdpSocket" => Ty::UdpSocket,
            "UdpDatagram" => Ty::UdpDatagram,
            "Instant" => Ty::Instant,
            "Duration" => Ty::Duration,
            "Self" if self.self_type.is_some() => {
                Ty::Owned(self.self_type.as_ref().unwrap().clone())
            }
            name if self.generic_copy.contains(name) => Ty::Copy,
            name if self
                .program
                .structs
                .iter()
                .any(|declaration| declaration.name == name)
                || self
                    .program
                    .enums
                    .iter()
                    .any(|declaration| declaration.name == name) =>
            {
                Ty::Nominal(
                    name.into(),
                    ty.arguments
                        .iter()
                        .map(|argument| self.ty_from_name(argument))
                        .collect(),
                )
            }
            name if name.len() == 1 && name.chars().all(char::is_uppercase) => {
                Ty::Generic(name.into())
            }
            name => Ty::Owned(name.into()),
        };
        match ty.qualifier {
            TypeQualifier::Owned => base,
            TypeQualifier::SharedReference => Ty::Reference(Box::new(base), false),
            TypeQualifier::MutableReference => Ty::Reference(Box::new(base), true),
            TypeQualifier::RawConstPointer => Ty::RawPointer(Box::new(base), false),
            TypeQualifier::RawMutPointer => Ty::RawPointer(Box::new(base), true),
        }
    }

    fn ty_is_copy(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Copy | Ty::Reference(_, false) | Ty::RawPointer(_, _) | Ty::MemoryPointer(_, _) => {
                true
            }
            Ty::Reference(_, true) => false,
            Ty::Option(value) => self.ty_is_copy(value),
            Ty::Result(ok, error) => self.ty_is_copy(ok) && self.ty_is_copy(error),
            Ty::Array(element) => self.ty_is_copy(element),
            Ty::Slice(_) | Ty::Str | Ty::CStr | Ty::Instant | Ty::Duration | Ty::IpAddress => true,
            Ty::Path
            | Ty::Url
            | Ty::Json
            | Ty::SocketAddress
            | Ty::TcpStream
            | Ty::TlsStream
            | Ty::HttpRequest
            | Ty::HttpResponse
            | Ty::TcpListener
            | Ty::UdpSocket
            | Ty::UdpDatagram
            | Ty::CString
            | Ty::CRegistration
            | Ty::Memory
            | Ty::List(_)
            | Ty::Map(_, _)
            | Ty::Set(_)
            | Ty::Thread(_)
            | Ty::Future(_)
            | Ty::Task(_)
            | Ty::Mutex(_)
            | Ty::MutexGuard(_)
            | Ty::Channel(_)
            | Ty::AtomicInt
            | Ty::Function => false,
            Ty::Owned(name) => self.copy_types.contains(name),
            Ty::Nominal(name, arguments) => {
                self.copy_types.contains(name)
                    && arguments.iter().all(|argument| self.ty_is_copy(argument))
            }
            Ty::Generic(name) => self.generic_copy.contains(name),
            Ty::Unit => true,
        }
    }

    fn type_name_is_copy(&self, ty: &TypeName, generic_copy: &HashSet<String>) -> bool {
        if ty.qualifier != TypeQualifier::Owned {
            return true;
        }
        match ty.name.as_str() {
            "i8" | "i16" | "i32" | "i64" | "i128" | "u8" | "u16" | "u32" | "u64" | "u128"
            | "int" | "uint" | "f32" | "f64" | "bool" | "char" | "Unit" | "CStr" | "CInt"
            | "CUInt" | "CSize" | "CSSize" | "CChar" | "CUChar" | "CShort" | "CUShort"
            | "CLongLong" | "CULongLong" | "CFloat" | "CDouble" => true,
            "fn" => false,
            "Option" => ty
                .arguments
                .first()
                .is_some_and(|value| self.type_name_is_copy(value, generic_copy)),
            "Result" => ty
                .arguments
                .iter()
                .all(|value| self.type_name_is_copy(value, generic_copy)),
            name => generic_copy.contains(name) || self.copy_types.contains(name),
        }
    }

    fn declare(
        &mut self,
        name: &str,
        ty: Ty,
        defined: Span,
        mutable: bool,
        initialized: bool,
        reference_origin: Option<Place>,
    ) -> Result<SlotId, Diagnostic> {
        if self.scopes.last().unwrap().names.contains_key(name) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("duplicate local `{name}`"),
                defined,
            ));
        }
        let id = SlotId(self.next_slot);
        self.next_slot += 1;
        self.slots.insert(
            id,
            Slot {
                name: name.into(),
                ty,
                mutable,
                defined,
                state: if initialized {
                    InitState::Initialized
                } else {
                    InitState::Uninitialized
                },
                scope_depth: self.scopes.len() - 1,
                parameter: false,
                reference_origin,
                borrow_origins: vec![],
                closure_origins: vec![],
            },
        );
        let scope = self.scopes.last_mut().unwrap();
        scope.names.insert(name.into(), id);
        scope.order.push(id);
        Ok(id)
    }

    fn lookup(&self, name: &str) -> Option<SlotId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.names.get(name).copied())
    }
    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }
    fn pop_scope(&mut self, span: Span, reason: DropReason) {
        let Some(scope) = self.scopes.pop() else {
            return;
        };
        let ids: HashSet<_> = scope.order.iter().copied().collect();
        self.loans.retain(|loan| {
            !loan
                .borrower
                .is_some_and(|borrower| ids.contains(&borrower))
        });
        for id in scope.order.into_iter().rev() {
            if let Some(slot) = self.slots.remove(&id)
                && !self.ty_is_copy(&slot.ty)
                && matches!(
                    slot.state,
                    InitState::Initialized | InitState::Partial { .. }
                )
            {
                self.report.drops.push(DropFact {
                    name: slot.name,
                    declaration: slot.defined,
                    exit: span,
                    reason,
                });
            }
        }
    }

    fn expire_loans(&mut self, index: usize, last_uses: &HashMap<String, usize>) {
        let current_depth = self.scopes.len().saturating_sub(1);
        self.loans.retain(|loan| match loan.borrower {
            Some(id) => self.slots.get(&id).is_some_and(|slot| {
                // A nested branch or loop does not own liveness for a value
                // declared outside it. Its loan is expired by the enclosing
                // block after the control-flow construct, never inside it.
                slot.scope_depth < current_depth
                    || last_uses.get(&slot.name).copied().unwrap_or(0) >= index
            }),
            None => false,
        });
    }

    fn merge_from(&mut self, left: &Self, right: &Self) {
        for (id, slot) in &mut self.slots {
            let (Some(a), Some(b)) = (left.slots.get(id), right.slots.get(id)) else {
                continue;
            };
            slot.state = merge_init(&a.state, &b.state);
            slot.closure_origins = a.closure_origins.clone();
            slot.closure_origins.extend(b.closure_origins.clone());
            slot.closure_origins
                .sort_by(|(left, left_mutable), (right, right_mutable)| {
                    left.root
                        .0
                        .cmp(&right.root.0)
                        .then_with(|| left.fields.cmp(&right.fields))
                        .then_with(|| left_mutable.cmp(right_mutable))
                });
            slot.closure_origins.dedup();
            slot.borrow_origins = a.borrow_origins.clone();
            slot.borrow_origins.extend(b.borrow_origins.clone());
            slot.borrow_origins
                .sort_by(|(left, left_mutable), (right, right_mutable)| {
                    left.root
                        .0
                        .cmp(&right.root.0)
                        .then_with(|| left.fields.cmp(&right.fields))
                        .then_with(|| left_mutable.cmp(right_mutable))
                });
            slot.borrow_origins.dedup();
            slot.reference_origin = slot.borrow_origins.first().map(|(place, _)| place.clone());
        }
        self.loans = left.loans.clone();
        for loan in &right.loans {
            if !self.loans.iter().any(|existing| {
                existing.borrower == loan.borrower
                    && existing.place == loan.place
                    && existing.mutable == loan.mutable
            }) {
                self.loans.push(loan.clone());
            }
        }
    }

    fn merge_loop(&mut self, before: &Self, body: &Self) {
        for (id, slot) in &mut self.slots {
            let (Some(a), Some(b)) = (before.slots.get(id), body.slots.get(id)) else {
                continue;
            };
            slot.state = match (&a.state, &b.state) {
                (InitState::Initialized, InitState::Initialized) => InitState::Initialized,
                (InitState::Uninitialized, _) => InitState::Uninitialized,
                (_, InitState::Moved { at }) => InitState::Moved { at: *at },
                (_, InitState::Partial { fields }) => InitState::Partial {
                    fields: fields.clone(),
                },
                _ => a.state.clone(),
            };
            slot.closure_origins = a.closure_origins.clone();
            slot.closure_origins.extend(b.closure_origins.clone());
            slot.closure_origins
                .sort_by(|(left, left_mutable), (right, right_mutable)| {
                    left.root
                        .0
                        .cmp(&right.root.0)
                        .then_with(|| left.fields.cmp(&right.fields))
                        .then_with(|| left_mutable.cmp(right_mutable))
                });
            slot.closure_origins.dedup();
            slot.borrow_origins = a.borrow_origins.clone();
            slot.borrow_origins.extend(b.borrow_origins.clone());
            slot.borrow_origins
                .sort_by(|(left, left_mutable), (right, right_mutable)| {
                    left.root
                        .0
                        .cmp(&right.root.0)
                        .then_with(|| left.fields.cmp(&right.fields))
                        .then_with(|| left_mutable.cmp(right_mutable))
                });
            slot.borrow_origins.dedup();
            slot.reference_origin = slot.borrow_origins.first().map(|(place, _)| place.clone());
        }
        self.loans = before.loans.clone();
        for loan in &body.loans {
            if !self.loans.iter().any(|existing| {
                existing.borrower == loan.borrower
                    && existing.place == loan.place
                    && existing.mutable == loan.mutable
            }) {
                self.loans.push(loan.clone());
            }
        }
    }

    fn record_live_drops(&mut self, span: Span, reason: DropReason) {
        for scope in self.scopes.iter().rev() {
            for id in scope.order.iter().rev() {
                let slot = &self.slots[id];
                if !self.ty_is_copy(&slot.ty)
                    && matches!(
                        slot.state,
                        InitState::Initialized | InitState::Partial { .. }
                    )
                {
                    self.report.drops.push(DropFact {
                        name: slot.name.clone(),
                        declaration: slot.defined,
                        exit: span,
                        reason,
                    });
                }
            }
        }
    }

    fn moved_error(&self, slot: &Slot, moved: Span, used: Span) -> Diagnostic {
        Diagnostic::new(DiagnosticKind::Type, format!("use of moved value `{}`", slot.name), used)
            .with_help(format!("defined at {}:{}, moved at {}:{}; borrow it instead if ownership transfer is not required", slot.defined.start.line, slot.defined.start.column, moved.start.line, moved.start.column))
    }
    fn error_unknown(&self, name: &str, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("unknown ownership slot `{name}`"),
            span,
        )
    }
}

fn is_numeric_type_name(name: &str) -> bool {
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
            | "float"
            | "f32"
    )
}

fn ty_is_borrowed_view(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Reference(_, _)
            | Ty::RawPointer(_, _)
            | Ty::MemoryPointer(_, _)
            | Ty::Slice(_)
            | Ty::Str
            | Ty::CStr
    )
}

fn places_overlap(left: &Place, right: &Place) -> bool {
    if left.root != right.root {
        return false;
    }
    let common = left.fields.len().min(right.fields.len());
    for index in 0..common {
        let a = &left.fields[index];
        let b = &right.fields[index];
        if a == b {
            continue;
        }
        if a.starts_with('@') || b.starts_with('@') {
            return regions_overlap(a, b);
        }
        return false;
    }
    true
}

fn regions_overlap(left: &str, right: &str) -> bool {
    fn region(value: &str) -> Option<(u128, u128)> {
        if let Some(value) = value.strip_prefix("@i:") {
            let x = value.parse().ok()?;
            return Some((x, x + 1));
        }
        let value = value.strip_prefix("@r:")?;
        let (a, b) = value.split_once(':')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }
    match (region(left), region(right)) {
        (Some((a, b)), Some((x, y))) => a < y && x < b,
        _ => true,
    }
}

fn merge_init(left: &InitState, right: &InitState) -> InitState {
    match (left, right) {
        (InitState::Initialized, InitState::Initialized) => InitState::Initialized,
        (InitState::Uninitialized, _) | (_, InitState::Uninitialized) => InitState::Uninitialized,
        (InitState::Moved { at }, _) | (_, InitState::Moved { at }) => InitState::Moved { at: *at },
        (InitState::Partial { fields: a }, InitState::Partial { fields: b }) => {
            let mut fields = a.clone();
            fields.extend(b.clone());
            InitState::Partial { fields }
        }
        (InitState::Partial { fields }, _) | (_, InitState::Partial { fields }) => {
            InitState::Partial {
                fields: fields.clone(),
            }
        }
    }
}

fn block_last_uses(block: &Block) -> HashMap<String, usize> {
    let mut uses = HashMap::new();
    for (index, statement) in block.statements.iter().enumerate() {
        let mut names = HashSet::new();
        collect_statement_names(&statement.node, &mut names);
        for name in names {
            uses.insert(name, index);
        }
    }
    uses
}

fn collect_statement_names(statement: &Statement, names: &mut HashSet<String>) {
    match statement {
        Statement::Binding { value, .. } => {
            if let Some(value) = value {
                collect_expr_names(value, names);
            }
        }
        Statement::Assignment { name, value, .. } => {
            names.insert(name.clone());
            collect_expr_names(value, names);
        }
        Statement::PlaceAssignment { target, value, .. } => {
            collect_expr_names(target, names);
            collect_expr_names(value, names);
        }
        Statement::Expression(value) | Statement::Return(Some(value)) => {
            collect_expr_names(value, names)
        }
        Statement::Return(None) | Statement::Break | Statement::Continue => {}
        Statement::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_names(condition, names);
            collect_block_names(then_branch, names);
            if let Some(block) = else_branch {
                collect_block_names(block, names);
            }
        }
        Statement::While { condition, body } => {
            collect_expr_names(condition, names);
            collect_block_names(body, names);
        }
        Statement::For {
            start, end, body, ..
        } => {
            collect_expr_names(start, names);
            collect_expr_names(end, names);
            collect_block_names(body, names);
        }
        Statement::ForEach { iterable, body, .. } => {
            collect_expr_names(iterable, names);
            collect_block_names(body, names);
        }
        Statement::Loop(body) | Statement::Unsafe { body, .. } => collect_block_names(body, names),
    }
}
fn collect_block_names(block: &Block, names: &mut HashSet<String>) {
    for statement in &block.statements {
        collect_statement_names(&statement.node, names);
    }
}
fn collect_expr_names(expression: &Expr, names: &mut HashSet<String>) {
    match &expression.node {
        Expression::Array(values) => {
            for value in values {
                collect_expr_names(value, names);
            }
        }
        Expression::DataWrite { value, store, .. } => {
            collect_expr_names(value, names);
            collect_expr_names(store, names);
        }
        Expression::DataStore { path } => {
            if let Some(path) = path {
                collect_expr_names(path, names);
            }
        }
        Expression::DataQuery {
            aggregate,
            store,
            predicate,
            order,
            limit,
            ..
        } => {
            if let Some(aggregate) = aggregate {
                collect_expr_names(aggregate, names);
            }
            collect_expr_names(store, names);
            if let Some(predicate) = predicate {
                collect_expr_names(predicate, names);
            }
            if let Some(order) = order {
                collect_expr_names(&order.key, names);
            }
            if let Some(limit) = limit {
                collect_expr_names(limit, names);
            }
        }
        Expression::DataRemove {
            store, predicate, ..
        } => {
            collect_expr_names(store, names);
            collect_expr_names(predicate, names);
        }
        Expression::Closure {
            parameters, body, ..
        } => {
            names.extend(
                crate::ast::closure_capture_uses(parameters, body)
                    .into_keys()
                    .filter(|name| !parameters.iter().any(|parameter| parameter.name == *name)),
            );
        }
        Expression::Index { object, index } => {
            collect_expr_names(object, names);
            collect_expr_names(index, names);
        }
        Expression::Subslice { object, start, end } => {
            collect_expr_names(object, names);
            collect_expr_names(start, names);
            collect_expr_names(end, names);
        }
        Expression::Identifier(name) => {
            names.insert(name.clone());
        }
        Expression::StructConstruct { fields, .. } => {
            for field in fields {
                collect_expr_names(&field.value, names);
            }
        }
        Expression::FieldAccess { object, .. }
        | Expression::Try(object)
        | Expression::Await(object)
        | Expression::Spawn(object)
        | Expression::Move(object)
        | Expression::Dereference(object)
        | Expression::Unary {
            operand: object, ..
        } => collect_expr_names(object, names),
        Expression::Borrow { target, .. } => collect_expr_names(target, names),
        Expression::Binary { left, right, .. } => {
            collect_expr_names(left, names);
            collect_expr_names(right, names);
        }
        Expression::Call { callee, arguments } => {
            collect_expr_names(callee, names);
            for argument in arguments {
                collect_expr_names(argument, names);
            }
        }
        Expression::Match { value, arms } => {
            collect_expr_names(value, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_names(guard, names);
                }
                collect_expr_names(&arm.value, names);
            }
        }
        Expression::Integer(_)
        | Expression::Float(_)
        | Expression::String(_)
        | Expression::Character(_)
        | Expression::Bool(_) => {}
    }
}
