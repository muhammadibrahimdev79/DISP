use crate::{
    ast::{
        BinaryOperator, Block, ClosureBody, Expr, Expression, Function, Pattern, Program, Spanned,
        Statement, StructPatternField, UnaryOperator,
    },
    diagnostics::{Diagnostic, DiagnosticKind, Span},
};

pub use crate::limits::{MAX_EXPANSION_DEPTH, MAX_GENERATED_NODES, MAX_REPEAT_COUNT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub name: String,
    pub span: Span,
    pub generated_nodes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub expansions: Vec<Expansion>,
    pub generated_nodes: usize,
}

impl Report {
    pub fn render(&self) -> String {
        let mut output = String::new();
        for expansion in &self.expansions {
            output.push_str(&expansion.name);
            output.push_str(" generated ");
            output.push_str(&expansion.generated_nodes.to_string());
            output.push_str(" nodes at ");
            output.push_str(&expansion.span.start.line.to_string());
            output.push(':');
            output.push_str(&expansion.span.start.column.to_string());
            output.push('\n');
        }
        output
    }
}

struct Expander {
    report: Report,
    next_pattern_group: u32,
}

pub fn expand(program: &mut Program) -> Result<Report, Diagnostic> {
    let mut expander = Expander {
        report: Report {
            expansions: Vec::new(),
            generated_nodes: 0,
        },
        next_pattern_group: 0,
    };
    for function in &mut program.functions {
        expander.expand_function(function)?;
    }
    for implementation in &mut program.implementations {
        for method in &mut implementation.methods {
            expander.expand_function(method)?;
        }
    }
    Ok(expander.report)
}

impl Expander {
    fn expand_function(&mut self, function: &mut Function) -> Result<(), Diagnostic> {
        if function.external.is_none() {
            self.expand_block(&mut function.body, 0)?;
        }
        Ok(())
    }

    fn expand_block(&mut self, block: &mut Block, depth: usize) -> Result<(), Diagnostic> {
        for statement in &mut block.statements {
            self.expand_statement(&mut statement.node, depth)?;
        }
        Ok(())
    }

    fn expand_statement(
        &mut self,
        statement: &mut Statement,
        depth: usize,
    ) -> Result<(), Diagnostic> {
        match statement {
            Statement::Binding { value, .. } | Statement::Return(value) => {
                if let Some(value) = value {
                    self.expand_expr(value, depth)?;
                }
            }
            Statement::Assignment { value, .. } => self.expand_expr(value, depth)?,
            Statement::PlaceAssignment { target, value, .. } => {
                self.expand_expr(target, depth)?;
                self.expand_expr(value, depth)?;
            }
            Statement::Expression(value) => self.expand_expr(value, depth)?,
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expand_expr(condition, depth)?;
                self.expand_block(then_branch, depth)?;
                if let Some(branch) = else_branch {
                    self.expand_block(branch, depth)?;
                }
            }
            Statement::While { condition, body } => {
                self.expand_expr(condition, depth)?;
                self.expand_block(body, depth)?;
            }
            Statement::For {
                start, end, body, ..
            } => {
                self.expand_expr(start, depth)?;
                self.expand_expr(end, depth)?;
                self.expand_block(body, depth)?;
            }
            Statement::ForEach { iterable, body, .. } => {
                self.expand_expr(iterable, depth)?;
                self.expand_block(body, depth)?;
            }
            Statement::Loop(body) | Statement::Unsafe { body, .. } => {
                self.expand_block(body, depth)?;
            }
            Statement::Break | Statement::Continue => {}
        }
        Ok(())
    }

    fn expand_expr(&mut self, expression: &mut Expr, depth: usize) -> Result<(), Diagnostic> {
        if let Some(name) = meta_call_name(expression) {
            return self.expand_meta(expression, name, depth + 1);
        }
        match &mut expression.node {
            Expression::Array(values) => {
                for value in values {
                    self.expand_expr(value, depth)?;
                }
            }
            Expression::StructConstruct { fields, .. } => {
                for field in fields {
                    self.expand_expr(&mut field.value, depth)?;
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
            } => self.expand_expr(object, depth)?,
            Expression::Borrow { target, .. } => self.expand_expr(target, depth)?,
            Expression::Index { object, index } => {
                self.expand_expr(object, depth)?;
                self.expand_expr(index, depth)?;
            }
            Expression::Subslice { object, start, end } => {
                self.expand_expr(object, depth)?;
                self.expand_expr(start, depth)?;
                self.expand_expr(end, depth)?;
            }
            Expression::Match { value, arms } => {
                self.expand_expr(value, depth)?;
                let original = std::mem::take(arms);
                for mut arm in original {
                    if let Some(guard) = &mut arm.guard {
                        self.expand_expr(guard, depth)?;
                    }
                    self.expand_expr(&mut arm.value, depth)?;
                    let alternatives = pattern_alternatives(&arm.pattern)?;
                    if arms.len().saturating_add(alternatives.len()) > MAX_REPEAT_COUNT {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "match expands to more than {MAX_REPEAT_COUNT} pattern alternatives"
                            ),
                            arm.pattern.span,
                        )
                        .with_help("reduce nested `|` alternatives in this match"));
                    }
                    let group = (alternatives.len() > 1).then(|| {
                        let group = self.next_pattern_group;
                        self.next_pattern_group = self.next_pattern_group.wrapping_add(1);
                        group
                    });
                    for pattern in alternatives {
                        let mut alternative = arm.clone();
                        alternative.pattern = pattern;
                        alternative.alternative_group = group;
                        arms.push(alternative);
                    }
                }
            }
            Expression::Binary { left, right, .. } => {
                self.expand_expr(left, depth)?;
                self.expand_expr(right, depth)?;
            }
            Expression::Call { callee, arguments } => {
                self.expand_expr(callee, depth)?;
                for argument in arguments {
                    self.expand_expr(argument, depth)?;
                }
            }
            Expression::Closure { body, .. } => match body {
                ClosureBody::Expression(value) => self.expand_expr(value, depth)?,
                ClosureBody::Block(block) => self.expand_block(block, depth)?,
            },
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    self.expand_expr(path, depth)?;
                }
            }
            Expression::DataWrite { value, store, .. } => {
                self.expand_expr(value, depth)?;
                self.expand_expr(store, depth)?;
            }
            Expression::DataQuery {
                store,
                predicate,
                order,
                limit,
                ..
            } => {
                self.expand_expr(store, depth)?;
                if let Some(predicate) = predicate {
                    self.expand_expr(predicate, depth)?;
                }
                if let Some(order) = order {
                    self.expand_expr(&mut order.key, depth)?;
                }
                if let Some(limit) = limit {
                    self.expand_expr(limit, depth)?;
                }
            }
            Expression::DataRemove {
                store, predicate, ..
            } => {
                self.expand_expr(store, depth)?;
                self.expand_expr(predicate, depth)?;
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

    fn expand_meta(
        &mut self,
        expression: &mut Expr,
        name: &'static str,
        depth: usize,
    ) -> Result<(), Diagnostic> {
        if depth > MAX_EXPANSION_DEPTH {
            return Err(expansion_error(
                format!("structured expansion exceeded depth {MAX_EXPANSION_DEPTH}"),
                expression.span,
            ));
        }
        let Expression::Call { arguments, .. } = &expression.node else {
            unreachable!("meta_call_name only recognizes calls")
        };
        if arguments.len() != 2 {
            return Err(expansion_error(
                format!("Meta.{name} expects exactly two arguments"),
                expression.span,
            ));
        }
        let count = expansion_count(&arguments[0])?;
        if count > MAX_REPEAT_COUNT {
            return Err(expansion_error(
                format!("Meta.{name} count {count} exceeds the limit {MAX_REPEAT_COUNT}"),
                arguments[0].span,
            ));
        }
        let mut template = match name {
            "repeat" => arguments[1].clone(),
            "map" => {
                let Expression::Closure {
                    move_captures: false,
                    parameters,
                    body: ClosureBody::Expression(body),
                    ..
                } = &arguments[1].node
                else {
                    return Err(expansion_error(
                        "Meta.map expects a non-moving one-parameter expression closure",
                        arguments[1].span,
                    ));
                };
                if parameters.len() != 1 {
                    return Err(expansion_error(
                        "Meta.map expects exactly one mapper parameter",
                        arguments[1].span,
                    ));
                }
                let mut generated = Vec::with_capacity(count);
                for index in 0..count {
                    let mut value = (**body).clone();
                    substitute_bound_identifier(
                        &mut value,
                        &parameters[0].name,
                        index as u128,
                        expression.span,
                    );
                    generated.push(value);
                }
                let generated_nodes = generated
                    .iter()
                    .map(expression_nodes)
                    .try_fold(1usize, usize::checked_add)
                    .ok_or_else(|| {
                        expansion_error("generated syntax node count overflowed", expression.span)
                    })?;
                self.charge(name, expression.span, generated_nodes)?;
                expression.node = Expression::Array(generated);
                self.expand_expr(expression, depth)?;
                return Ok(());
            }
            _ => unreachable!("meta_call_name restricts names"),
        };
        self.expand_expr(&mut template, depth)?;
        let template_nodes = expression_nodes(&template);
        let generated_nodes = template_nodes
            .checked_mul(count)
            .and_then(|nodes| nodes.checked_add(1))
            .ok_or_else(|| {
                expansion_error("generated syntax node count overflowed", expression.span)
            })?;
        self.charge(name, expression.span, generated_nodes)?;
        expression.node = Expression::Array(vec![template; count]);
        Ok(())
    }

    fn charge(&mut self, name: &str, span: Span, nodes: usize) -> Result<(), Diagnostic> {
        self.report.generated_nodes = self
            .report
            .generated_nodes
            .checked_add(nodes)
            .ok_or_else(|| expansion_error("generated syntax node count overflowed", span))?;
        if self.report.generated_nodes > MAX_GENERATED_NODES {
            return Err(expansion_error(
                format!("structured expansion exceeded {MAX_GENERATED_NODES} generated nodes"),
                span,
            ));
        }
        self.report.expansions.push(Expansion {
            name: format!("Meta.{name}"),
            span,
            generated_nodes: nodes,
        });
        Ok(())
    }
}

fn meta_call_name(expression: &Expr) -> Option<&'static str> {
    let Expression::Call { callee, .. } = &expression.node else {
        return None;
    };
    let Expression::FieldAccess { object, field, .. } = &callee.node else {
        return None;
    };
    if !matches!(&object.node, Expression::Identifier(owner) if owner == "Meta") {
        return None;
    }
    match field.as_str() {
        "repeat" => Some("repeat"),
        "map" => Some("map"),
        _ => None,
    }
}

fn expansion_count(expression: &Expr) -> Result<usize, Diagnostic> {
    fn integer(expression: &Expr) -> Option<(bool, u128)> {
        match &expression.node {
            Expression::Integer(value) => Some((false, *value)),
            Expression::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } => integer(operand).map(|(_, value)| (value != 0, value)),
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let (left_negative, left) = integer(left)?;
                let (right_negative, right) = integer(right)?;
                if left_negative || right_negative {
                    return None;
                }
                match operator {
                    BinaryOperator::Add => left.checked_add(right).map(|value| (false, value)),
                    BinaryOperator::Subtract => left.checked_sub(right).map(|value| (false, value)),
                    BinaryOperator::Multiply => left.checked_mul(right).map(|value| (false, value)),
                    BinaryOperator::Divide if right != 0 => Some((false, left / right)),
                    BinaryOperator::Remainder if right != 0 => Some((false, left % right)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    let Some((false, count)) = integer(expression) else {
        return Err(expansion_error(
            "structured expansion count must be a non-negative constant integer expression",
            expression.span,
        ));
    };
    usize::try_from(count).map_err(|_| {
        expansion_error(
            "structured expansion count does not fit the host-independent limit",
            expression.span,
        )
    })
}

fn substitute_bound_identifier(expression: &mut Expr, name: &str, value: u128, span: Span) {
    if matches!(&expression.node, Expression::Identifier(identifier) if identifier == name) {
        expression.node = Expression::Integer(value);
        expression.span = span;
        return;
    }
    match &mut expression.node {
        Expression::Array(values) => {
            for item in values {
                substitute_bound_identifier(item, name, value, span);
            }
        }
        Expression::StructConstruct { fields, .. } => {
            for field in fields {
                substitute_bound_identifier(&mut field.value, name, value, span);
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
        } => substitute_bound_identifier(object, name, value, span),
        Expression::Borrow { target, .. } => substitute_bound_identifier(target, name, value, span),
        Expression::Index { object, index } => {
            substitute_bound_identifier(object, name, value, span);
            substitute_bound_identifier(index, name, value, span);
        }
        Expression::Subslice { object, start, end } => {
            substitute_bound_identifier(object, name, value, span);
            substitute_bound_identifier(start, name, value, span);
            substitute_bound_identifier(end, name, value, span);
        }
        Expression::Match {
            value: matched,
            arms,
        } => {
            substitute_bound_identifier(matched, name, value, span);
            for arm in arms {
                if !pattern_binds(&arm.pattern.node, name) {
                    if let Some(guard) = &mut arm.guard {
                        substitute_bound_identifier(guard, name, value, span);
                    }
                    substitute_bound_identifier(&mut arm.value, name, value, span);
                }
            }
        }
        Expression::Binary { left, right, .. } => {
            substitute_bound_identifier(left, name, value, span);
            substitute_bound_identifier(right, name, value, span);
        }
        Expression::Call { callee, arguments } => {
            substitute_bound_identifier(callee, name, value, span);
            for argument in arguments {
                substitute_bound_identifier(argument, name, value, span);
            }
        }
        Expression::Closure {
            parameters, body, ..
        } => {
            if parameters.iter().any(|parameter| parameter.name == name) {
                return;
            }
            match body {
                ClosureBody::Expression(body) => {
                    substitute_bound_identifier(body, name, value, span);
                }
                ClosureBody::Block(_) => {}
            }
        }
        Expression::DataStore { path } => {
            if let Some(path) = path {
                substitute_bound_identifier(path, name, value, span);
            }
        }
        Expression::DataWrite {
            value: written,
            store,
            ..
        } => {
            substitute_bound_identifier(written, name, value, span);
            substitute_bound_identifier(store, name, value, span);
        }
        Expression::DataQuery {
            store,
            predicate,
            order,
            limit,
            ..
        } => {
            substitute_bound_identifier(store, name, value, span);
            if let Some(predicate) = predicate {
                substitute_bound_identifier(predicate, name, value, span);
            }
            if let Some(order) = order {
                substitute_bound_identifier(&mut order.key, name, value, span);
            }
            if let Some(limit) = limit {
                substitute_bound_identifier(limit, name, value, span);
            }
        }
        Expression::DataRemove {
            store, predicate, ..
        } => {
            substitute_bound_identifier(store, name, value, span);
            substitute_bound_identifier(predicate, name, value, span);
        }
        Expression::Integer(_)
        | Expression::Float(_)
        | Expression::String(_)
        | Expression::Character(_)
        | Expression::Bool(_)
        | Expression::Identifier(_) => {}
    }
}

fn pattern_binds(pattern: &crate::ast::Pattern, name: &str) -> bool {
    match pattern {
        crate::ast::Pattern::Binding(binding) => binding == name,
        crate::ast::Pattern::Or(alternatives) => alternatives
            .iter()
            .any(|alternative| pattern_binds(&alternative.node, name)),
        crate::ast::Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|field| pattern_binds(&field.pattern.node, name)),
        crate::ast::Pattern::Variant { arguments, .. } => arguments
            .iter()
            .any(|argument| pattern_binds(&argument.node, name)),
        _ => false,
    }
}

fn pattern_alternatives(pattern: &Spanned<Pattern>) -> Result<Vec<Spanned<Pattern>>, Diagnostic> {
    match &pattern.node {
        Pattern::Or(alternatives) => {
            let expected = alternatives
                .first()
                .map(|alternative| pattern_binding_names(&alternative.node))
                .unwrap_or_default();
            let mut expanded = Vec::new();
            for alternative in alternatives {
                let actual = pattern_binding_names(&alternative.node);
                if actual != expected {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "every `|` pattern alternative must bind the same names",
                        alternative.span,
                    )
                    .with_help(
                        "add or remove bindings so every alternative exposes one contract",
                    ));
                }
                expanded.extend(pattern_alternatives(alternative)?);
                if expanded.len() > MAX_REPEAT_COUNT {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("pattern expands to more than {MAX_REPEAT_COUNT} alternatives"),
                        pattern.span,
                    ));
                }
            }
            Ok(expanded)
        }
        Pattern::Variant {
            type_name,
            variant,
            arguments,
        } => {
            let mut products = vec![Vec::new()];
            for argument in arguments {
                let alternatives = pattern_alternatives(argument)?;
                let mut next = Vec::new();
                for product in products {
                    for alternative in &alternatives {
                        let mut product = product.clone();
                        product.push(alternative.clone());
                        next.push(product);
                        if next.len() > MAX_REPEAT_COUNT {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "pattern expands to more than {MAX_REPEAT_COUNT} alternatives"
                                ),
                                pattern.span,
                            ));
                        }
                    }
                }
                products = next;
            }
            Ok(products
                .into_iter()
                .map(|arguments| Spanned {
                    node: Pattern::Variant {
                        type_name: type_name.clone(),
                        variant: variant.clone(),
                        arguments,
                    },
                    span: pattern.span,
                })
                .collect())
        }
        Pattern::Struct {
            type_name,
            fields,
            rest,
        } => {
            let mut products = vec![Vec::<StructPatternField>::new()];
            for field in fields {
                let alternatives = pattern_alternatives(&field.pattern)?;
                let mut next = Vec::new();
                for product in products {
                    for alternative in &alternatives {
                        let mut product = product.clone();
                        product.push(StructPatternField {
                            name: field.name.clone(),
                            name_span: field.name_span,
                            pattern: alternative.clone(),
                        });
                        next.push(product);
                        if next.len() > MAX_REPEAT_COUNT {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "pattern expands to more than {MAX_REPEAT_COUNT} alternatives"
                                ),
                                pattern.span,
                            ));
                        }
                    }
                }
                products = next;
            }
            Ok(products
                .into_iter()
                .map(|fields| Spanned {
                    node: Pattern::Struct {
                        type_name: type_name.clone(),
                        fields,
                        rest: *rest,
                    },
                    span: pattern.span,
                })
                .collect())
        }
        _ => Ok(vec![pattern.clone()]),
    }
}

fn pattern_binding_names(pattern: &Pattern) -> std::collections::BTreeSet<String> {
    fn collect(pattern: &Pattern, names: &mut std::collections::BTreeSet<String>) {
        match pattern {
            Pattern::Binding(name) => {
                names.insert(name.clone());
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    collect(&alternative.node, names);
                }
            }
            Pattern::Struct { fields, .. } => {
                for field in fields {
                    collect(&field.pattern.node, names);
                }
            }
            Pattern::Variant { arguments, .. } => {
                for argument in arguments {
                    collect(&argument.node, names);
                }
            }
            Pattern::Wildcard
            | Pattern::Integer(_)
            | Pattern::NegativeInteger(_)
            | Pattern::String(_)
            | Pattern::Character(_)
            | Pattern::Bool(_) => {}
        }
    }
    let mut names = std::collections::BTreeSet::new();
    collect(pattern, &mut names);
    names
}

fn expression_nodes(expression: &Expr) -> usize {
    let children = match &expression.node {
        Expression::Array(values) => values.iter().map(expression_nodes).sum(),
        Expression::StructConstruct { fields, .. } => fields
            .iter()
            .map(|field| expression_nodes(&field.value))
            .sum(),
        Expression::FieldAccess { object, .. }
        | Expression::Try(object)
        | Expression::Await(object)
        | Expression::Spawn(object)
        | Expression::Move(object)
        | Expression::Borrow { target: object, .. }
        | Expression::Dereference(object)
        | Expression::Unary {
            operand: object, ..
        } => expression_nodes(object),
        Expression::Index { object, index } => expression_nodes(object) + expression_nodes(index),
        Expression::Subslice { object, start, end } => {
            expression_nodes(object) + expression_nodes(start) + expression_nodes(end)
        }
        Expression::Match { value, arms } => {
            expression_nodes(value)
                + arms
                    .iter()
                    .map(|arm| {
                        arm.guard.as_ref().map_or(0, expression_nodes)
                            + expression_nodes(&arm.value)
                    })
                    .sum::<usize>()
        }
        Expression::Binary { left, right, .. } => expression_nodes(left) + expression_nodes(right),
        Expression::Call { callee, arguments } => {
            expression_nodes(callee) + arguments.iter().map(expression_nodes).sum::<usize>()
        }
        Expression::Closure { body, .. } => match body {
            ClosureBody::Expression(value) => expression_nodes(value),
            ClosureBody::Block(_) => 1,
        },
        Expression::DataStore { path } => path.as_deref().map_or(0, expression_nodes),
        Expression::DataWrite { value, store, .. } => {
            expression_nodes(value) + expression_nodes(store)
        }
        Expression::DataQuery {
            store,
            predicate,
            order,
            limit,
            ..
        } => {
            expression_nodes(store)
                + predicate.as_deref().map_or(0, expression_nodes)
                + order
                    .as_ref()
                    .map_or(0, |order| expression_nodes(&order.key))
                + limit.as_deref().map_or(0, expression_nodes)
        }
        Expression::DataRemove {
            store, predicate, ..
        } => expression_nodes(store) + expression_nodes(predicate),
        Expression::Integer(_)
        | Expression::Float(_)
        | Expression::String(_)
        | Expression::Character(_)
        | Expression::Bool(_)
        | Expression::Identifier(_) => 0,
    };
    1usize.saturating_add(children)
}

fn expansion_error(message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Parse, message, span)
        .with_help("structured compile-time expansion is bounded and does not execute plugins")
}
