use crate::ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use crate::hir;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub usize);

#[derive(Debug, Clone)]
pub struct Program {
    pub functions: Vec<Function>,
    pub structs: Vec<hir::Struct>,
    pub enums: Vec<hir::Enum>,
    pub implementations: Vec<hir::Implementation>,
    pub traits: Vec<hir::Trait>,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub id: hir::FunctionId,
    pub name: String,
    pub locals: Vec<Local>,
    pub argument_count: usize,
    pub return_local: LocalId,
    pub blocks: Vec<BasicBlock>,
    pub span: Span,
    pub generic_parameters: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub id: LocalId,
    pub name: String,
    pub ty: hir::Type,
    pub kind: LocalKind,
    pub span: Span,
    pub needs_drop: bool,
    pub mutable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    Return,
    Argument,
    User,
    Temporary,
    DropFlag,
}

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub statements: Vec<Statement>,
    pub terminator: Terminator,
}

#[derive(Debug, Clone)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StatementKind {
    StorageLive(LocalId),
    StorageDead(LocalId),
    Assign(Place, Rvalue),
    SetDropFlag { local: LocalId, initialized: bool },
    Drop { place: Place, flag: Option<LocalId> },
    Nop,
}

#[derive(Debug, Clone)]
pub enum Terminator {
    Goto(BlockId),
    SwitchBool {
        condition: Operand,
        true_block: BlockId,
        false_block: BlockId,
    },
    SwitchValue {
        discriminant: Operand,
        targets: Vec<(Constant, BlockId)>,
        otherwise: BlockId,
    },
    SwitchEnum {
        discriminant: Operand,
        targets: Vec<(hir::VariantId, BlockId)>,
        otherwise: BlockId,
    },
    Call {
        target: hir::CallTarget,
        arguments: Vec<Operand>,
        destination: Place,
        next: BlockId,
        unwind: Option<BlockId>,
        substitutions: Vec<hir::Type>,
        span: Span,
    },
    Spawn {
        target: hir::FunctionId,
        arguments: Vec<Operand>,
        destination: Place,
        next: BlockId,
        substitutions: Vec<hir::Type>,
        span: Span,
    },
    Return,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    pub local: LocalId,
    pub projections: Vec<Projection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Projection {
    Field(usize),
    SafeDereference,
    RawDereference,
    VariantField(hir::VariantId, usize),
    Index {
        index: LocalId,
        span: Span,
    },
    Subslice {
        start: LocalId,
        end: LocalId,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum Operand {
    Move(Place),
    Copy(Place),
    Constant(Constant),
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
pub enum Rvalue {
    Use(Operand),
    UnaryOp(UnaryOperator, Operand),
    BinaryOp(BinaryOperator, Operand, Operand),
    Aggregate(AggregateKind, Vec<Operand>),
    BorrowShared(Place),
    BorrowMut(Place),
    RawAddress { mutable: bool, place: Place },
    Discriminant(Place),
    Len(Place),
    Cast { operand: Operand, target: hir::Type },
}

#[derive(Debug, Clone)]
pub enum AggregateKind {
    Array,
    Struct(hir::StructId),
    Enum(hir::EnumId, hir::VariantId),
}

pub fn lower(program: &hir::Program) -> Result<Program, Diagnostic> {
    let functions = program
        .functions
        .iter()
        .map(|function| Builder::new(program, function).lower())
        .collect::<Result<Vec<_>, _>>()?;
    let mir = Program {
        functions,
        structs: program.structs.clone(),
        enums: program.enums.clone(),
        implementations: program.implementations.clone(),
        traits: program.traits.clone(),
    };
    validate(&mir)?;
    Ok(mir)
}

struct LoopFrame {
    break_block: BlockId,
    continue_block: BlockId,
    live_depth: usize,
}

struct Builder<'a> {
    program: &'a hir::Program,
    function: &'a hir::Function,
    locals: Vec<Local>,
    source_locals: HashMap<hir::LocalId, LocalId>,
    drop_flags: HashMap<Place, LocalId>,
    blocks: Vec<BasicBlock>,
    current: BlockId,
    live: Vec<LocalId>,
    loops: Vec<LoopFrame>,
}

impl<'a> Builder<'a> {
    fn new(program: &'a hir::Program, function: &'a hir::Function) -> Self {
        let return_local = Local {
            id: LocalId(0),
            name: "_return".into(),
            ty: function.return_type.clone(),
            kind: LocalKind::Return,
            span: function.span,
            needs_drop: !function.return_type.is_copy(program),
            mutable: true,
        };
        let mut locals = vec![return_local];
        let mut source_locals = HashMap::new();
        for local in &function.locals {
            let id = LocalId(locals.len());
            source_locals.insert(local.id, id);
            locals.push(Local {
                id,
                name: local.name.clone(),
                ty: local.ty.clone(),
                kind: if local.parameter {
                    LocalKind::Argument
                } else {
                    LocalKind::User
                },
                span: local.span,
                needs_drop: !local.ty.is_copy(program),
                mutable: local.mutable,
            });
        }
        Self {
            program,
            function,
            locals,
            source_locals,
            drop_flags: HashMap::new(),
            blocks: vec![BasicBlock {
                statements: Vec::new(),
                terminator: Terminator::Unreachable,
            }],
            current: BlockId(0),
            live: Vec::new(),
            loops: Vec::new(),
        }
    }

    fn lower(mut self) -> Result<Function, Diagnostic> {
        for parameter in &self.function.parameters {
            let local = self.source_locals[parameter];
            self.live.push(local);
            self.ensure_drop_flag(local);
        }
        self.lower_block(&self.function.body, false)?;
        if self.open(self.current) {
            self.emit_drops_to(0, self.function.span);
            self.terminate(Terminator::Return);
        }
        Ok(Function {
            id: self.function.id,
            name: self.function.name.clone(),
            locals: self.locals,
            argument_count: self.function.parameters.len(),
            return_local: LocalId(0),
            blocks: self.blocks,
            span: self.function.span,
            generic_parameters: self.function.generic_parameters.clone(),
        })
    }

    fn lower_block(&mut self, block: &hir::Block, scoped: bool) -> Result<(), Diagnostic> {
        let depth = self.live.len();
        for statement in &block.statements {
            if !self.open(self.current) {
                self.current = self.new_block();
            }
            self.lower_statement(statement)?;
        }
        if scoped && self.open(self.current) {
            self.emit_drops_to(depth, block.span);
        }
        if scoped {
            self.live.truncate(depth);
        }
        Ok(())
    }

    fn lower_statement(&mut self, statement: &hir::Statement) -> Result<(), Diagnostic> {
        match &statement.kind {
            hir::StatementKind::Let { local, value } => {
                let local = self.source_locals[local];
                self.push(StatementKind::StorageLive(local), statement.span);
                self.live.push(local);
                self.ensure_drop_flag(local);
                if let Some(value) = value {
                    let operand = self.lower_expr(value)?;
                    self.push(
                        StatementKind::Assign(self.place(local), Rvalue::Use(operand.clone())),
                        statement.span,
                    );
                    self.consume_operand(&operand, statement.span);
                    self.set_initialized(local, true, statement.span);
                }
            }
            hir::StatementKind::Assign {
                target,
                operator,
                value,
            } => {
                let target = self.place_from_hir(target)?;
                let right = self.lower_expr(value)?;
                let rvalue = if *operator == AssignmentOperator::Assign {
                    Rvalue::Use(right.clone())
                } else {
                    let left = Operand::Copy(target.clone());
                    let operator = match operator {
                        AssignmentOperator::Add => BinaryOperator::Add,
                        AssignmentOperator::Subtract => BinaryOperator::Subtract,
                        AssignmentOperator::Multiply => BinaryOperator::Multiply,
                        AssignmentOperator::Divide => BinaryOperator::Divide,
                        AssignmentOperator::Assign => unreachable!(),
                    };
                    Rvalue::BinaryOp(operator, left, right.clone())
                };
                if *operator == AssignmentOperator::Assign {
                    self.emit_drop_for_place(&target, statement.span);
                }
                self.push(
                    StatementKind::Assign(target.clone(), rvalue),
                    statement.span,
                );
                self.consume_operand(&right, statement.span);
                self.set_place_initialized(&target, true, statement.span);
            }
            hir::StatementKind::Expression(expr) => {
                self.lower_expr(expr)?;
            }
            hir::StatementKind::Return(value) => {
                if let Some(value) = value {
                    let value = self.lower_expr(value)?;
                    self.push(
                        StatementKind::Assign(self.place(LocalId(0)), Rvalue::Use(value.clone())),
                        statement.span,
                    );
                    self.consume_operand(&value, statement.span);
                }
                self.emit_drops_to(0, statement.span);
                self.terminate(Terminator::Return);
            }
            hir::StatementKind::If {
                condition,
                then_block,
                else_block,
            } => {
                let condition = self.lower_expr(condition)?;
                let then_id = self.new_block();
                let else_id = self.new_block();
                let join = self.new_block();
                self.terminate(Terminator::SwitchBool {
                    condition,
                    true_block: then_id,
                    false_block: else_id,
                });
                self.current = then_id;
                self.lower_block(then_block, true)?;
                if self.open(self.current) {
                    self.terminate(Terminator::Goto(join));
                }
                self.current = else_id;
                if let Some(block) = else_block {
                    self.lower_block(block, true)?;
                }
                if self.open(self.current) {
                    self.terminate(Terminator::Goto(join));
                }
                self.current = join;
            }
            hir::StatementKind::While { condition, body } => {
                let header = self.new_block();
                let body_id = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Goto(header));
                self.current = header;
                let condition = self.lower_expr(condition)?;
                self.terminate(Terminator::SwitchBool {
                    condition,
                    true_block: body_id,
                    false_block: exit,
                });
                self.loops.push(LoopFrame {
                    break_block: exit,
                    continue_block: header,
                    live_depth: self.live.len(),
                });
                self.current = body_id;
                self.lower_block(body, true)?;
                if self.open(self.current) {
                    self.terminate(Terminator::Goto(header));
                }
                self.loops.pop();
                self.current = exit;
            }
            hir::StatementKind::Loop(body) => {
                let header = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Goto(header));
                self.loops.push(LoopFrame {
                    break_block: exit,
                    continue_block: header,
                    live_depth: self.live.len(),
                });
                self.current = header;
                self.lower_block(body, true)?;
                if self.open(self.current) {
                    self.terminate(Terminator::Goto(header));
                }
                self.loops.pop();
                self.current = exit;
            }
            hir::StatementKind::For {
                local,
                start,
                end,
                inclusive,
                body,
            } => {
                let local = self.source_locals[local];
                self.push(StatementKind::StorageLive(local), statement.span);
                self.live.push(local);
                let start = self.lower_expr(start)?;
                self.push(
                    StatementKind::Assign(self.place(local), Rvalue::Use(start)),
                    statement.span,
                );
                let end_temp = self.temp(self.locals[local.0].ty.clone(), statement.span);
                let end = self.lower_expr(end)?;
                self.push(
                    StatementKind::Assign(self.place(end_temp), Rvalue::Use(end)),
                    statement.span,
                );
                let header = self.new_block();
                let body_id = self.new_block();
                let step = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Goto(header));
                self.current = header;
                let cmp = self.temp(hir::Type::Bool, statement.span);
                self.push(
                    StatementKind::Assign(
                        self.place(cmp),
                        Rvalue::BinaryOp(
                            if *inclusive {
                                BinaryOperator::LessEqual
                            } else {
                                BinaryOperator::Less
                            },
                            Operand::Copy(self.place(local)),
                            Operand::Copy(self.place(end_temp)),
                        ),
                    ),
                    statement.span,
                );
                self.terminate(Terminator::SwitchBool {
                    condition: Operand::Copy(self.place(cmp)),
                    true_block: body_id,
                    false_block: exit,
                });
                self.loops.push(LoopFrame {
                    break_block: exit,
                    continue_block: step,
                    live_depth: self.live.len(),
                });
                self.current = body_id;
                self.lower_block(body, true)?;
                if self.open(self.current) {
                    self.terminate(Terminator::Goto(step));
                }
                self.current = step;
                self.push(
                    StatementKind::Assign(
                        self.place(local),
                        Rvalue::BinaryOp(
                            BinaryOperator::Add,
                            Operand::Copy(self.place(local)),
                            Operand::Constant(Constant::Signed(1, None)),
                        ),
                    ),
                    statement.span,
                );
                self.terminate(Terminator::Goto(header));
                self.loops.pop();
                self.current = exit;
            }
            hir::StatementKind::ForEach {
                local,
                iterable,
                body,
            } => {
                let item = self.source_locals[local];
                self.push(StatementKind::StorageLive(item), statement.span);
                self.live.push(item);
                let mut collection = if let Some(place) = self.expr_place(iterable) {
                    place
                } else {
                    let operand = self.lower_expr(iterable)?;
                    let temporary = self.materialize(operand, iterable.ty.clone(), iterable.span);
                    self.place(temporary)
                };
                if matches!(iterable.ty, hir::Type::Reference { .. }) {
                    collection.projections.push(Projection::SafeDereference);
                }
                let usize_ty = hir::Type::Int {
                    signed: false,
                    width: None,
                };
                let index = self.temp(usize_ty.clone(), statement.span);
                self.push(
                    StatementKind::Assign(
                        self.place(index),
                        Rvalue::Use(Operand::Constant(Constant::Unsigned(0, None))),
                    ),
                    statement.span,
                );
                let length = self.temp(usize_ty, statement.span);
                self.push(
                    StatementKind::Assign(self.place(length), Rvalue::Len(collection.clone())),
                    statement.span,
                );
                let header = self.new_block();
                let body_id = self.new_block();
                let step = self.new_block();
                let exit = self.new_block();
                self.terminate(Terminator::Goto(header));
                self.current = header;
                let condition = self.temp(hir::Type::Bool, statement.span);
                self.push(
                    StatementKind::Assign(
                        self.place(condition),
                        Rvalue::BinaryOp(
                            BinaryOperator::Less,
                            Operand::Copy(self.place(index)),
                            Operand::Copy(self.place(length)),
                        ),
                    ),
                    statement.span,
                );
                self.terminate(Terminator::SwitchBool {
                    condition: Operand::Copy(self.place(condition)),
                    true_block: body_id,
                    false_block: exit,
                });
                self.loops.push(LoopFrame {
                    break_block: exit,
                    continue_block: step,
                    live_depth: self.live.len(),
                });
                self.current = body_id;
                let mut element = collection;
                element.projections.push(Projection::Index {
                    index,
                    span: iterable.span,
                });
                let item_value = match &self.locals[item.0].ty {
                    hir::Type::Reference { .. } => Rvalue::BorrowShared(element),
                    _ => Rvalue::Use(Operand::Copy(element)),
                };
                self.push(
                    StatementKind::Assign(self.place(item), item_value),
                    statement.span,
                );
                self.lower_block(body, true)?;
                if self.open(self.current) {
                    self.terminate(Terminator::Goto(step));
                }
                self.current = step;
                self.push(
                    StatementKind::Assign(
                        self.place(index),
                        Rvalue::BinaryOp(
                            BinaryOperator::Add,
                            Operand::Copy(self.place(index)),
                            Operand::Constant(Constant::Unsigned(1, None)),
                        ),
                    ),
                    statement.span,
                );
                self.terminate(Terminator::Goto(header));
                self.loops.pop();
                self.current = exit;
                self.live.pop();
                self.push(StatementKind::StorageDead(item), statement.span);
            }
            hir::StatementKind::Unsafe(block) => self.lower_block(block, true)?,
            hir::StatementKind::Break | hir::StatementKind::Continue => {
                let frame = self.loops.last().ok_or_else(|| {
                    Diagnostic::new(
                        DiagnosticKind::Internal,
                        "MIR lowering lost loop context",
                        statement.span,
                    )
                })?;
                let target = if matches!(statement.kind, hir::StatementKind::Break) {
                    frame.break_block
                } else {
                    frame.continue_block
                };
                let depth = frame.live_depth;
                self.emit_drops_to(depth, statement.span);
                self.terminate(Terminator::Goto(target));
            }
        }
        Ok(())
    }

    fn lower_expr(&mut self, expr: &hir::Expr) -> Result<Operand, Diagnostic> {
        match &expr.kind {
            hir::ExprKind::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| self.lower_expr(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let consumed = values.clone();
                let temporary = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(
                        self.place(temporary),
                        Rvalue::Aggregate(AggregateKind::Array, values),
                    ),
                    expr.span,
                );
                for value in &consumed {
                    self.consume_operand(value, expr.span);
                }
                self.set_initialized(temporary, true, expr.span);
                Ok(Operand::Move(self.place(temporary)))
            }
            hir::ExprKind::Constant(x) => Ok(Operand::Constant(lower_constant(x))),
            hir::ExprKind::Local(x) => {
                let local = self.source_locals[x];
                Ok(if expr.ty.is_copy(self.program) {
                    Operand::Copy(self.place(local))
                } else {
                    self.set_initialized(local, false, expr.span);
                    Operand::Move(self.place(local))
                })
            }
            hir::ExprKind::Move(place) => {
                let place = self.place_from_hir(place)?;
                self.set_place_initialized(&place, false, expr.span);
                Ok(Operand::Move(place))
            }
            hir::ExprKind::Borrow { mutable, place } => {
                let mut borrowed = self.place_from_hir(place)?;
                if matches!(
                    borrowed.projections.last(),
                    Some(Projection::Subslice { .. })
                ) {
                    let inner = match &expr.ty {
                        hir::Type::Reference { inner, .. } => (**inner).clone(),
                        _ => unreachable!(),
                    };
                    let temporary = self.temp(inner, expr.span);
                    self.push(
                        StatementKind::Assign(
                            self.place(temporary),
                            Rvalue::Use(Operand::Copy(borrowed)),
                        ),
                        expr.span,
                    );
                    borrowed = self.place(temporary);
                }
                let result = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(
                        self.place(result),
                        if *mutable {
                            Rvalue::BorrowMut(borrowed)
                        } else {
                            Rvalue::BorrowShared(borrowed)
                        },
                    ),
                    expr.span,
                );
                Ok(Operand::Copy(self.place(result)))
            }
            hir::ExprKind::Index { object, index } => {
                let mut place = if let Some(place) = self.expr_place(object) {
                    place
                } else {
                    let operand = self.lower_expr(object)?;
                    let local = self.materialize(operand, object.ty.clone(), object.span);
                    self.place(local)
                };
                let index_operand = self.lower_expr(index)?;
                let index_local = self.materialize(index_operand, index.ty.clone(), index.span);
                place.projections.push(Projection::Index {
                    index: index_local,
                    span: index.span,
                });
                Ok(if expr.ty.is_copy(self.program) {
                    Operand::Copy(place)
                } else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "moving a non-Copy element through dynamic indexing is not yet permitted",
                        expr.span,
                    ));
                })
            }
            hir::ExprKind::Subslice { object, start, end } => {
                let mut place = if let Some(place) = self.expr_place(object) {
                    place
                } else {
                    let operand = self.lower_expr(object)?;
                    let local = self.materialize(operand, object.ty.clone(), object.span);
                    self.place(local)
                };
                let start_operand = self.lower_expr(start)?;
                let start_local = self.materialize(start_operand, start.ty.clone(), start.span);
                let end_operand = self.lower_expr(end)?;
                let end_local = self.materialize(end_operand, end.ty.clone(), end.span);
                place.projections.push(Projection::Subslice {
                    start: start_local,
                    end: end_local,
                    span: expr.span,
                });
                Ok(Operand::Copy(place))
            }
            hir::ExprKind::Dereference(value, raw) => {
                let operand = self.lower_expr(value)?;
                let source = self.materialize(operand, value.ty.clone(), value.span);
                let mut place = self.place(source);
                place.projections.push(if *raw {
                    Projection::RawDereference
                } else {
                    Projection::SafeDereference
                });
                if *raw {
                    let result = self.temp(expr.ty.clone(), expr.span);
                    self.push(
                        StatementKind::Assign(
                            self.place(result),
                            Rvalue::Use(Operand::Copy(place)),
                        ),
                        expr.span,
                    );
                    Ok(Operand::Copy(self.place(result)))
                } else {
                    Ok(if expr.ty.is_copy(self.program) {
                        Operand::Copy(place)
                    } else {
                        Operand::Move(place)
                    })
                }
            }
            hir::ExprKind::Field { object, index } => {
                let mut place = if let Some(place) = self.expr_place(object) {
                    place
                } else {
                    let operand = self.lower_expr(object)?;
                    let local = self.materialize(operand, object.ty.clone(), object.span);
                    self.place(local)
                };
                if matches!(object.ty, hir::Type::Reference { .. }) {
                    place.projections.push(Projection::SafeDereference);
                }
                place.projections.push(Projection::Field(*index));
                Ok(if expr.ty.is_copy(self.program) {
                    Operand::Copy(place)
                } else {
                    self.set_place_initialized(&place, false, expr.span);
                    Operand::Move(place)
                })
            }
            hir::ExprKind::Unary { operator, operand } => {
                let operand = self.lower_expr(operand)?;
                let result = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(self.place(result), Rvalue::UnaryOp(*operator, operand)),
                    expr.span,
                );
                Ok(Operand::Copy(self.place(result)))
            }
            hir::ExprKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.lower_expr(left)?;
                let right = self.lower_expr(right)?;
                let result = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(
                        self.place(result),
                        Rvalue::BinaryOp(*operator, left, right),
                    ),
                    expr.span,
                );
                Ok(Operand::Copy(self.place(result)))
            }
            hir::ExprKind::Struct { id, fields } => {
                let mut values = Vec::new();
                for (_, field) in fields {
                    values.push(self.lower_expr(field)?);
                }
                let result = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(
                        self.place(result),
                        Rvalue::Aggregate(AggregateKind::Struct(*id), values),
                    ),
                    expr.span,
                );
                self.set_initialized(result, true, expr.span);
                Ok(Operand::Move(self.place(result)))
            }
            hir::ExprKind::Variant {
                enum_id,
                variant_id,
            } => {
                let result = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(
                        self.place(result),
                        Rvalue::Aggregate(AggregateKind::Enum(*enum_id, *variant_id), vec![]),
                    ),
                    expr.span,
                );
                Ok(Operand::Move(self.place(result)))
            }
            hir::ExprKind::EnumConstruct {
                enum_id,
                variant_id,
                payload,
            } => {
                let mut values = Vec::new();
                for value in payload {
                    values.push(self.lower_expr(value)?);
                }
                let consumed = values.clone();
                let result = self.temp(expr.ty.clone(), expr.span);
                self.push(
                    StatementKind::Assign(
                        self.place(result),
                        Rvalue::Aggregate(AggregateKind::Enum(*enum_id, *variant_id), values),
                    ),
                    expr.span,
                );
                for value in &consumed {
                    self.consume_operand(value, expr.span);
                }
                self.set_initialized(result, true, expr.span);
                Ok(Operand::Move(self.place(result)))
            }
            hir::ExprKind::Function(_) => Ok(Operand::Constant(Constant::Unit)),
            hir::ExprKind::Call(call) => self.lower_call(call, &expr.ty, expr.span),
            hir::ExprKind::Spawn(call) => self.lower_spawn(call, &expr.ty, expr.span),
            hir::ExprKind::Match { value, arms } => {
                self.lower_match(value, arms, &expr.ty, expr.span)
            }
            hir::ExprKind::Try(value) => self.lower_try(value, &expr.ty, expr.span),
        }
    }

    fn lower_call(
        &mut self,
        call: &hir::Call,
        ty: &hir::Type,
        span: Span,
    ) -> Result<Operand, Diagnostic> {
        let mut arguments = Vec::new();
        for (index, argument) in call.arguments.iter().enumerate() {
            let borrow = if index == 0 {
                call.receiver
            } else if (index == 1
                && matches!(&call.target, hir::CallTarget::Intrinsic(name) if matches!(name.as_str(), "String.push_str" | "String.contains" | "String.starts_with" | "String.ends_with" | "Map.has" | "Map.get" | "Map.get_mut" | "Map.remove" | "Set.has" | "Set.remove")))
                || matches!(&call.target, hir::CallTarget::Intrinsic(name) if name == "Path.new" || name == "Path.join" || name.starts_with("File.") || name.starts_with("Directory."))
            {
                Some(hir::ReceiverMode::Shared)
            } else {
                None
            };
            arguments.push(match borrow {
                Some(hir::ReceiverMode::Shared) => self.borrow_argument(argument, false)?,
                Some(hir::ReceiverMode::Mutable) => self.borrow_argument(argument, true)?,
                _ if matches!(&call.target, hir::CallTarget::Intrinsic(name) if name == "print")
                    && !argument.ty.is_copy(self.program) =>
                {
                    self.borrow_argument(argument, false)?
                }
                _ => self.lower_expr(argument)?,
            });
        }
        let destination = self.temp(ty.clone(), span);
        for argument in &arguments {
            self.consume_operand(argument, span);
        }
        let next = self.new_block();
        self.terminate(Terminator::Call {
            target: call.target.clone(),
            arguments,
            destination: self.place(destination),
            next,
            unwind: None,
            substitutions: call.substitutions.clone(),
            span,
        });
        self.current = next;
        self.set_initialized(destination, true, span);
        Ok(if ty.is_copy(self.program) {
            Operand::Copy(self.place(destination))
        } else {
            Operand::Move(self.place(destination))
        })
    }

    fn lower_spawn(
        &mut self,
        call: &hir::Call,
        ty: &hir::Type,
        span: Span,
    ) -> Result<Operand, Diagnostic> {
        let hir::CallTarget::Function(target) = call.target else {
            return Err(Diagnostic::new(
                DiagnosticKind::Internal,
                "MIR lowering received a non-function spawn target",
                span,
            ));
        };
        let mut arguments = Vec::with_capacity(call.arguments.len());
        for argument in &call.arguments {
            arguments.push(self.lower_expr(argument)?);
        }
        let destination = self.temp(ty.clone(), span);
        for argument in &arguments {
            self.consume_operand(argument, span);
        }
        let next = self.new_block();
        self.terminate(Terminator::Spawn {
            target,
            arguments,
            destination: self.place(destination),
            next,
            substitutions: call.substitutions.clone(),
            span,
        });
        self.current = next;
        self.set_initialized(destination, true, span);
        Ok(Operand::Move(self.place(destination)))
    }

    fn borrow_argument(
        &mut self,
        expression: &hir::Expr,
        mutable: bool,
    ) -> Result<Operand, Diagnostic> {
        let place = if let Some(place) = self.expr_place(expression) {
            place
        } else {
            let operand = self.lower_expr(expression)?;
            let local = self.materialize(operand, expression.ty.clone(), expression.span);
            self.place(local)
        };
        let ty = hir::Type::Reference {
            mutable,
            inner: Box::new(expression.ty.clone()),
        };
        let result = self.temp(ty, expression.span);
        self.push(
            StatementKind::Assign(
                self.place(result),
                if mutable {
                    Rvalue::BorrowMut(place)
                } else {
                    Rvalue::BorrowShared(place)
                },
            ),
            expression.span,
        );
        Ok(Operand::Copy(self.place(result)))
    }

    fn expr_place(&self, expression: &hir::Expr) -> Option<Place> {
        match &expression.kind {
            hir::ExprKind::Local(local) => Some(self.place(self.source_locals[local])),
            hir::ExprKind::Move(place) | hir::ExprKind::Borrow { place, .. } => {
                self.static_place_from_hir(place)
            }
            hir::ExprKind::Field { object, index } => {
                let mut place = self.expr_place(object)?;
                if matches!(object.ty, hir::Type::Reference { .. }) {
                    place.projections.push(Projection::SafeDereference);
                }
                place.projections.push(Projection::Field(*index));
                Some(place)
            }
            hir::ExprKind::Dereference(object, raw) => {
                let mut place = self.expr_place(object)?;
                place.projections.push(if *raw {
                    Projection::RawDereference
                } else {
                    Projection::SafeDereference
                });
                Some(place)
            }
            _ => None,
        }
    }

    fn lower_match(
        &mut self,
        value: &hir::Expr,
        arms: &[hir::MatchArm],
        ty: &hir::Type,
        span: Span,
    ) -> Result<Operand, Diagnostic> {
        let value_ty = value.ty.clone();
        let value = self.lower_expr(value)?;
        let value_local = self.materialize(value, value_ty, span);
        let destination = self.temp(ty.clone(), span);
        let join = self.new_block();
        let arm_blocks = arms.iter().map(|_| self.new_block()).collect::<Vec<_>>();
        let otherwise = arms
            .iter()
            .position(|arm| {
                matches!(
                    arm.pattern,
                    hir::Pattern::Wildcard | hir::Pattern::Binding(_)
                )
            })
            .map(|i| arm_blocks[i])
            .unwrap_or_else(|| self.new_block());
        let enum_targets = arms
            .iter()
            .enumerate()
            .filter_map(|(i, arm)| match arm.pattern {
                hir::Pattern::Variant { variant_id, .. } => Some((variant_id, arm_blocks[i])),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !enum_targets.is_empty() {
            self.terminate(Terminator::SwitchEnum {
                discriminant: Operand::Copy(self.place(value_local)),
                targets: enum_targets,
                otherwise,
            });
        } else {
            let value_targets = arms
                .iter()
                .enumerate()
                .filter_map(|(i, arm)| match &arm.pattern {
                    hir::Pattern::Constant(c) => Some((lower_constant(c), arm_blocks[i])),
                    _ => None,
                })
                .collect();
            self.terminate(Terminator::SwitchValue {
                discriminant: Operand::Copy(self.place(value_local)),
                targets: value_targets,
                otherwise,
            });
        }
        for (arm, block) in arms.iter().zip(arm_blocks) {
            self.current = block;
            let arm_depth = self.live.len();
            self.bind_pattern(&arm.pattern, value_local, arm.span);
            let result = self.lower_expr(&arm.value)?;
            self.push(
                StatementKind::Assign(self.place(destination), Rvalue::Use(result)),
                arm.span,
            );
            self.emit_drops_to(arm_depth, arm.span);
            self.live.truncate(arm_depth);
            if self.open(self.current) {
                self.terminate(Terminator::Goto(join));
            }
        }
        if self.open(otherwise)
            && !arms.iter().any(|arm| {
                matches!(
                    arm.pattern,
                    hir::Pattern::Wildcard | hir::Pattern::Binding(_)
                )
            })
        {
            self.current = otherwise;
            self.terminate(Terminator::Unreachable);
        }
        self.current = join;
        self.set_initialized(destination, true, span);
        Ok(if ty.is_copy(self.program) {
            Operand::Copy(self.place(destination))
        } else {
            Operand::Move(self.place(destination))
        })
    }

    fn lower_try(
        &mut self,
        value: &hir::Expr,
        ty: &hir::Type,
        span: Span,
    ) -> Result<Operand, Diagnostic> {
        let value_ty = value.ty.clone();
        let success_variant = match &value_ty {
            hir::Type::Option(_) => hir::builtin_variant("Some"),
            _ => hir::builtin_variant("Ok"),
        };
        let value = self.lower_expr(value)?;
        let source = self.materialize(value, value_ty, span);
        let success = self.new_block();
        let failure = self.new_block();
        let join = self.new_block();
        let ok = success_variant;
        self.terminate(Terminator::SwitchEnum {
            discriminant: Operand::Copy(self.place(source)),
            targets: vec![(ok, success)],
            otherwise: failure,
        });
        let result = self.temp(ty.clone(), span);
        self.current = success;
        let mut payload = self.place(source);
        payload.projections.push(Projection::VariantField(ok, 0));
        self.push(
            StatementKind::Assign(
                self.place(result),
                Rvalue::Use(if ty.is_copy(self.program) {
                    Operand::Copy(payload.clone())
                } else {
                    Operand::Move(payload.clone())
                }),
            ),
            span,
        );
        if !ty.is_copy(self.program) {
            self.set_place_initialized(&payload, false, span);
            self.set_initialized(source, false, span);
        }
        self.terminate(Terminator::Goto(join));
        self.current = failure;
        self.push(
            StatementKind::Assign(
                self.place(LocalId(0)),
                Rvalue::Use(Operand::Move(self.place(source))),
            ),
            span,
        );
        self.set_initialized(source, false, span);
        self.emit_drops_to(0, span);
        self.terminate(Terminator::Return);
        self.current = join;
        Ok(if ty.is_copy(self.program) {
            Operand::Copy(self.place(result))
        } else {
            Operand::Move(self.place(result))
        })
    }

    fn bind_pattern(&mut self, pattern: &hir::Pattern, source: LocalId, span: Span) {
        match pattern {
            hir::Pattern::Binding(local) => {
                let target = self.source_locals[local];
                self.push(StatementKind::StorageLive(target), span);
                self.live.push(target);
                self.ensure_drop_flag(target);
                self.push(
                    StatementKind::Assign(
                        self.place(target),
                        Rvalue::Use(Operand::Move(self.place(source))),
                    ),
                    span,
                );
                self.set_initialized(target, true, span);
            }
            hir::Pattern::Variant {
                variant_id,
                arguments,
                ..
            } => {
                for (index, argument) in arguments.iter().enumerate() {
                    if let hir::Pattern::Binding(local) = argument {
                        let target = self.source_locals[local];
                        let mut payload = self.place(source);
                        payload
                            .projections
                            .push(Projection::VariantField(*variant_id, index));
                        self.push(StatementKind::StorageLive(target), span);
                        self.live.push(target);
                        self.ensure_drop_flag(target);
                        self.push(
                            StatementKind::Assign(
                                self.place(target),
                                Rvalue::Use(if self.locals[target.0].ty.is_copy(self.program) {
                                    Operand::Copy(payload)
                                } else {
                                    Operand::Move(payload)
                                }),
                            ),
                            span,
                        );
                        self.set_initialized(target, true, span);
                    }
                }
            }
            _ => {}
        }
    }
    fn temp(&mut self, ty: hir::Type, span: Span) -> LocalId {
        let id = LocalId(self.locals.len());
        let needs_drop = !ty.is_copy(self.program);
        self.locals.push(Local {
            id,
            name: format!("_tmp{}", id.0),
            ty,
            kind: LocalKind::Temporary,
            span,
            needs_drop,
            mutable: true,
        });
        self.push(StatementKind::StorageLive(id), span);
        self.live.push(id);
        self.ensure_drop_flag(id);
        id
    }
    fn materialize(&mut self, operand: Operand, ty: hir::Type, span: Span) -> LocalId {
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projections.is_empty() => {
                place.local
            }
            operand => {
                let temp = self.temp(ty, span);
                self.push(
                    StatementKind::Assign(self.place(temp), Rvalue::Use(operand)),
                    span,
                );
                temp
            }
        }
    }
    fn consume_operand(&mut self, operand: &Operand, span: Span) {
        if let Operand::Move(place) = operand {
            self.set_place_initialized(place, false, span);
        }
    }
    fn ensure_drop_flag(&mut self, local: LocalId) {
        if !self.locals[local.0].needs_drop {
            return;
        }
        let mut places = self.drop_places(local);
        if places.is_empty() {
            places.push(self.place(local));
        }
        for place in places {
            if self.drop_flags.contains_key(&place) {
                continue;
            }
            let id = LocalId(self.locals.len());
            self.locals.push(Local {
                id,
                name: format!("_drop{}_{}", local.0, self.drop_flags.len()),
                ty: hir::Type::Bool,
                kind: LocalKind::DropFlag,
                span: self.locals[local.0].span,
                needs_drop: false,
                mutable: true,
            });
            self.drop_flags.insert(place, id);
            self.push(StatementKind::StorageLive(id), self.locals[local.0].span);
            self.push(
                StatementKind::SetDropFlag {
                    local: id,
                    initialized: self.locals[local.0].kind == LocalKind::Argument,
                },
                self.locals[local.0].span,
            );
        }
    }
    fn set_initialized(&mut self, local: LocalId, initialized: bool, span: Span) {
        let flags = self
            .drop_flags
            .iter()
            .filter_map(|(place, flag)| (place.local == local).then_some(*flag))
            .collect::<Vec<_>>();
        for flag in flags {
            self.push(
                StatementKind::SetDropFlag {
                    local: flag,
                    initialized,
                },
                span,
            );
        }
    }
    fn set_place_initialized(&mut self, place: &Place, initialized: bool, span: Span) {
        if place.projections.is_empty() {
            self.set_initialized(place.local, initialized, span);
            return;
        }
        let flags = self
            .drop_flags
            .iter()
            .filter_map(|(candidate, flag)| {
                (candidate.local == place.local
                    && candidate.projections.starts_with(&place.projections))
                .then_some(*flag)
            })
            .collect::<Vec<_>>();
        for flag in flags {
            self.push(
                StatementKind::SetDropFlag {
                    local: flag,
                    initialized,
                },
                span,
            );
        }
    }
    fn drop_places(&self, local: LocalId) -> Vec<Place> {
        self.drop_places_for_type(
            &self.locals[local.0].ty,
            self.place(local),
            &mut HashSet::new(),
        )
    }
    fn drop_places_for_type(
        &self,
        ty: &hir::Type,
        base: Place,
        visiting: &mut HashSet<hir::StructId>,
    ) -> Vec<Place> {
        let hir::Type::Struct(id, _) = ty else {
            return vec![base];
        };
        if !visiting.insert(*id) {
            return vec![base];
        }
        let mut result = Vec::new();
        for field in &self.program.structs[id.0].fields {
            if field.ty.is_copy(self.program) {
                continue;
            }
            let mut place = base.clone();
            place.projections.push(Projection::Field(field.index));
            result.extend(self.drop_places_for_type(&field.ty, place, visiting));
        }
        visiting.remove(id);
        if result.is_empty() {
            vec![base]
        } else {
            result
        }
    }
    fn emit_drops_to(&mut self, depth: usize, span: Span) {
        let values = self
            .live
            .iter()
            .skip(depth)
            .rev()
            .copied()
            .collect::<Vec<_>>();
        for local in values {
            if self.locals[local.0].needs_drop {
                let mut drops = self
                    .drop_flags
                    .iter()
                    .filter_map(|(place, flag)| {
                        (place.local == local).then_some((place.clone(), *flag))
                    })
                    .collect::<Vec<_>>();
                drops.sort_by(|(left, _), (right, _)| right.projections.cmp(&left.projections));
                for (place, flag) in drops {
                    self.push(
                        StatementKind::Drop {
                            place,
                            flag: Some(flag),
                        },
                        span,
                    );
                }
                self.set_initialized(local, false, span);
            }
            self.push(StatementKind::StorageDead(local), span);
        }
    }
    fn emit_drop_for_place(&mut self, target: &Place, span: Span) {
        let mut drops = self
            .drop_flags
            .iter()
            .filter_map(|(place, flag)| {
                (place.local == target.local && place.projections.starts_with(&target.projections))
                    .then_some((place.clone(), *flag))
            })
            .collect::<Vec<_>>();
        drops.sort_by(|(left, _), (right, _)| right.projections.cmp(&left.projections));
        for (place, flag) in drops {
            self.push(
                StatementKind::Drop {
                    place,
                    flag: Some(flag),
                },
                span,
            );
        }
        self.set_place_initialized(target, false, span);
    }
    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        id
    }
    fn push(&mut self, kind: StatementKind, span: Span) {
        self.blocks[self.current.0]
            .statements
            .push(Statement { kind, span });
    }
    fn terminate(&mut self, terminator: Terminator) {
        self.blocks[self.current.0].terminator = terminator;
    }
    fn open(&self, block: BlockId) -> bool {
        matches!(self.blocks[block.0].terminator, Terminator::Unreachable)
    }
    fn place(&self, local: LocalId) -> Place {
        Place {
            local,
            projections: vec![],
        }
    }
    fn place_from_hir(&mut self, place: &hir::Place) -> Result<Place, Diagnostic> {
        let mut lowered = Place {
            local: self.source_locals[&place.local],
            projections: vec![],
        };
        for projection in &place.projections {
            lowered.projections.push(match projection {
                hir::Projection::Field(x) => Projection::Field(*x),
                hir::Projection::SafeDereference => Projection::SafeDereference,
                hir::Projection::RawDereference => Projection::RawDereference,
                hir::Projection::VariantField(v, x) => Projection::VariantField(*v, *x),
                hir::Projection::Index(index) => {
                    let operand = self.lower_expr(index)?;
                    let local = self.materialize(operand, index.ty.clone(), index.span);
                    Projection::Index {
                        index: local,
                        span: index.span,
                    }
                }
                hir::Projection::Subslice { start, end } => {
                    let a = self.lower_expr(start)?;
                    let a = self.materialize(a, start.ty.clone(), start.span);
                    let b = self.lower_expr(end)?;
                    let b = self.materialize(b, end.ty.clone(), end.span);
                    Projection::Subslice {
                        start: a,
                        end: b,
                        span: start.span.through(end.span),
                    }
                }
            });
        }
        Ok(lowered)
    }
    fn static_place_from_hir(&self, place: &hir::Place) -> Option<Place> {
        let mut lowered = Place {
            local: self.source_locals[&place.local],
            projections: vec![],
        };
        for projection in &place.projections {
            lowered.projections.push(match projection {
                hir::Projection::Field(x) => Projection::Field(*x),
                hir::Projection::SafeDereference => Projection::SafeDereference,
                hir::Projection::RawDereference => Projection::RawDereference,
                hir::Projection::VariantField(v, x) => Projection::VariantField(*v, *x),
                hir::Projection::Index(_) | hir::Projection::Subslice { .. } => return None,
            });
        }
        Some(lowered)
    }
}

fn lower_constant(value: &hir::Constant) -> Constant {
    match value {
        hir::Constant::Signed(x, w) => Constant::Signed(*x, *w),
        hir::Constant::Unsigned(x, w) => Constant::Unsigned(*x, *w),
        hir::Constant::Float(x, w) => Constant::Float(*x, *w),
        hir::Constant::Bool(x) => Constant::Bool(*x),
        hir::Constant::Char(x) => Constant::Char(*x),
        hir::Constant::String(x) => Constant::String(x.clone()),
        hir::Constant::Unit => Constant::Unit,
    }
}

pub fn validate(program: &Program) -> Result<(), Diagnostic> {
    for function in &program.functions {
        let storage_live = function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .filter_map(|statement| match statement.kind {
                StatementKind::StorageLive(local) => Some(local),
                _ => None,
            })
            .collect::<HashSet<_>>();
        for local in &function.locals {
            if matches!(
                local.kind,
                LocalKind::User | LocalKind::Temporary | LocalKind::DropFlag
            ) && !storage_live.contains(&local.id)
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Internal,
                    format!("MIR local _{} has no StorageLive", local.id.0),
                    local.span,
                ));
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            let check_block = |target: BlockId| {
                if target.0 >= function.blocks.len() {
                    Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        format!("MIR bb{index} targets missing bb{}", target.0),
                        function.span,
                    ))
                } else {
                    Ok(())
                }
            };
            for statement in &block.statements {
                match &statement.kind {
                    StatementKind::StorageLive(local)
                    | StatementKind::StorageDead(local)
                    | StatementKind::SetDropFlag { local, .. } => {
                        check_local(function, *local, statement.span)?
                    }
                    StatementKind::Assign(place, rvalue) => {
                        check_place(program, function, place, statement.span)?;
                        validate_rvalue(program, function, rvalue, statement.span)?;
                    }
                    StatementKind::Drop { place, flag } => {
                        check_place(program, function, place, statement.span)?;
                        if let Some(flag) = flag {
                            check_local(function, *flag, statement.span)?;
                        }
                    }
                    StatementKind::Nop => {}
                }
            }
            match &block.terminator {
                Terminator::Goto(x) => check_block(*x)?,
                Terminator::SwitchBool {
                    true_block,
                    false_block,
                    ..
                } => {
                    check_block(*true_block)?;
                    check_block(*false_block)?;
                }
                Terminator::SwitchValue {
                    targets, otherwise, ..
                } => {
                    for (_, x) in targets {
                        check_block(*x)?;
                    }
                    check_block(*otherwise)?;
                }
                Terminator::SwitchEnum {
                    targets, otherwise, ..
                } => {
                    for (_, x) in targets {
                        check_block(*x)?;
                    }
                    check_block(*otherwise)?;
                }
                Terminator::Call {
                    destination,
                    next,
                    unwind,
                    ..
                } => {
                    check_place(program, function, destination, function.span)?;
                    if let Terminator::Call { arguments, .. } = &block.terminator {
                        for argument in arguments {
                            validate_operand(program, function, argument, function.span)?;
                        }
                    }
                    check_block(*next)?;
                    if let Some(x) = unwind {
                        check_block(*x)?;
                    }
                }
                Terminator::Spawn {
                    target,
                    arguments,
                    destination,
                    next,
                    substitutions: _,
                    span,
                } => {
                    if target.0 >= program.functions.len() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Internal,
                            "MIR spawn target is out of range",
                            *span,
                        ));
                    }
                    check_place(program, function, destination, *span)?;
                    for argument in arguments {
                        validate_operand(program, function, argument, *span)?;
                    }
                    check_block(*next)?;
                }
                Terminator::Return | Terminator::Unreachable => {}
            }
        }
    }
    Ok(())
}
fn check_local(function: &Function, local: LocalId, span: Span) -> Result<(), Diagnostic> {
    if local.0 >= function.locals.len() {
        Err(Diagnostic::new(
            DiagnosticKind::Internal,
            "MIR references an invalid local",
            span,
        ))
    } else {
        Ok(())
    }
}
fn check_place(
    program: &Program,
    function: &Function,
    place: &Place,
    span: Span,
) -> Result<hir::Type, Diagnostic> {
    check_local(function, place.local, span)?;
    let mut ty = function.locals[place.local.0].ty.clone();
    for projection in &place.projections {
        ty = match (projection, ty) {
            (Projection::SafeDereference, hir::Type::Reference { inner, .. })
            | (Projection::SafeDereference, hir::Type::MutexGuard(inner))
            | (Projection::RawDereference, hir::Type::RawPointer { inner, .. }) => *inner,
            (Projection::Field(index), hir::Type::Struct(id, _)) => program
                .structs
                .get(id.0)
                .and_then(|declaration| declaration.fields.get(*index))
                .map(|field| field.ty.clone())
                .ok_or_else(|| {
                    Diagnostic::new(
                        DiagnosticKind::Internal,
                        "MIR field projection is out of range",
                        span,
                    )
                })?,
            (
                Projection::VariantField(_, _),
                hir::Type::Enum(_, _) | hir::Type::Option(_) | hir::Type::Result(_, _),
            ) => hir::Type::Unknown,
            (
                Projection::Index { index, .. },
                hir::Type::Array(element, _)
                | hir::Type::Slice(element)
                | hir::Type::List(element)
                | hir::Type::Set(element),
            ) => {
                check_local(function, *index, span)?;
                if !matches!(function.locals[index.0].ty, hir::Type::Int { .. }) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "MIR index projection operand is not an integer",
                        span,
                    ));
                }
                *element
            }
            (
                Projection::Subslice { start, end, .. },
                hir::Type::Array(element, _) | hir::Type::Slice(element) | hir::Type::List(element),
            ) => {
                check_local(function, *start, span)?;
                check_local(function, *end, span)?;
                if !matches!(function.locals[start.0].ty, hir::Type::Int { .. })
                    || !matches!(function.locals[end.0].ty, hir::Type::Int { .. })
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "MIR subslice bounds are not integers",
                        span,
                    ));
                }
                hir::Type::Slice(element)
            }
            (Projection::Subslice { start, end, .. }, hir::Type::String | hir::Type::Str) => {
                check_local(function, *start, span)?;
                check_local(function, *end, span)?;
                if !matches!(function.locals[start.0].ty, hir::Type::Int { .. })
                    || !matches!(function.locals[end.0].ty, hir::Type::Int { .. })
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Internal,
                        "MIR string slice bounds are not integers",
                        span,
                    ));
                }
                hir::Type::Str
            }
            _ => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Internal,
                    "MIR projection is invalid for its base type",
                    span,
                ));
            }
        };
    }
    Ok(ty)
}
fn validate_operand(
    program: &Program,
    function: &Function,
    operand: &Operand,
    span: Span,
) -> Result<(), Diagnostic> {
    if let Operand::Move(place) | Operand::Copy(place) = operand {
        check_place(program, function, place, span)?;
        if matches!(operand, Operand::Copy(_))
            && place.projections.is_empty()
            && function.locals[place.local.0].needs_drop
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Internal,
                format!(
                    "MIR copies non-Copy owning local _{} `{}` of type {:?}",
                    place.local.0,
                    function.locals[place.local.0].name,
                    function.locals[place.local.0].ty
                ),
                span,
            ));
        }
    }
    Ok(())
}
fn validate_rvalue(
    program: &Program,
    function: &Function,
    rvalue: &Rvalue,
    span: Span,
) -> Result<(), Diagnostic> {
    match rvalue {
        Rvalue::Use(x) | Rvalue::UnaryOp(_, x) | Rvalue::Cast { operand: x, .. } => {
            validate_operand(program, function, x, span)?
        }
        Rvalue::BinaryOp(_, x, y) => {
            validate_operand(program, function, x, span)?;
            validate_operand(program, function, y, span)?;
        }
        Rvalue::Aggregate(_, xs) => {
            for x in xs {
                validate_operand(program, function, x, span)?;
            }
        }
        Rvalue::BorrowMut(x) => {
            check_place(program, function, x, span)?;
            let through_mutable_reference =
                matches!(x.projections.first(), Some(Projection::SafeDereference))
                    && matches!(
                        function.locals[x.local.0].ty,
                        hir::Type::Reference { mutable: true, .. } | hir::Type::MutexGuard(_)
                    );
            if !function.locals[x.local.0].mutable && !through_mutable_reference {
                return Err(Diagnostic::new(
                    DiagnosticKind::Internal,
                    "MIR creates a mutable borrow from an immutable base",
                    span,
                ));
            }
        }
        Rvalue::BorrowShared(x) | Rvalue::RawAddress { place: x, .. } | Rvalue::Discriminant(x) => {
            check_place(program, function, x, span)?;
        }
        Rvalue::Len(x) => {
            let ty = check_place(program, function, x, span)?;
            if !matches!(
                ty,
                hir::Type::Array(_, _)
                    | hir::Type::Slice(_)
                    | hir::Type::List(_)
                    | hir::Type::Set(_)
                    | hir::Type::String
                    | hir::Type::Str
            ) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Internal,
                    "MIR length operation requires a collection",
                    span,
                ));
            }
        }
    }
    Ok(())
}

pub fn dump(program: &Program) -> String {
    let mut out = String::new();
    for function in &program.functions {
        out.push_str(&format!(
            "mir fn{} {} args={} return=_0 @ {}:{}\n",
            function.id.0,
            function.name,
            function.argument_count,
            function.span.start.line,
            function.span.start.column
        ));
        for local in &function.locals {
            out.push_str(&format!(
                "  let _{} {}: {:?} [{:?}]\n",
                local.id.0, local.name, local.ty, local.kind
            ));
        }
        for (index, block) in function.blocks.iter().enumerate() {
            out.push_str(&format!("bb{index}:\n"));
            for statement in &block.statements {
                out.push_str(&format!(
                    "  {:?} @ {}:{}\n",
                    statement.kind, statement.span.start.line, statement.span.start.column
                ));
            }
            out.push_str(&format!("  -> {:?}\n", block.terminator));
        }
    }
    out
}

pub fn moved_places(function: &Function) -> HashSet<Place> {
    let mut moved = HashSet::new();
    for block in &function.blocks {
        for statement in &block.statements {
            if let StatementKind::Assign(_, value) = &statement.kind {
                collect_moved(value, &mut moved);
            }
        }
        if let Terminator::Call { arguments, .. } | Terminator::Spawn { arguments, .. } =
            &block.terminator
        {
            for argument in arguments {
                if let Operand::Move(place) = argument {
                    moved.insert(place.clone());
                }
            }
        }
    }
    moved
}
fn collect_moved(value: &Rvalue, output: &mut HashSet<Place>) {
    let mut operand = |x: &Operand| {
        if let Operand::Move(place) = x {
            output.insert(place.clone());
        }
    };
    match value {
        Rvalue::Use(x) | Rvalue::UnaryOp(_, x) | Rvalue::Cast { operand: x, .. } => {
            operand(x);
        }
        Rvalue::BinaryOp(_, x, y) => {
            operand(x);
            operand(y);
        }
        Rvalue::Aggregate(_, values) => {
            for x in values {
                operand(x);
            }
        }
        _ => {}
    }
}
