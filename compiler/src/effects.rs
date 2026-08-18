use crate::{
    ast::{Block, Capability, ClosureBody, Expr, Expression, Function, Program, Statement},
    diagnostics::{Diagnostic, DiagnosticKind, Span},
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionEffects {
    pub name: String,
    pub explicit: bool,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub functions: Vec<FunctionEffects>,
}

impl Report {
    pub fn render(&self) -> String {
        let mut output = String::new();
        for function in &self.functions {
            let capabilities = if function.capabilities.is_empty() {
                "Pure".to_owned()
            } else {
                function
                    .capabilities
                    .iter()
                    .map(|capability| capability.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            output.push_str(&function.name);
            output.push_str(" uses ");
            output.push_str(&capabilities);
            if !function.explicit {
                output.push_str(" [inferred]");
            }
            output.push('\n');
        }
        output
    }
}

#[derive(Clone)]
struct FunctionNode<'a> {
    function: &'a Function,
    label: String,
}

#[derive(Default, Clone)]
struct DirectEffects {
    capabilities: BTreeMap<Capability, Span>,
    calls: Vec<(usize, Span)>,
    function_values: Vec<(usize, Span)>,
    closures: Vec<(Span, DirectEffects)>,
}

struct Collector<'a> {
    functions: &'a HashMap<String, usize>,
    methods: &'a HashMap<String, Vec<usize>>,
    external_functions: HashSet<String>,
}

pub fn analyze(program: &Program) -> Result<Report, Diagnostic> {
    let mut nodes = Vec::new();
    for function in &program.functions {
        nodes.push(FunctionNode {
            function,
            label: function.name.clone(),
        });
    }
    for (implementation_index, implementation) in program.implementations.iter().enumerate() {
        for method in &implementation.methods {
            nodes.push(FunctionNode {
                function: method,
                label: format!("impl{implementation_index}.{}", method.name),
            });
        }
    }

    let functions = nodes
        .iter()
        .take(program.functions.len())
        .enumerate()
        .map(|(index, node)| (node.function.name.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut methods = HashMap::<String, Vec<usize>>::new();
    for (index, node) in nodes.iter().enumerate().skip(program.functions.len()) {
        methods
            .entry(node.function.name.clone())
            .or_default()
            .push(index);
    }
    let collector = Collector {
        functions: &functions,
        methods: &methods,
        external_functions: program
            .functions
            .iter()
            .filter(|function| function.external.is_some())
            .map(|function| function.name.clone())
            .collect(),
    };

    let mut direct = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let mut effects = DirectEffects::default();
        if node.function.external.is_some() {
            effects
                .capabilities
                .insert(Capability::Foreign, node.function.span);
        } else {
            collector.collect_block(&node.function.body, &mut effects);
        }
        direct.push(effects);
    }

    let declared = nodes
        .iter()
        .map(|node| {
            node.function.capabilities.as_ref().map(|items| {
                items
                    .iter()
                    .map(|item| item.capability)
                    .collect::<BTreeSet<_>>()
            })
        })
        .collect::<Vec<_>>();
    let mut actual = direct
        .iter()
        .map(|effects| effects.capabilities.clone())
        .collect::<Vec<_>>();

    loop {
        let previous = actual.clone();
        for (index, effects) in direct.iter().enumerate() {
            for (target, call_span) in &effects.calls {
                if let Some(explicit) = &declared[*target] {
                    for capability in explicit {
                        actual[index].entry(*capability).or_insert(*call_span);
                    }
                } else {
                    for capability in previous[*target].keys() {
                        actual[index].entry(*capability).or_insert(*call_span);
                    }
                }
            }
        }
        if actual == previous {
            break;
        }
    }

    for (index, node) in nodes.iter().enumerate() {
        if let Some(allowed) = &declared[index] {
            for (capability, span) in &actual[index] {
                if !allowed.contains(capability) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "function `{}` requires capability `{}` but its `uses` contract does not allow it",
                            node.label,
                            capability.name()
                        ),
                        *span,
                    )
                    .with_help(format!(
                        "add `{}` to the function's `uses` clause or move the operation behind an owned capability value",
                        capability.name()
                    )));
                }
            }
        }
    }

    let contracts = nodes
        .iter()
        .enumerate()
        .map(|(index, _)| {
            declared[index]
                .clone()
                .unwrap_or_else(|| actual[index].keys().copied().collect())
        })
        .collect::<Vec<_>>();
    for effects in &direct {
        validate_hidden_effects(effects, &contracts, &nodes)?;
    }

    Ok(Report {
        functions: nodes
            .iter()
            .enumerate()
            .map(|(index, node)| FunctionEffects {
                name: node.label.clone(),
                explicit: declared[index].is_some(),
                capabilities: contracts[index].iter().copied().collect(),
            })
            .collect(),
    })
}

fn validate_hidden_effects(
    effects: &DirectEffects,
    contracts: &[BTreeSet<Capability>],
    nodes: &[FunctionNode<'_>],
) -> Result<(), Diagnostic> {
    for (target, span) in &effects.function_values {
        if !contracts[*target].is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "capability-bearing function `{}` cannot be erased into an effect-free function value",
                    nodes[*target].label
                ),
                *span,
            )
            .with_help("call it directly or wrap the operation behind an owned capability value"));
        }
    }
    for (span, closure) in &effects.closures {
        let mut required = closure
            .capabilities
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        for (target, _) in &closure.calls {
            required.extend(&contracts[*target]);
        }
        if !required.is_empty() {
            let names = required
                .iter()
                .map(|capability| capability.name())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "closure requires capabilities `{names}` but function-value types are effect-free"
                ),
                *span,
            )
            .with_help("keep capability-bearing work in a directly called named function"));
        }
        validate_hidden_effects(closure, contracts, nodes)?;
    }
    Ok(())
}

impl Collector<'_> {
    fn collect_block(&self, block: &Block, effects: &mut DirectEffects) {
        for statement in &block.statements {
            self.collect_statement(&statement.node, effects);
        }
    }

    fn collect_statement(&self, statement: &Statement, effects: &mut DirectEffects) {
        match statement {
            Statement::Binding { value, .. } | Statement::Return(value) => {
                if let Some(value) = value {
                    self.collect_expr(value, effects, false);
                }
            }
            Statement::Assignment { value, .. } => self.collect_expr(value, effects, false),
            Statement::PlaceAssignment { target, value, .. } => {
                self.collect_expr(target, effects, false);
                self.collect_expr(value, effects, false);
            }
            Statement::Expression(expression) => self.collect_expr(expression, effects, false),
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.collect_expr(condition, effects, false);
                self.collect_block(then_branch, effects);
                if let Some(branch) = else_branch {
                    self.collect_block(branch, effects);
                }
            }
            Statement::While { condition, body } => {
                self.collect_expr(condition, effects, false);
                self.collect_block(body, effects);
            }
            Statement::For {
                start, end, body, ..
            } => {
                self.collect_expr(start, effects, false);
                self.collect_expr(end, effects, false);
                self.collect_block(body, effects);
            }
            Statement::ForEach { iterable, body, .. } => {
                self.collect_expr(iterable, effects, false);
                self.collect_block(body, effects);
            }
            Statement::Loop(body) => self.collect_block(body, effects),
            Statement::Unsafe { capabilities, body } => {
                if let Some(capabilities) = capabilities {
                    for capability in capabilities {
                        effects
                            .capabilities
                            .entry(capability.capability)
                            .or_insert(capability.span);
                    }
                }
                self.collect_block(body, effects);
            }
            Statement::Break | Statement::Continue => {}
        }
    }

    fn collect_expr(&self, expression: &Expr, effects: &mut DirectEffects, direct_callee: bool) {
        match &expression.node {
            Expression::Array(values) => {
                for value in values {
                    self.collect_expr(value, effects, false);
                }
            }
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    effects
                        .capabilities
                        .entry(Capability::FileSystem)
                        .or_insert(expression.span);
                    self.collect_expr(path, effects, false);
                }
            }
            Expression::DataWrite { value, store, .. } => {
                self.collect_expr(value, effects, false);
                self.collect_expr(store, effects, false);
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
                    self.collect_expr(aggregate, effects, false);
                }
                self.collect_expr(store, effects, false);
                if let Some(predicate) = predicate {
                    self.collect_expr(predicate, effects, false);
                }
                if let Some(order) = order {
                    self.collect_expr(&order.key, effects, false);
                }
                if let Some(limit) = limit {
                    self.collect_expr(limit, effects, false);
                }
            }
            Expression::DataRemove {
                store, predicate, ..
            } => {
                self.collect_expr(store, effects, false);
                self.collect_expr(predicate, effects, false);
            }
            Expression::Closure { body, .. } => {
                let mut closure = DirectEffects::default();
                match body {
                    ClosureBody::Expression(value) => self.collect_expr(value, &mut closure, false),
                    ClosureBody::Block(block) => self.collect_block(block, &mut closure),
                }
                effects.closures.push((expression.span, closure));
            }
            Expression::Identifier(name) => {
                if !direct_callee && let Some(target) = self.functions.get(name) {
                    if self.external_functions.contains(name) {
                        effects
                            .capabilities
                            .entry(Capability::Foreign)
                            .or_insert(expression.span);
                    } else {
                        effects.function_values.push((*target, expression.span));
                    }
                }
            }
            Expression::StructConstruct { fields, .. } => {
                for field in fields {
                    self.collect_expr(&field.value, effects, false);
                }
            }
            Expression::FieldAccess { object, .. } => self.collect_expr(object, effects, false),
            Expression::Index { object, index } => {
                self.collect_expr(object, effects, false);
                self.collect_expr(index, effects, false);
            }
            Expression::Subslice { object, start, end } => {
                self.collect_expr(object, effects, false);
                self.collect_expr(start, effects, false);
                self.collect_expr(end, effects, false);
            }
            Expression::Match { value, arms } => {
                self.collect_expr(value, effects, false);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.collect_expr(guard, effects, false);
                    }
                    self.collect_expr(&arm.value, effects, false);
                }
            }
            Expression::Try(value)
            | Expression::Await(value)
            | Expression::Spawn(value)
            | Expression::Move(value)
            | Expression::Dereference(value)
            | Expression::Unary { operand: value, .. } => self.collect_expr(value, effects, false),
            Expression::Borrow { target, .. } => self.collect_expr(target, effects, false),
            Expression::Binary { left, right, .. } => {
                self.collect_expr(left, effects, false);
                self.collect_expr(right, effects, false);
            }
            Expression::Call { callee, arguments } => {
                for argument in arguments {
                    self.collect_expr(argument, effects, false);
                }
                if let Expression::Identifier(name) = &callee.node
                    && let Some(target) = self.functions.get(name)
                {
                    effects.calls.push((*target, expression.span));
                    return;
                }
                if let Some((owner, method)) = static_method(callee) {
                    if let Some(capability) = intrinsic_capability(owner, method) {
                        effects
                            .capabilities
                            .entry(capability)
                            .or_insert(expression.span);
                    } else if let Some(targets) = self.methods.get(method) {
                        effects
                            .calls
                            .extend(targets.iter().map(|target| (*target, expression.span)));
                    }
                }
                self.collect_expr(callee, effects, true);
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => {}
        }
    }
}

fn static_method(expression: &Expr) -> Option<(&str, &str)> {
    let Expression::FieldAccess { object, field, .. } = &expression.node else {
        return None;
    };
    let Expression::Identifier(owner) = &object.node else {
        return None;
    };
    Some((owner, field))
}

fn intrinsic_capability(owner: &str, method: &str) -> Option<Capability> {
    match owner {
        "File" => Some(Capability::FileSystem),
        "Database" if method == "open" => Some(Capability::FileSystem),
        "Async"
            if matches!(
                method,
                "read_text" | "read_bytes" | "write_text" | "write_bytes"
            ) =>
        {
            Some(Capability::FileSystem)
        }
        "Async"
            if matches!(
                method,
                "resolve" | "resolve_timeout" | "connect" | "connect_timeout"
            ) =>
        {
            Some(Capability::Network)
        }
        "Dns" | "TcpListener" | "UdpSocket" | "Tls" | "Http" => Some(Capability::Network),
        "Process" | "Environment" => Some(Capability::Process),
        "CRegistration" if matches!(method, "adopt" | "adopt_async" | "register_async") => {
            Some(Capability::Foreign)
        }
        "Port" | "Mmio" => Some(Capability::DeviceIo),
        "Time" if method == "ticks" => Some(Capability::Timer),
        "Crypto"
            if matches!(
                method,
                "random_bytes" | "random_secret" | "ed25519_generate" | "argon2id_hash_password"
            ) =>
        {
            Some(Capability::Random)
        }
        "Gpu" => Some(Capability::Gpu),
        "Ui" | "Page" | "Window" => Some(Capability::Ui),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::analyze;
    use crate::{lexer::Lexer, parser::Parser, resolver::Resolver, type_checker::TypeChecker};

    fn report(source: &str) -> Result<String, crate::diagnostics::Diagnostic> {
        let tokens = Lexer::new(source).tokenize()?;
        let program = Parser::new(tokens).parse()?;
        Resolver::new().resolve(&program)?;
        TypeChecker::new().check(&program)?;
        Ok(analyze(&program)?.render())
    }

    #[test]
    fn infers_and_propagates_capabilities_deterministically() {
        let source = r#"
fn load(path: Path) -> Result<String, IoError> { return File.read_text(path) }
fn service(path: Path) -> Result<String, IoError> { return load(path) }
fn main() {}
"#;
        assert_eq!(
            report(source).unwrap(),
            "load uses FileSystem [inferred]\nservice uses FileSystem [inferred]\nmain uses Pure [inferred]\n"
        );
    }

    #[test]
    fn explicit_contracts_reject_missing_authority() {
        let source = r#"
fn load(path: Path) -> Result<String, IoError> uses Pure { return File.read_text(path) }
fn main() {}
"#;
        let error = report(source).unwrap_err();
        assert!(error.message.contains("requires capability `FileSystem`"));
        assert!(error.help.unwrap().contains("uses"));
    }
}
