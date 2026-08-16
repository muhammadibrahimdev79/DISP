use crate::{
    ast::{
        BinaryOperator, BindingKind, Block, ClosureBody, Expr, Expression, Pattern, Program,
        Statement, UnaryOperator,
    },
    diagnostics::{Diagnostic, DiagnosticKind, Span},
};
use std::collections::{BTreeMap, HashMap};

pub use crate::limits::{
    MAX_CONST_DEPTH as MAX_DEPTH, MAX_CONST_STEPS as MAX_STEPS,
    MAX_CONST_STRING_BYTES as MAX_STRING_BYTES, MAX_CONST_VALUE_NODES as MAX_VALUE_NODES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub steps: usize,
    pub depth: usize,
    pub value_nodes: usize,
    pub string_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: MAX_STEPS,
            depth: MAX_DEPTH,
            value_nodes: MAX_VALUE_NODES,
            string_bytes: MAX_STRING_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Integer {
    negative: bool,
    magnitude: u128,
}

impl Integer {
    fn positive(magnitude: u128) -> Self {
        Self {
            negative: false,
            magnitude,
        }
    }

    fn normalized(negative: bool, magnitude: u128) -> Self {
        Self {
            negative: negative && magnitude != 0,
            magnitude,
        }
    }

    fn negate(self) -> Self {
        Self::normalized(!self.negative, self.magnitude)
    }

    fn checked_add(self, right: Self) -> Option<Self> {
        if self.negative == right.negative {
            self.magnitude
                .checked_add(right.magnitude)
                .map(|magnitude| Self::normalized(self.negative, magnitude))
        } else if self.magnitude >= right.magnitude {
            Some(Self::normalized(
                self.negative,
                self.magnitude - right.magnitude,
            ))
        } else {
            Some(Self::normalized(
                right.negative,
                right.magnitude - self.magnitude,
            ))
        }
    }

    fn compare(self, right: Self) -> std::cmp::Ordering {
        match (self.negative, right.negative) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => self.magnitude.cmp(&right.magnitude),
            (true, true) => right.magnitude.cmp(&self.magnitude),
        }
    }
}

impl std::fmt::Display for Integer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negative {
            write!(formatter, "-{}", self.magnitude)
        } else {
            write!(formatter, "{}", self.magnitude)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(Integer),
    Float(f64),
    String(String),
    Character(char),
    Bool(bool),
    Array(Vec<Value>),
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
}

impl Value {
    fn render(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value) => format!("{value:?}"),
            Self::Character(value) => format!("{value:?}"),
            Self::Bool(value) => value.to_string(),
            Self::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Struct { name, fields } => format!(
                "{name} {{ {} }}",
                fields
                    .iter()
                    .map(|(name, value)| format!("{name}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn expression(&self, span: Span) -> Expr {
        let node = match self {
            Self::Integer(value) if value.negative => Expression::Unary {
                operator: UnaryOperator::Negate,
                operand: Box::new(Expr {
                    node: Expression::Integer(value.magnitude),
                    span,
                }),
            },
            Self::Integer(value) => Expression::Integer(value.magnitude),
            Self::Float(value) => Expression::Float(*value),
            Self::String(value) => Expression::String(value.clone()),
            Self::Character(value) => Expression::Character(*value),
            Self::Bool(value) => Expression::Bool(*value),
            Self::Array(values) => {
                Expression::Array(values.iter().map(|value| value.expression(span)).collect())
            }
            Self::Struct { name, fields } => Expression::StructConstruct {
                name: name.clone(),
                name_span: span,
                fields: fields
                    .iter()
                    .map(|(name, value)| crate::ast::StructFieldValue {
                        name: name.clone(),
                        name_span: span,
                        value: value.expression(span),
                    })
                    .collect(),
            },
        };
        Expr { node, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Constant {
    pub function: String,
    pub name: String,
    pub span: Span,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub constants: Vec<Constant>,
    pub steps: usize,
}

impl Report {
    pub fn render(&self) -> String {
        let mut output = String::new();
        for constant in &self.constants {
            output.push_str(&constant.function);
            output.push_str("::");
            output.push_str(&constant.name);
            output.push_str(" = ");
            output.push_str(&constant.value.render());
            output.push('\n');
        }
        output
    }
}

type Scope = HashMap<String, Option<Value>>;

struct Evaluator {
    scopes: Vec<Scope>,
    constants: Vec<Constant>,
    function: String,
    steps: usize,
    value_nodes: usize,
    string_bytes: usize,
    limits: Limits,
}

pub fn evaluate(program: &Program) -> Result<Report, Diagnostic> {
    evaluate_with_limits(program, Limits::default())
}

pub fn evaluate_with_limits(program: &Program, limits: Limits) -> Result<Report, Diagnostic> {
    let mut evaluator = Evaluator {
        scopes: Vec::new(),
        constants: Vec::new(),
        function: String::new(),
        steps: 0,
        value_nodes: 0,
        string_bytes: 0,
        limits,
    };
    for function in &program.functions {
        if function.external.is_some() {
            continue;
        }
        evaluator.function.clone_from(&function.name);
        evaluator.scopes.clear();
        evaluator.scopes.push(
            function
                .parameters
                .iter()
                .map(|parameter| (parameter.name.clone(), None))
                .collect(),
        );
        evaluator.visit_block(&function.body)?;
    }
    for (implementation_index, implementation) in program.implementations.iter().enumerate() {
        for method in &implementation.methods {
            evaluator.function = format!("impl{implementation_index}.{}", method.name);
            evaluator.scopes.clear();
            evaluator.scopes.push(
                method
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.name.clone(), None))
                    .collect(),
            );
            evaluator.visit_block(&method.body)?;
        }
    }
    Ok(Report {
        constants: evaluator.constants,
        steps: evaluator.steps,
    })
}

pub fn fold(program: &mut Program, report: &Report) {
    let mut folder = Folder {
        scopes: Vec::new(),
        constants: &report.constants,
        function: String::new(),
    };
    for function in &mut program.functions {
        if function.external.is_some() {
            continue;
        }
        folder.function.clone_from(&function.name);
        folder.scopes.clear();
        folder.scopes.push(
            function
                .parameters
                .iter()
                .map(|parameter| (parameter.name.clone(), None))
                .collect(),
        );
        folder.fold_block(&mut function.body);
    }
    for (implementation_index, implementation) in program.implementations.iter_mut().enumerate() {
        for method in &mut implementation.methods {
            folder.function = format!("impl{implementation_index}.{}", method.name);
            folder.scopes.clear();
            folder.scopes.push(
                method
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.name.clone(), None))
                    .collect(),
            );
            folder.fold_block(&mut method.body);
        }
    }
}

struct Folder<'a> {
    scopes: Vec<Scope>,
    constants: &'a [Constant],
    function: String,
}

impl Folder<'_> {
    fn fold_block(&mut self, block: &mut Block) {
        self.scopes.push(HashMap::new());
        for statement in &mut block.statements {
            self.fold_statement(&mut statement.node, statement.span);
        }
        self.scopes.pop();
    }

    fn fold_statement(&mut self, statement: &mut Statement, span: Span) {
        match statement {
            Statement::Binding {
                kind, name, value, ..
            } => {
                if let Some(initializer) = value {
                    self.fold_expr(initializer);
                }
                let constant = if *kind == BindingKind::Const {
                    self.constants
                        .iter()
                        .find(|constant| {
                            constant.function == self.function
                                && constant.name == *name
                                && constant.span == span
                        })
                        .map(|constant| constant.value.clone())
                } else {
                    None
                };
                if let (Some(initializer), Some(constant)) = (value, &constant) {
                    *initializer = constant.expression(initializer.span);
                }
                self.scopes
                    .last_mut()
                    .expect("block scope exists")
                    .insert(name.clone(), constant);
            }
            Statement::Assignment { value, .. } => self.fold_expr(value),
            Statement::PlaceAssignment { target, value, .. } => {
                self.fold_expr(target);
                self.fold_expr(value);
            }
            Statement::Expression(value) => self.fold_expr(value),
            Statement::Return(value) => {
                if let Some(value) = value {
                    self.fold_expr(value);
                }
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.fold_expr(condition);
                self.fold_block(then_branch);
                if let Some(branch) = else_branch {
                    self.fold_block(branch);
                }
            }
            Statement::While { condition, body } => {
                self.fold_expr(condition);
                self.fold_block(body);
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                self.fold_expr(start);
                self.fold_expr(end);
                self.fold_block_with_shadow(body, name);
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                self.fold_expr(iterable);
                self.fold_block_with_shadow(body, name);
            }
            Statement::Loop(body) | Statement::Unsafe { body, .. } => self.fold_block(body),
            Statement::Break | Statement::Continue => {}
        }
    }

    fn fold_block_with_shadow(&mut self, block: &mut Block, name: &str) {
        self.scopes.push(HashMap::from([(name.to_owned(), None)]));
        self.fold_block(block);
        self.scopes.pop();
    }

    fn fold_expr(&mut self, expression: &mut Expr) {
        if let Expression::Identifier(name) = &expression.node
            && let Some(value) = self.lookup(name).cloned()
        {
            *expression = value.expression(expression.span);
            return;
        }
        match &mut expression.node {
            Expression::Array(values) => {
                for value in values {
                    self.fold_expr(value);
                }
            }
            Expression::StructConstruct { fields, .. } => {
                for field in fields {
                    self.fold_expr(&mut field.value);
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
            } => self.fold_expr(object),
            Expression::Borrow { target, .. } => self.fold_expr(target),
            Expression::Index { object, index } => {
                self.fold_expr(object);
                self.fold_expr(index);
            }
            Expression::Subslice { object, start, end } => {
                self.fold_expr(object);
                self.fold_expr(start);
                self.fold_expr(end);
            }
            Expression::Match { value, arms } => {
                self.fold_expr(value);
                for arm in arms {
                    let shadows = pattern_names(&arm.pattern.node);
                    self.scopes
                        .push(shadows.into_iter().map(|name| (name, None)).collect());
                    if let Some(guard) = &mut arm.guard {
                        self.fold_expr(guard);
                    }
                    self.fold_expr(&mut arm.value);
                    self.scopes.pop();
                }
            }
            Expression::Binary { left, right, .. } => {
                self.fold_expr(left);
                self.fold_expr(right);
            }
            Expression::Call { callee, arguments } => {
                self.fold_expr(callee);
                for argument in arguments {
                    self.fold_expr(argument);
                }
            }
            Expression::Closure {
                parameters, body, ..
            } => {
                self.scopes.push(
                    parameters
                        .iter()
                        .map(|parameter| (parameter.name.clone(), None))
                        .collect(),
                );
                match body {
                    ClosureBody::Expression(value) => self.fold_expr(value),
                    ClosureBody::Block(block) => self.fold_block(block),
                }
                self.scopes.pop();
            }
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    self.fold_expr(path);
                }
            }
            Expression::DataWrite { value, store, .. } => {
                self.fold_expr(value);
                self.fold_expr(store);
            }
            Expression::DataQuery {
                store,
                predicate,
                order,
                limit,
                ..
            } => {
                self.fold_expr(store);
                if let Some(predicate) = predicate {
                    self.fold_expr(predicate);
                }
                if let Some(order) = order {
                    self.fold_expr(&mut order.key);
                }
                if let Some(limit) = limit {
                    self.fold_expr(limit);
                }
            }
            Expression::DataRemove {
                store, predicate, ..
            } => {
                self.fold_expr(store);
                self.fold_expr(predicate);
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_)
            | Expression::Identifier(_) => {}
        }
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return value.as_ref();
            }
        }
        None
    }
}

fn pattern_names(pattern: &Pattern) -> Vec<String> {
    match pattern {
        Pattern::Binding(name) => vec![name.clone()],
        Pattern::Or(alternatives) => alternatives
            .first()
            .map(|alternative| pattern_names(&alternative.node))
            .unwrap_or_default(),
        Pattern::Struct { fields, .. } => fields
            .iter()
            .flat_map(|field| pattern_names(&field.pattern.node))
            .collect(),
        Pattern::Variant { arguments, .. } => arguments
            .iter()
            .flat_map(|argument| pattern_names(&argument.node))
            .collect(),
        _ => Vec::new(),
    }
}

impl Evaluator {
    fn visit_block(&mut self, block: &Block) -> Result<(), Diagnostic> {
        self.scopes.push(HashMap::new());
        for statement in &block.statements {
            self.visit_statement(&statement.node, statement.span)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn visit_statement(&mut self, statement: &Statement, span: Span) -> Result<(), Diagnostic> {
        match statement {
            Statement::Binding {
                kind, name, value, ..
            } => {
                if *kind == BindingKind::Const {
                    let value = self.eval(
                        value
                            .as_ref()
                            .expect("type checking requires constant initializers"),
                        0,
                    )?;
                    self.charge_value(&value, span)?;
                    self.constants.push(Constant {
                        function: self.function.clone(),
                        name: name.clone(),
                        span,
                        value: value.clone(),
                    });
                    self.scopes
                        .last_mut()
                        .expect("block scope exists")
                        .insert(name.clone(), Some(value));
                } else {
                    self.scopes
                        .last_mut()
                        .expect("block scope exists")
                        .insert(name.clone(), None);
                }
                if let Some(value) = value {
                    self.visit_nested_closures(value)?;
                }
            }
            Statement::Assignment { value, .. } => self.visit_nested_closures(value)?,
            Statement::PlaceAssignment { target, value, .. } => {
                self.visit_nested_closures(target)?;
                self.visit_nested_closures(value)?;
            }
            Statement::Expression(value) => self.visit_nested_closures(value)?,
            Statement::Return(value) => {
                if let Some(value) = value {
                    self.visit_nested_closures(value)?;
                }
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.visit_nested_closures(condition)?;
                self.visit_block(then_branch)?;
                if let Some(branch) = else_branch {
                    self.visit_block(branch)?;
                }
            }
            Statement::While { condition, body } => {
                self.visit_nested_closures(condition)?;
                self.visit_block(body)?;
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                self.visit_nested_closures(start)?;
                self.visit_nested_closures(end)?;
                self.visit_block_with_shadow(body, name)?;
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                self.visit_nested_closures(iterable)?;
                self.visit_block_with_shadow(body, name)?;
            }
            Statement::Loop(body) | Statement::Unsafe { body, .. } => self.visit_block(body)?,
            Statement::Break | Statement::Continue => {}
        }
        Ok(())
    }

    fn visit_block_with_shadow(&mut self, block: &Block, name: &str) -> Result<(), Diagnostic> {
        self.scopes.push(HashMap::from([(name.to_owned(), None)]));
        let result = self.visit_block(block);
        self.scopes.pop();
        result
    }

    fn visit_nested_closures(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        match &expression.node {
            Expression::Closure {
                parameters, body, ..
            } => {
                self.scopes.push(
                    parameters
                        .iter()
                        .map(|parameter| (parameter.name.clone(), None))
                        .collect(),
                );
                if let ClosureBody::Block(block) = body {
                    self.visit_block(block)?;
                } else if let ClosureBody::Expression(value) = body {
                    self.visit_nested_closures(value)?;
                }
                self.scopes.pop();
            }
            Expression::Array(values) => {
                for value in values {
                    self.visit_nested_closures(value)?;
                }
            }
            Expression::StructConstruct { fields, .. } => {
                for field in fields {
                    self.visit_nested_closures(&field.value)?;
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
            } => self.visit_nested_closures(object)?,
            Expression::Borrow { target, .. } => self.visit_nested_closures(target)?,
            Expression::Index { object, index } => {
                self.visit_nested_closures(object)?;
                self.visit_nested_closures(index)?;
            }
            Expression::Subslice { object, start, end } => {
                self.visit_nested_closures(object)?;
                self.visit_nested_closures(start)?;
                self.visit_nested_closures(end)?;
            }
            Expression::Match { value, arms } => {
                self.visit_nested_closures(value)?;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.visit_nested_closures(guard)?;
                    }
                    self.visit_nested_closures(&arm.value)?;
                }
            }
            Expression::Binary { left, right, .. } => {
                self.visit_nested_closures(left)?;
                self.visit_nested_closures(right)?;
            }
            Expression::Call { callee, arguments } => {
                self.visit_nested_closures(callee)?;
                for argument in arguments {
                    self.visit_nested_closures(argument)?;
                }
            }
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    self.visit_nested_closures(path)?;
                }
            }
            Expression::DataWrite { value, store, .. } => {
                self.visit_nested_closures(value)?;
                self.visit_nested_closures(store)?;
            }
            Expression::DataQuery {
                store,
                predicate,
                order,
                limit,
                ..
            } => {
                self.visit_nested_closures(store)?;
                if let Some(predicate) = predicate {
                    self.visit_nested_closures(predicate)?;
                }
                if let Some(order) = order {
                    self.visit_nested_closures(&order.key)?;
                }
                if let Some(limit) = limit {
                    self.visit_nested_closures(limit)?;
                }
            }
            Expression::DataRemove {
                store, predicate, ..
            } => {
                self.visit_nested_closures(store)?;
                self.visit_nested_closures(predicate)?;
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_)
            | Expression::Identifier(_) => {}
        }
        Ok(())
    }

    fn eval(&mut self, expression: &Expr, depth: usize) -> Result<Value, Diagnostic> {
        self.steps = self.steps.checked_add(1).ok_or_else(|| {
            self.limit_error("compile-time step counter overflowed", expression.span)
        })?;
        if self.steps > self.limits.steps {
            return Err(self.limit_error(
                format!(
                    "compile-time evaluation exceeded {} steps",
                    self.limits.steps
                ),
                expression.span,
            ));
        }
        if depth > self.limits.depth {
            return Err(self.limit_error(
                format!(
                    "compile-time evaluation exceeded depth {}",
                    self.limits.depth
                ),
                expression.span,
            ));
        }
        match &expression.node {
            Expression::Integer(value) => Ok(Value::Integer(Integer::positive(*value))),
            Expression::Float(value) => Ok(Value::Float(*value)),
            Expression::String(value) => Ok(Value::String(value.clone())),
            Expression::Character(value) => Ok(Value::Character(*value)),
            Expression::Bool(value) => Ok(Value::Bool(*value)),
            Expression::Identifier(name) => self.lookup(name).cloned().ok_or_else(|| {
                self.eval_error(
                    format!("`{name}` has no compile-time value"),
                    expression.span,
                )
            }),
            Expression::Array(values) => values
                .iter()
                .map(|value| self.eval(value, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Expression::StructConstruct { name, fields, .. } => {
                let mut values = BTreeMap::new();
                for field in fields {
                    values.insert(field.name.clone(), self.eval(&field.value, depth + 1)?);
                }
                Ok(Value::Struct {
                    name: name.clone(),
                    fields: values,
                })
            }
            Expression::FieldAccess { object, field, .. } => {
                let Value::Struct { fields, .. } = self.eval(object, depth + 1)? else {
                    return Err(self.eval_error(
                        "compile-time field access requires a struct value",
                        expression.span,
                    ));
                };
                fields.get(field).cloned().ok_or_else(|| {
                    self.eval_error(
                        format!("unknown compile-time field `{field}`"),
                        expression.span,
                    )
                })
            }
            Expression::Unary { operator, operand } => {
                let value = self.eval(operand, depth + 1)?;
                self.eval_unary(*operator, value, expression.span)
            }
            Expression::Binary {
                left,
                operator,
                right,
            } => self.eval_binary(left, *operator, right, depth, expression.span),
            Expression::Match { value, arms } => {
                let value = self.eval(value, depth + 1)?;
                for arm in arms {
                    let mut bindings = HashMap::new();
                    if pattern_matches(&arm.pattern.node, &value, &mut bindings) {
                        self.scopes.push(
                            bindings
                                .into_iter()
                                .map(|(name, value)| (name, Some(value)))
                                .collect(),
                        );
                        let guard_matches = if let Some(guard) = &arm.guard {
                            matches!(self.eval(guard, depth + 1)?, Value::Bool(true))
                        } else {
                            true
                        };
                        if !guard_matches {
                            self.scopes.pop();
                            continue;
                        }
                        let result = self.eval(&arm.value, depth + 1);
                        self.scopes.pop();
                        return result;
                    }
                }
                Err(self.eval_error("constant match has no matching arm", expression.span))
            }
            _ => Err(self.eval_error(
                "expression is not available in deterministic compile-time evaluation",
                expression.span,
            )),
        }
    }

    fn eval_unary(
        &self,
        operator: UnaryOperator,
        value: Value,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        match (operator, value) {
            (UnaryOperator::Negate, Value::Integer(value)) => Ok(Value::Integer(value.negate())),
            (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
            (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
            _ => Err(self.eval_error("invalid compile-time unary operation", span)),
        }
    }

    fn eval_binary(
        &mut self,
        left: &Expr,
        operator: BinaryOperator,
        right: &Expr,
        depth: usize,
        span: Span,
    ) -> Result<Value, Diagnostic> {
        let left = self.eval(left, depth + 1)?;
        if operator == BinaryOperator::And && left == Value::Bool(false) {
            return Ok(Value::Bool(false));
        }
        if operator == BinaryOperator::Or && left == Value::Bool(true) {
            return Ok(Value::Bool(true));
        }
        let right = self.eval(right, depth + 1)?;
        match (left, operator, right) {
            (Value::Integer(left), BinaryOperator::Add, Value::Integer(right)) => left
                .checked_add(right)
                .map(Value::Integer)
                .ok_or_else(|| self.eval_error("compile-time integer overflow", span)),
            (Value::Integer(left), BinaryOperator::Subtract, Value::Integer(right)) => left
                .checked_add(right.negate())
                .map(Value::Integer)
                .ok_or_else(|| self.eval_error("compile-time integer overflow", span)),
            (Value::Integer(left), BinaryOperator::Multiply, Value::Integer(right)) => left
                .magnitude
                .checked_mul(right.magnitude)
                .map(|magnitude| {
                    Value::Integer(Integer::normalized(
                        left.negative != right.negative,
                        magnitude,
                    ))
                })
                .ok_or_else(|| self.eval_error("compile-time integer overflow", span)),
            (Value::Integer(_), BinaryOperator::Divide, Value::Integer(right))
            | (Value::Integer(_), BinaryOperator::Remainder, Value::Integer(right))
                if right.magnitude == 0 =>
            {
                Err(self.eval_error("division by zero during compile-time evaluation", span))
            }
            (Value::Integer(left), BinaryOperator::Divide, Value::Integer(right)) => {
                Ok(Value::Integer(Integer::normalized(
                    left.negative != right.negative,
                    left.magnitude / right.magnitude,
                )))
            }
            (Value::Integer(left), BinaryOperator::Remainder, Value::Integer(right)) => {
                Ok(Value::Integer(Integer::normalized(
                    left.negative,
                    left.magnitude % right.magnitude,
                )))
            }
            (Value::Integer(left), operator, Value::Integer(right))
                if comparison(operator).is_some() =>
            {
                Ok(Value::Bool(compare_ordering(left.compare(right), operator)))
            }
            (Value::Float(left), BinaryOperator::Add, Value::Float(right)) => {
                Ok(Value::Float(left + right))
            }
            (Value::Float(left), BinaryOperator::Subtract, Value::Float(right)) => {
                Ok(Value::Float(left - right))
            }
            (Value::Float(left), BinaryOperator::Multiply, Value::Float(right)) => {
                Ok(Value::Float(left * right))
            }
            (Value::Float(left), BinaryOperator::Divide, Value::Float(right)) => {
                Ok(Value::Float(left / right))
            }
            (Value::Float(left), BinaryOperator::Remainder, Value::Float(right)) => {
                Ok(Value::Float(left % right))
            }
            (Value::Float(left), operator, Value::Float(right))
                if comparison(operator).is_some() =>
            {
                Ok(Value::Bool(compare_floats(left, right, operator)))
            }
            (Value::String(mut left), BinaryOperator::Add, Value::String(right)) => {
                left.push_str(&right);
                Ok(Value::String(left))
            }
            (Value::String(left), operator, Value::String(right))
                if comparison(operator).is_some() =>
            {
                Ok(Value::Bool(compare_ordering(left.cmp(&right), operator)))
            }
            (Value::Character(left), operator, Value::Character(right))
                if comparison(operator).is_some() =>
            {
                Ok(Value::Bool(compare_ordering(left.cmp(&right), operator)))
            }
            (Value::Bool(left), BinaryOperator::And, Value::Bool(right)) => {
                Ok(Value::Bool(left && right))
            }
            (Value::Bool(left), BinaryOperator::Or, Value::Bool(right)) => {
                Ok(Value::Bool(left || right))
            }
            (Value::Bool(left), BinaryOperator::Equal, Value::Bool(right)) => {
                Ok(Value::Bool(left == right))
            }
            (Value::Bool(left), BinaryOperator::NotEqual, Value::Bool(right)) => {
                Ok(Value::Bool(left != right))
            }
            _ => Err(self.eval_error("invalid compile-time binary operation", span)),
        }
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return value.as_ref();
            }
        }
        None
    }

    fn charge_value(&mut self, value: &Value, span: Span) -> Result<(), Diagnostic> {
        fn measure(value: &Value) -> (usize, usize) {
            match value {
                Value::String(value) => (1, value.len()),
                Value::Array(values) => values.iter().fold((1, 0), |(nodes, bytes), value| {
                    let (child_nodes, child_bytes) = measure(value);
                    (
                        nodes.saturating_add(child_nodes),
                        bytes.saturating_add(child_bytes),
                    )
                }),
                Value::Struct { fields, .. } => {
                    fields.values().fold((1, 0), |(nodes, bytes), value| {
                        let (child_nodes, child_bytes) = measure(value);
                        (
                            nodes.saturating_add(child_nodes),
                            bytes.saturating_add(child_bytes),
                        )
                    })
                }
                _ => (1, 0),
            }
        }
        let (nodes, bytes) = measure(value);
        self.value_nodes = self.value_nodes.saturating_add(nodes);
        self.string_bytes = self.string_bytes.saturating_add(bytes);
        if self.value_nodes > self.limits.value_nodes {
            return Err(self.limit_error(
                format!(
                    "compile-time values exceeded {} nodes",
                    self.limits.value_nodes
                ),
                span,
            ));
        }
        if self.string_bytes > self.limits.string_bytes {
            return Err(self.limit_error(
                format!(
                    "compile-time strings exceeded {} bytes",
                    self.limits.string_bytes
                ),
                span,
            ));
        }
        Ok(())
    }

    fn eval_error(&self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::new(DiagnosticKind::Type, message, span)
            .with_help("compile-time evaluation is deterministic and has no ambient authority")
    }

    fn limit_error(&self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::new(DiagnosticKind::Type, message, span)
            .with_help("reduce the constant expression or generated value")
    }
}

fn comparison(operator: BinaryOperator) -> Option<()> {
    matches!(
        operator,
        BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
    )
    .then_some(())
}

fn compare_ordering(ordering: std::cmp::Ordering, operator: BinaryOperator) -> bool {
    match operator {
        BinaryOperator::Equal => ordering.is_eq(),
        BinaryOperator::NotEqual => !ordering.is_eq(),
        BinaryOperator::Less => ordering.is_lt(),
        BinaryOperator::LessEqual => !ordering.is_gt(),
        BinaryOperator::Greater => ordering.is_gt(),
        BinaryOperator::GreaterEqual => !ordering.is_lt(),
        _ => false,
    }
}

fn compare_floats(left: f64, right: f64, operator: BinaryOperator) -> bool {
    match operator {
        BinaryOperator::Equal => left == right,
        BinaryOperator::NotEqual => left != right,
        BinaryOperator::Less => left < right,
        BinaryOperator::LessEqual => left <= right,
        BinaryOperator::Greater => left > right,
        BinaryOperator::GreaterEqual => left >= right,
        _ => false,
    }
}

fn pattern_matches(
    pattern: &Pattern,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        Pattern::Binding(name) => {
            bindings.insert(name.clone(), value.clone());
            true
        }
        Pattern::Integer(pattern) => *value == Value::Integer(Integer::positive(*pattern)),
        Pattern::NegativeInteger(pattern) => {
            *value == Value::Integer(Integer::normalized(true, *pattern))
        }
        Pattern::String(pattern) => matches!(value, Value::String(value) if value == pattern),
        Pattern::Character(pattern) => {
            matches!(value, Value::Character(value) if value == pattern)
        }
        Pattern::Bool(pattern) => matches!(value, Value::Bool(value) if value == pattern),
        Pattern::Or(alternatives) => alternatives.iter().any(|alternative| {
            let mut candidate = bindings.clone();
            let matched = pattern_matches(&alternative.node, value, &mut candidate);
            if matched {
                *bindings = candidate;
            }
            matched
        }),
        Pattern::Struct {
            type_name, fields, ..
        } => {
            let Value::Struct {
                name,
                fields: values,
            } = value
            else {
                return false;
            };
            name == type_name
                && fields.iter().all(|field| {
                    values
                        .get(&field.name)
                        .is_some_and(|value| pattern_matches(&field.pattern.node, value, bindings))
                })
        }
        Pattern::Variant { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Limits, evaluate, evaluate_with_limits};
    use crate::{lexer::Lexer, parser::Parser, resolver::Resolver, type_checker::TypeChecker};

    fn report(source: &str) -> Result<String, crate::diagnostics::Diagnostic> {
        let program = Parser::new(Lexer::new(source).tokenize()?).parse()?;
        Resolver::new().resolve(&program)?;
        TypeChecker::new().check(&program)?;
        Ok(evaluate(&program)?.render())
    }

    #[test]
    fn evaluates_constants_in_lexical_order() {
        let source = "fn main() { const base = 7 const answer = base * 6 print(answer) }";
        assert_eq!(
            report(source).unwrap(),
            "main::base = 7\nmain::answer = 42\n"
        );
    }

    #[test]
    fn catches_compile_time_division_by_zero() {
        let error = report("fn main() { const bad = 1 / 0 }").unwrap_err();
        assert!(error.message.contains("division by zero"));
    }

    #[test]
    fn deterministic_resource_budgets_fail_closed() {
        let program = Parser::new(
            Lexer::new("fn main() { const value = [1, 2, 3] }")
                .tokenize()
                .unwrap(),
        )
        .parse()
        .unwrap();
        Resolver::new().resolve(&program).unwrap();
        TypeChecker::new().check(&program).unwrap();
        let error = evaluate_with_limits(
            &program,
            Limits {
                steps: 2,
                ..Limits::default()
            },
        )
        .unwrap_err();
        assert!(error.message.contains("exceeded 2 steps"));
    }
}
