use crate::ast::{
    BindingKind, Block, Expr, Expression, Function, Pattern, Program, Statement, TypeName,
    TypeQualifier,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ty {
    Copy,
    Owned(String),
    Generic(String),
    Reference(Box<Ty>, bool),
    RawPointer(Box<Ty>, bool),
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
    AtomicInt,
    Str,
    CString,
    CStr,
    Memory,
    Path,
    SocketAddress,
    TcpStream,
    TcpListener,
    UdpSocket,
    UdpDatagram,
    Instant,
    Duration,
    Function,
    Unit,
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
        let return_ty = function
            .return_type
            .as_ref()
            .map(|ty| self.ty_from_name(ty))
            .unwrap_or(Ty::Unit);
        if ty_contains_reference(&return_ty)
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
            if matches!(ty, Ty::Slice(_) | Ty::Str | Ty::CStr) {
                slot.reference_origin = Some(Place {
                    root: id,
                    fields: vec![],
                });
            }
            if matches!(ty, Ty::Function) {
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
                    self.loans.push(Loan {
                        place,
                        mutable: false,
                        borrower: Some(id),
                        at: object.span,
                    });
                    if last_uses.get(name).copied().unwrap_or(index) == index {
                        self.loans.retain(|loan| loan.borrower != Some(id));
                    }
                    return Ok(());
                }
                let closure_origins = value
                    .as_ref()
                    .map(|value| self.closure_origins(value))
                    .unwrap_or_default();
                let (ty, origin) = if let Some(value) = value {
                    let ty = self.check_expr(value, UseMode::Consume)?;
                    (
                        annotation
                            .as_ref()
                            .map(|a| self.ty_from_name(a))
                            .unwrap_or(ty),
                        self.reference_origin(value),
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
                if let Some(origin) = origin
                    && ty_contains_reference(&ty)
                {
                    let mutable = ty_contains_mutable_reference(&ty);
                    self.check_borrow(&origin, mutable, *name_span)?;
                    self.loans.push(Loan {
                        place: origin,
                        mutable,
                        borrower: Some(id),
                        at: *name_span,
                    });
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
                        self.loans.push(Loan {
                            place,
                            mutable: false,
                            borrower: Some(id),
                            at: object.span,
                        });
                        if last_uses.get(name).copied().unwrap_or(index) == index {
                            self.loans.retain(|loan| loan.borrower != Some(id));
                        }
                        return Ok(());
                    }
                    let closure_origins = self.closure_origins(value);
                    let ty = self.check_expr(value, UseMode::Consume)?;
                    let origin = self.reference_origin(value);
                    let id =
                        self.declare(name, ty.clone(), *name_span, true, true, origin.clone())?;
                    self.attach_closure_origins(id, closure_origins, *name_span);
                    if let Some(origin) = origin
                        && ty_contains_reference(&ty)
                    {
                        let mutable = ty_contains_mutable_reference(&ty);
                        self.check_borrow(&origin, mutable, *name_span)?;
                        self.loans.push(Loan {
                            place: origin,
                            mutable,
                            borrower: Some(id),
                            at: *name_span,
                        });
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
                    if ty_contains_reference(&ty) {
                        let direct_reference_parameter = match &value.node {
                            Expression::Identifier(name) => self.lookup(name).is_some_and(|id| {
                                self.slots[&id].parameter
                                    && ty_is_borrowed_view(&self.slots[&id].ty)
                            }),
                            _ => false,
                        };
                        let origin = self.reference_origin(value);
                        let borrowed_reference_parameter = origin.as_ref().is_some_and(|origin| {
                            self.slots[&origin.root].parameter
                                && ty_is_borrowed_view(&self.slots[&origin.root].ty)
                        });
                        if !direct_reference_parameter && !borrowed_reference_parameter {
                            let local = origin.map(|origin| self.slots[&origin.root].name.clone());
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
            Statement::Unsafe(body) => self.check_block(body)?,
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
        self.check_expr(value, UseMode::Consume)?;
        let reference_origin = self.reference_origin(value);
        if let Some(origin) = &reference_origin {
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
        let slot = self.slots.get_mut(&place.root).unwrap();
        if place.fields.is_empty() {
            slot.state = InitState::Initialized;
            slot.reference_origin = reference_origin;
        } else if let InitState::Partial { fields } = &mut slot.state {
            fields.remove(&place.fields[0]);
            if fields.is_empty() {
                slot.state = InitState::Initialized;
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
                Ty::Reference(inner, _) | Ty::RawPointer(inner, _) | Ty::MutexGuard(inner) => {
                    Ok(*inner)
                }
                _ => Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "cannot dereference this value",
                    expression.span,
                )),
            },
            Expression::StructConstruct { name, fields, .. } => {
                for field in fields {
                    self.check_expr(&field.value, UseMode::Consume)?;
                }
                Ok(Ty::Owned(name.clone()))
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
                let matched = self.check_expr(value, UseMode::Consume)?;
                let before = self.clone();
                let mut branch_states = Vec::new();
                let mut result = Ty::Unit;
                for arm in arms {
                    let mut arm_state = before.clone();
                    arm_state.push_scope();
                    arm_state.bind_pattern(&arm.pattern.node, &matched, arm.pattern.span)?;
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
            if let Expression::Identifier(owner) = &object.node
                && matches!(
                    owner.as_str(),
                    "Path" | "File" | "Directory" | "Time" | "Duration"
                )
            {
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
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
                    ("Time", "sleep") => Ty::Unit,
                    ("Duration", _) => Ty::Duration,
                    _ => Ty::Unit,
                });
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
            if matches!(self.expr_ty(object), Ok(Ty::AtomicInt)) {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "share" => Ty::AtomicInt,
                    "store" => Ty::Unit,
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
            if matches!(self.expr_ty(object), Ok(Ty::CStr)) {
                self.check_expr(object, UseMode::Read)?;
                return Ok(if field == "to_string" {
                    Ty::Owned("String".into())
                } else {
                    Ty::Copy
                });
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
                    "as_ptr" => Ty::RawPointer(Box::new(Ty::Copy), false),
                    "as_mut_ptr" => Ty::RawPointer(Box::new(Ty::Copy), true),
                    _ => Ty::Copy,
                });
            }
            if let Ok(Ty::RawPointer(inner, mutable)) = self.expr_ty(object)
                && matches!(field.as_str(), "offset" | "read" | "write")
            {
                self.check_expr(object, UseMode::Read)?;
                for argument in arguments {
                    self.check_expr(argument, UseMode::Read)?;
                }
                return Ok(match field.as_str() {
                    "offset" => Ty::RawPointer(inner, mutable),
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
                for argument in arguments {
                    self.check_expr(argument, UseMode::Consume)?;
                }
                self.loans.truncate(temporary_start);
                return Ok(Ty::Owned(match &object.node {
                    Expression::Identifier(name) => name.clone(),
                    _ => unreachable!(),
                }));
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
                        if let Some(origin) = self.slots[&id].reference_origin.clone() {
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
                    Ty::Reference(inner, _) | Ty::RawPointer(inner, _) | Ty::MutexGuard(inner) => {
                        *inner
                    }
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
            let Ty::Owned(name) = ty else {
                return Ok(Ty::Owned("field".into()));
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
            ty = declaration
                .fields
                .iter()
                .find(|candidate| candidate.name == *field)
                .map(|field| self.ty_from_name(&field.ty))
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
            Expression::StructConstruct { name, .. } => Ok(Ty::Owned(name.clone())),
            _ => Ok(Ty::Owned("expression".into())),
        }
    }

    fn reference_origin(&self, expression: &Expr) -> Option<Place> {
        match &expression.node {
            Expression::Borrow { target, .. } => self.place(target).ok(),
            Expression::Try(operand) => self.reference_origin(operand),
            Expression::Identifier(name) => self
                .lookup(name)
                .and_then(|id| self.slots[&id].reference_origin.clone()),
            Expression::Call { callee, arguments }
                if matches!(&callee.node, Expression::Identifier(name) if matches!(name.as_str(), "Some" | "Ok" | "Err"))
                    && arguments.len() == 1 =>
            {
                self.reference_origin(&arguments[0])
            }
            Expression::Call { callee, arguments }
                if matches!(
                    &callee.node,
                    Expression::FieldAccess { field, .. }
                        if matches!(field.as_str(), "get" | "get_mut")
                ) && arguments.len() == 1 =>
            {
                let Expression::FieldAccess { object, .. } = &callee.node else {
                    unreachable!()
                };
                let mut place = self.place(object).ok()?;
                place.fields.push(match arguments[0].node {
                    Expression::Integer(value) => format!("@i:{value}"),
                    _ => "@i:*".into(),
                });
                Some(place)
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
                self.place(object).ok()
            }
            Expression::Call { callee, arguments } => {
                let Expression::Identifier(name) = &callee.node else {
                    return None;
                };
                let function = self
                    .program
                    .functions
                    .iter()
                    .find(|function| function.name == *name)?;
                let return_ty = function
                    .return_type
                    .as_ref()
                    .map(|ty| self.ty_from_name(ty))
                    .unwrap_or(Ty::Unit);
                if !ty_contains_reference(&return_ty) {
                    return None;
                }
                let (index, _) =
                    function
                        .parameters
                        .iter()
                        .enumerate()
                        .find(|(_, parameter)| {
                            ty_is_borrowed_view(&self.ty_from_name(&parameter.ty))
                        })?;
                let argument = arguments.get(index)?;
                self.reference_origin(argument)
                    .or_else(|| self.place(argument).ok())
            }
            _ => None,
        }
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
                    if ty_contains_reference(&slot.ty) {
                        return vec![(
                            slot.reference_origin.clone().unwrap_or(Place {
                                root,
                                fields: vec![],
                            }),
                            usage.mutated || ty_contains_mutable_reference(&slot.ty),
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

    fn ty_contains_function_inner(&self, ty: &Ty, visiting: &mut HashSet<String>) -> bool {
        match ty {
            Ty::Function | Ty::Generic(_) => true,
            Ty::Reference(inner, _)
            | Ty::RawPointer(inner, _)
            | Ty::Option(inner)
            | Ty::Array(inner)
            | Ty::Slice(inner)
            | Ty::List(inner)
            | Ty::Set(inner)
            | Ty::Thread(inner)
            | Ty::Future(inner)
            | Ty::Task(inner)
            | Ty::Mutex(inner)
            | Ty::MutexGuard(inner) => self.ty_contains_function_inner(inner, visiting),
            Ty::Map(key, value) | Ty::Result(key, value) => {
                self.ty_contains_function_inner(key, visiting)
                    || self.ty_contains_function_inner(value, visiting)
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
        span: Span,
    ) -> Result<(), Diagnostic> {
        match pattern {
            Pattern::Binding(name) => {
                self.declare(name, matched.clone(), span, false, true, None)?;
            }
            Pattern::Variant {
                variant, arguments, ..
            } => {
                let payload_types = match matched {
                    Ty::Option(inner) if variant == "Some" => vec![(**inner).clone()],
                    Ty::Result(ok, _) if variant == "Ok" => vec![(**ok).clone()],
                    Ty::Result(_, error) if variant == "Err" => vec![(**error).clone()],
                    Ty::Owned(owner) => self
                        .program
                        .enums
                        .iter()
                        .find(|declaration| declaration.name == *owner)
                        .and_then(|declaration| {
                            declaration
                                .variants
                                .iter()
                                .find(|candidate| candidate.name == *variant)
                        })
                        .map(|variant| {
                            variant
                                .payload
                                .iter()
                                .map(|ty| self.ty_from_name(ty))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                for (index, argument) in arguments.iter().enumerate() {
                    let payload = payload_types
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| Ty::Owned("payload".into()));
                    self.bind_pattern(&argument.node, &payload, argument.span)?;
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
            Ty::Owned(name) => name,
            Ty::Reference(inner, _) => match &**inner {
                Ty::Owned(name) => name,
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
            "AtomicInt" => Ty::AtomicInt,
            "str" => Ty::Str,
            "CString" => Ty::CString,
            "CStr" => Ty::CStr,
            "Memory" => Ty::Memory,
            "CInt" | "CUInt" | "CSize" | "CSSize" | "CChar" | "CUChar" | "CShort" | "CUShort"
            | "CLongLong" | "CULongLong" | "CFloat" | "CDouble" => Ty::Copy,
            "Path" => Ty::Path,
            "SocketAddress" => Ty::SocketAddress,
            "TcpStream" => Ty::TcpStream,
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
                Ty::Owned(name.into())
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
            Ty::Copy | Ty::Reference(_, false) | Ty::RawPointer(_, _) => true,
            Ty::Reference(_, true) => false,
            Ty::Option(value) => self.ty_is_copy(value),
            Ty::Result(ok, error) => self.ty_is_copy(ok) && self.ty_is_copy(error),
            Ty::Array(element) => self.ty_is_copy(element),
            Ty::Slice(_) | Ty::Str | Ty::CStr | Ty::Instant | Ty::Duration => true,
            Ty::Path
            | Ty::SocketAddress
            | Ty::TcpStream
            | Ty::TcpListener
            | Ty::UdpSocket
            | Ty::UdpDatagram
            | Ty::CString
            | Ty::Memory
            | Ty::List(_)
            | Ty::Map(_, _)
            | Ty::Set(_)
            | Ty::Thread(_)
            | Ty::Future(_)
            | Ty::Task(_)
            | Ty::Mutex(_)
            | Ty::MutexGuard(_)
            | Ty::AtomicInt
            | Ty::Function => false,
            Ty::Owned(name) => self.copy_types.contains(name),
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

fn ty_contains_reference(ty: &Ty) -> bool {
    match ty {
        Ty::Reference(_, _) | Ty::Slice(_) | Ty::Str | Ty::CStr => true,
        Ty::Option(inner)
        | Ty::Array(inner)
        | Ty::List(inner)
        | Ty::Set(inner)
        | Ty::Thread(inner)
        | Ty::Future(inner)
        | Ty::Task(inner)
        | Ty::Mutex(inner) => ty_contains_reference(inner),
        Ty::MutexGuard(_) => false,
        Ty::Map(key, value) => ty_contains_reference(key) || ty_contains_reference(value),
        Ty::Result(ok, error) => ty_contains_reference(ok) || ty_contains_reference(error),
        _ => false,
    }
}

fn ty_is_borrowed_view(ty: &Ty) -> bool {
    matches!(ty, Ty::Reference(_, _) | Ty::Slice(_) | Ty::Str | Ty::CStr)
}

fn ty_contains_mutable_reference(ty: &Ty) -> bool {
    match ty {
        Ty::Reference(_, true) => true,
        Ty::Option(inner)
        | Ty::Array(inner)
        | Ty::List(inner)
        | Ty::Set(inner)
        | Ty::Thread(inner)
        | Ty::Future(inner)
        | Ty::Task(inner)
        | Ty::Mutex(inner) => ty_contains_mutable_reference(inner),
        Ty::Map(key, value) | Ty::Result(key, value) => {
            ty_contains_mutable_reference(key) || ty_contains_mutable_reference(value)
        }
        _ => false,
    }
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
        Statement::Loop(body) | Statement::Unsafe(body) => collect_block_names(body, names),
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
