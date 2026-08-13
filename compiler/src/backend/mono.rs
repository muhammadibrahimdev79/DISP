use super::layout::substitute;
use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    hir, mir,
};
use std::collections::{BTreeSet, HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionInstance {
    pub function: hir::FunctionId,
    pub substitutions: Vec<hir::Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeInstance {
    pub ty: hir::Type,
}

#[derive(Debug, Clone)]
pub struct MonoProgram {
    pub instances: Vec<FunctionInstance>,
    pub types: Vec<TypeInstance>,
    pub entry: FunctionInstance,
}

pub fn collect(program: &mir::Program) -> Result<MonoProgram, Diagnostic> {
    let main = program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| error("native program has no main function"))?;
    let entry = FunctionInstance {
        function: main.id,
        substitutions: vec![],
    };
    let mut queue = VecDeque::from([entry.clone()]);
    let mut seen = BTreeSet::new();
    while let Some(instance) = queue.pop_front() {
        if !seen.insert(instance.clone()) {
            continue;
        }
        if seen.len() > 16_384 {
            return Err(error("monomorphization exceeded 16384 function instances"));
        }
        let function = program
            .functions
            .get(instance.function.0)
            .ok_or_else(|| error("monomorphization found an invalid function id"))?;
        if function.generic_parameters.len() != instance.substitutions.len() {
            return Err(error(&format!(
                "function `{}` has unresolved generic substitutions",
                function.name
            )));
        }
        for block in &function.blocks {
            for statement in &block.statements {
                if let mir::StatementKind::Assign(_, mir::Rvalue::Function(target)) =
                    &statement.kind
                {
                    let target = program.functions.get(target.0).ok_or_else(|| {
                        error("function value references an invalid function identity")
                    })?;
                    if !target.generic_parameters.is_empty() {
                        return Err(error(
                            "generic function value reached monomorphization without substitutions",
                        ));
                    }
                    queue.push_back(FunctionInstance {
                        function: target.id,
                        substitutions: vec![],
                    });
                }
                if let mir::StatementKind::Assign(
                    _,
                    mir::Rvalue::Closure {
                        function: target, ..
                    },
                ) = &statement.kind
                {
                    let target = program
                        .functions
                        .get(target.0)
                        .ok_or_else(|| error("closure references an invalid function identity"))?;
                    queue.push_back(FunctionInstance {
                        function: target.id,
                        substitutions: instance.substitutions.clone(),
                    });
                }
            }
            if let mir::Terminator::Call {
                target,
                substitutions,
                arguments,
                ..
            } = &block.terminator
                && let Some(next) = resolve_target(
                    program,
                    function,
                    &instance,
                    target,
                    substitutions,
                    arguments,
                )?
            {
                queue.push_back(next);
            }
            if let mir::Terminator::Spawn {
                target,
                substitutions,
                arguments,
                ..
            } = &block.terminator
                && let Some(next) = resolve_target(
                    program,
                    function,
                    &instance,
                    &hir::CallTarget::Function(*target),
                    substitutions,
                    arguments,
                )?
            {
                queue.push_back(next);
            }
        }
    }
    let instances = seen.into_iter().collect::<Vec<_>>();
    let mut types = BTreeSet::new();
    let generic_names = program
        .functions
        .iter()
        .flat_map(|function| function.generic_parameters.iter().cloned())
        .chain(
            program
                .structs
                .iter()
                .flat_map(|declaration| declaration.generic_parameters.iter().cloned()),
        )
        .chain(
            program
                .enums
                .iter()
                .flat_map(|declaration| declaration.generic_parameters.iter().cloned()),
        )
        .collect::<BTreeSet<_>>();
    for instance in &instances {
        let function = &program.functions[instance.function.0];
        let substitutions = mapping(function, instance);
        for local in &function.locals {
            collect_type(
                program,
                &substitute(&local.ty, &substitutions),
                &mut types,
                &generic_names,
            )?;
        }
        for block in &function.blocks {
            for statement in &block.statements {
                collect_statement_types(
                    program,
                    function,
                    statement,
                    &substitutions,
                    &mut types,
                    &generic_names,
                )?;
            }
            collect_terminator_types(
                program,
                function,
                &block.terminator,
                &substitutions,
                &mut types,
                &generic_names,
            )?;
        }
    }
    Ok(MonoProgram {
        instances,
        types: types.into_iter().collect(),
        entry,
    })
}

fn collect_place_type(
    program: &mir::Program,
    function: &mir::Function,
    place: &mir::Place,
    substitutions: &HashMap<String, hir::Type>,
    types: &mut BTreeSet<TypeInstance>,
    generic_names: &BTreeSet<String>,
) -> Result<(), Diagnostic> {
    let ty = projected_place_type(program, function, place)
        .ok_or_else(|| error("MIR place has no type during monomorphization"))?;
    collect_type(
        program,
        &substitute(&ty, substitutions),
        types,
        generic_names,
    )
}

fn collect_operand_type(
    program: &mir::Program,
    function: &mir::Function,
    operand: &mir::Operand,
    substitutions: &HashMap<String, hir::Type>,
    types: &mut BTreeSet<TypeInstance>,
    generic_names: &BTreeSet<String>,
) -> Result<(), Diagnostic> {
    if let mir::Operand::Move(place) | mir::Operand::Copy(place) = operand {
        collect_place_type(
            program,
            function,
            place,
            substitutions,
            types,
            generic_names,
        )?;
    }
    Ok(())
}

fn collect_rvalue_types(
    program: &mir::Program,
    function: &mir::Function,
    rvalue: &mir::Rvalue,
    substitutions: &HashMap<String, hir::Type>,
    types: &mut BTreeSet<TypeInstance>,
    generic_names: &BTreeSet<String>,
) -> Result<(), Diagnostic> {
    let mut operand = |value| {
        collect_operand_type(
            program,
            function,
            value,
            substitutions,
            types,
            generic_names,
        )
    };
    match rvalue {
        mir::Rvalue::Use(value)
        | mir::Rvalue::UnaryOp(_, value)
        | mir::Rvalue::Cast { operand: value, .. } => operand(value)?,
        mir::Rvalue::BinaryOp(_, left, right) => {
            operand(left)?;
            operand(right)?;
        }
        mir::Rvalue::Closure { captures, .. } | mir::Rvalue::Aggregate(_, captures) => {
            for value in captures {
                operand(value)?;
            }
        }
        mir::Rvalue::BorrowShared(place)
        | mir::Rvalue::BorrowMut(place)
        | mir::Rvalue::RawAddress { place, .. }
        | mir::Rvalue::Discriminant(place)
        | mir::Rvalue::Len(place) => collect_place_type(
            program,
            function,
            place,
            substitutions,
            types,
            generic_names,
        )?,
        mir::Rvalue::Function(_) => {}
    }
    if let mir::Rvalue::Cast { target, .. } = rvalue {
        collect_type(
            program,
            &substitute(target, substitutions),
            types,
            generic_names,
        )?;
    }
    Ok(())
}

fn collect_statement_types(
    program: &mir::Program,
    function: &mir::Function,
    statement: &mir::Statement,
    substitutions: &HashMap<String, hir::Type>,
    types: &mut BTreeSet<TypeInstance>,
    generic_names: &BTreeSet<String>,
) -> Result<(), Diagnostic> {
    match &statement.kind {
        mir::StatementKind::Assign(place, rvalue) => {
            collect_place_type(
                program,
                function,
                place,
                substitutions,
                types,
                generic_names,
            )?;
            collect_rvalue_types(
                program,
                function,
                rvalue,
                substitutions,
                types,
                generic_names,
            )?;
        }
        mir::StatementKind::Drop { place, .. } => collect_place_type(
            program,
            function,
            place,
            substitutions,
            types,
            generic_names,
        )?,
        _ => {}
    }
    Ok(())
}

fn collect_terminator_types(
    program: &mir::Program,
    function: &mir::Function,
    terminator: &mir::Terminator,
    substitutions: &HashMap<String, hir::Type>,
    types: &mut BTreeSet<TypeInstance>,
    generic_names: &BTreeSet<String>,
) -> Result<(), Diagnostic> {
    let mut operand = |value| {
        collect_operand_type(
            program,
            function,
            value,
            substitutions,
            types,
            generic_names,
        )
    };
    match terminator {
        mir::Terminator::SwitchBool { condition, .. }
        | mir::Terminator::SwitchValue {
            discriminant: condition,
            ..
        }
        | mir::Terminator::SwitchEnum {
            discriminant: condition,
            ..
        } => operand(condition)?,
        mir::Terminator::Call {
            arguments,
            destination,
            ..
        }
        | mir::Terminator::Spawn {
            arguments,
            destination,
            ..
        } => {
            for argument in arguments {
                operand(argument)?;
            }
            collect_place_type(
                program,
                function,
                destination,
                substitutions,
                types,
                generic_names,
            )?;
        }
        mir::Terminator::Await {
            future,
            destination,
            ..
        } => {
            operand(future)?;
            collect_place_type(
                program,
                function,
                destination,
                substitutions,
                types,
                generic_names,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn collect_type(
    program: &mir::Program,
    ty: &hir::Type,
    types: &mut BTreeSet<TypeInstance>,
    generic_names: &BTreeSet<String>,
) -> Result<(), Diagnostic> {
    match ty {
        hir::Type::Struct(id, arguments) => {
            let instance = TypeInstance { ty: ty.clone() };
            if !types.insert(instance) {
                return Ok(());
            }
            let declaration = program
                .structs
                .get(id.0)
                .ok_or_else(|| error("monomorphization found an invalid struct id"))?;
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            for field in &declaration.fields {
                collect_type(
                    program,
                    &substitute(&field.ty, &substitutions),
                    types,
                    generic_names,
                )?;
            }
        }
        hir::Type::Enum(id, arguments) => {
            let instance = TypeInstance { ty: ty.clone() };
            if !types.insert(instance) {
                return Ok(());
            }
            let declaration = program
                .enums
                .get(id.0)
                .ok_or_else(|| error("monomorphization found an invalid enum id"))?;
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            for variant in &declaration.variants {
                for payload in &variant.payload {
                    collect_type(
                        program,
                        &substitute(payload, &substitutions),
                        types,
                        generic_names,
                    )?;
                }
            }
        }
        hir::Type::Option(inner) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, inner, types, generic_names)?;
            }
        }
        hir::Type::Result(ok, error_ty) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, ok, types, generic_names)?;
                collect_type(program, error_ty, types, generic_names)?;
            }
        }
        hir::Type::Array(element, _) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, element, types, generic_names)?;
            }
        }
        hir::Type::Slice(element) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, element, types, generic_names)?;
            }
        }
        hir::Type::List(element) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, element, types, generic_names)?;
            }
        }
        hir::Type::Map(key, value) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, key, types, generic_names)?;
                collect_type(program, value, types, generic_names)?;
            }
        }
        hir::Type::Set(element) => {
            if types.insert(TypeInstance { ty: ty.clone() }) {
                collect_type(program, element, types, generic_names)?;
            }
        }
        hir::Type::Thread(result) => {
            collect_type(program, result, types, generic_names)?;
        }
        hir::Type::Future(result) => {
            collect_type(program, result, types, generic_names)?;
        }
        hir::Type::IpAddress
        | hir::Type::SocketAddress
        | hir::Type::TcpStream
        | hir::Type::TlsStream
        | hir::Type::TcpListener
        | hir::Type::UdpSocket
        | hir::Type::UdpDatagram => {}
        hir::Type::Task(result) => {
            collect_type(program, result, types, generic_names)?;
        }
        hir::Type::Mutex(value) | hir::Type::MutexGuard(value) => {
            collect_type(program, value, types, generic_names)?;
        }
        hir::Type::Reference { inner, .. } | hir::Type::RawPointer { inner, .. } => {
            collect_type(program, inner, types, generic_names)?;
        }
        hir::Type::Function(arguments, result) => {
            for argument in arguments {
                collect_type(program, argument, types, generic_names)?;
            }
            collect_type(program, result, types, generic_names)?;
        }
        hir::Type::Generic(name) if generic_names.contains(name) => {
            return Err(error(&format!(
                "unresolved generic `{name}` reached native type monomorphization"
            )));
        }
        hir::Type::Generic(_) => {}
        hir::Type::Unknown => {
            return Err(error("unknown type reached native type monomorphization"));
        }
        _ => {}
    }
    Ok(())
}

pub fn mapping(
    function: &mir::Function,
    instance: &FunctionInstance,
) -> HashMap<String, hir::Type> {
    function
        .generic_parameters
        .iter()
        .cloned()
        .zip(instance.substitutions.iter().cloned())
        .collect()
}

pub fn resolve_target(
    program: &mir::Program,
    caller: &mir::Function,
    instance: &FunctionInstance,
    target: &hir::CallTarget,
    substitutions: &[hir::Type],
    arguments: &[mir::Operand],
) -> Result<Option<FunctionInstance>, Diagnostic> {
    let caller_map = mapping(caller, instance);
    match target {
        hir::CallTarget::Intrinsic(_) | hir::CallTarget::Callable => Ok(None),
        hir::CallTarget::Function(function) => {
            let callee = program
                .functions
                .get(function.0)
                .ok_or_else(|| error("call targets an invalid function"))?;
            let supplied = substitutions
                .iter()
                .map(|ty| substitute(ty, &caller_map))
                .collect::<Vec<_>>();
            if supplied
                .iter()
                .all(|ty| !matches!(ty, hir::Type::Generic(_)))
            {
                return Ok(Some(FunctionInstance {
                    function: *function,
                    substitutions: supplied,
                }));
            }
            let mut inferred = HashMap::new();
            for (name, ty) in callee.generic_parameters.iter().zip(&supplied) {
                if !matches!(ty, hir::Type::Generic(_)) {
                    inferred.insert(name.clone(), ty.clone());
                }
            }
            for (parameter, argument) in callee
                .locals
                .iter()
                .filter(|local| local.kind == mir::LocalKind::Argument)
                .zip(arguments)
            {
                if let Some(actual) = operand_type(program, caller, argument) {
                    let actual = substitute(&actual, &caller_map);
                    if !match_type(&parameter.ty, &actual, &mut inferred) {
                        return Err(error("call arguments do not match during monomorphization"));
                    }
                }
            }
            let substitutions = callee
                .generic_parameters
                .iter()
                .map(|name| {
                    inferred
                        .get(name)
                        .cloned()
                        .ok_or_else(|| error("generic call did not infer all type parameters"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(FunctionInstance {
                function: *function,
                substitutions,
            }))
        }
        hir::CallTarget::TraitMethod { trait_id, method } => {
            let receiver = arguments
                .first()
                .ok_or_else(|| error("trait call has no receiver"))?;
            let receiver_ty = operand_type(program, caller, receiver)
                .map(|ty| substitute(&ty, &caller_map))
                .ok_or_else(|| error("trait receiver has no concrete type"))?;
            let receiver_ty = match receiver_ty {
                hir::Type::Reference { inner, .. } => *inner,
                other => other,
            };
            for implementation in &program.implementations {
                if implementation.trait_id != Some(*trait_id) {
                    continue;
                }
                let mut inferred = HashMap::new();
                if match_type(&implementation.target, &receiver_ty, &mut inferred) {
                    let target = *implementation
                        .methods
                        .get(*method)
                        .ok_or_else(|| error("trait implementation is missing resolved method"))?;
                    let substitutions = implementation
                        .generic_parameters
                        .iter()
                        .map(|name| {
                            inferred.get(name).cloned().ok_or_else(|| {
                                error("generic trait implementation did not infer all parameters")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(Some(FunctionInstance {
                        function: target,
                        substitutions,
                    }));
                }
            }
            Err(error(
                "no concrete trait implementation during monomorphization",
            ))
        }
    }
}

fn projected_place_type(
    program: &mir::Program,
    function: &mir::Function,
    place: &mir::Place,
) -> Option<hir::Type> {
    let mut ty = function.locals.get(place.local.0)?.ty.clone();
    for projection in &place.projections {
        ty = match (projection, ty) {
            (mir::Projection::SafeDereference, hir::Type::Reference { inner, .. })
            | (mir::Projection::SafeDereference, hir::Type::MutexGuard(inner))
            | (mir::Projection::RawDereference, hir::Type::RawPointer { inner, .. }) => *inner,
            (mir::Projection::Field(index), hir::Type::Struct(id, arguments)) => {
                let declaration = program.structs.get(id.0)?;
                let substitutions = declaration
                    .generic_parameters
                    .iter()
                    .cloned()
                    .zip(arguments)
                    .collect::<HashMap<_, _>>();
                substitute(&declaration.fields.get(*index)?.ty, &substitutions)
            }
            (mir::Projection::VariantField(variant, index), hir::Type::Enum(id, arguments)) => {
                let declaration = program.enums.get(id.0)?;
                let substitutions = declaration
                    .generic_parameters
                    .iter()
                    .cloned()
                    .zip(arguments)
                    .collect::<HashMap<_, _>>();
                let variant = declaration
                    .variants
                    .iter()
                    .find(|candidate| candidate.id == *variant)?;
                substitute(variant.payload.get(*index)?, &substitutions)
            }
            (mir::Projection::VariantField(variant, 0), hir::Type::Option(inner))
                if *variant == hir::builtin_variant("Some") =>
            {
                *inner
            }
            (mir::Projection::VariantField(variant, 0), hir::Type::Result(ok, error)) => {
                if *variant == hir::builtin_variant("Ok") {
                    *ok
                } else if *variant == hir::builtin_variant("Err") {
                    *error
                } else {
                    return None;
                }
            }
            (
                mir::Projection::Index { .. },
                hir::Type::Array(element, _)
                | hir::Type::Slice(element)
                | hir::Type::List(element)
                | hir::Type::Set(element),
            ) => *element,
            (
                mir::Projection::Subslice { .. },
                hir::Type::Array(element, _) | hir::Type::Slice(element) | hir::Type::List(element),
            ) => hir::Type::Slice(element),
            (mir::Projection::Subslice { .. }, hir::Type::String | hir::Type::Str) => {
                hir::Type::Str
            }
            _ => return None,
        };
    }
    Some(ty)
}

fn operand_type(
    program: &mir::Program,
    function: &mir::Function,
    operand: &mir::Operand,
) -> Option<hir::Type> {
    match operand {
        mir::Operand::Move(place) | mir::Operand::Copy(place) => {
            projected_place_type(program, function, place)
        }
        mir::Operand::Constant(constant) => Some(match constant {
            mir::Constant::Signed(_, width) => hir::Type::Int {
                signed: true,
                width: *width,
            },
            mir::Constant::Unsigned(_, width) => hir::Type::Int {
                signed: false,
                width: *width,
            },
            mir::Constant::Float(_, width) => hir::Type::Float { width: *width },
            mir::Constant::Bool(_) => hir::Type::Bool,
            mir::Constant::Char(_) => hir::Type::Char,
            mir::Constant::String(_) => hir::Type::String,
            mir::Constant::Unit => hir::Type::Unit,
        }),
    }
}
fn match_type(
    pattern: &hir::Type,
    concrete: &hir::Type,
    inferred: &mut HashMap<String, hir::Type>,
) -> bool {
    match (pattern, concrete) {
        (hir::Type::Generic(name), concrete) => {
            inferred
                .get(name)
                .is_none_or(|previous| previous == concrete)
                && {
                    inferred.insert(name.clone(), concrete.clone());
                    true
                }
        }
        (hir::Type::Struct(a, xs), hir::Type::Struct(b, ys)) => {
            a.0 == b.0
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(x, y)| match_type(x, y, inferred))
        }
        (hir::Type::Enum(a, xs), hir::Type::Enum(b, ys)) => {
            a.0 == b.0
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(x, y)| match_type(x, y, inferred))
        }
        (hir::Type::Option(x), hir::Type::Option(y)) => match_type(x, y, inferred),
        (
            hir::Type::Reference {
                mutable: a,
                inner: x,
            },
            hir::Type::Reference {
                mutable: b,
                inner: y,
            },
        ) if !*a || *b => match_type(x, y, inferred),
        (
            hir::Type::RawPointer {
                mutable: a,
                inner: x,
            },
            hir::Type::RawPointer {
                mutable: b,
                inner: y,
            },
        ) if a == b => match_type(x, y, inferred),
        (hir::Type::Array(x, a), hir::Type::Array(y, b)) if a == b => match_type(x, y, inferred),
        (hir::Type::Slice(x), hir::Type::Slice(y)) => match_type(x, y, inferred),
        (hir::Type::List(x), hir::Type::List(y)) => match_type(x, y, inferred),
        (hir::Type::Map(ak, av), hir::Type::Map(bk, bv)) => {
            match_type(ak, bk, inferred) && match_type(av, bv, inferred)
        }
        (hir::Type::Set(x), hir::Type::Set(y)) => match_type(x, y, inferred),
        (hir::Type::Thread(x), hir::Type::Thread(y)) => match_type(x, y, inferred),
        (hir::Type::Future(x), hir::Type::Future(y)) => match_type(x, y, inferred),
        (hir::Type::Task(x), hir::Type::Task(y)) => match_type(x, y, inferred),
        (hir::Type::Mutex(x), hir::Type::Mutex(y))
        | (hir::Type::MutexGuard(x), hir::Type::MutexGuard(y)) => match_type(x, y, inferred),
        (hir::Type::Result(a, b), hir::Type::Result(x, y)) => {
            match_type(a, x, inferred) && match_type(b, y, inferred)
        }
        _ => pattern == concrete,
    }
}
fn error(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}

pub fn mangle(program: &mir::Program, instance: &FunctionInstance) -> String {
    let function = &program.functions[instance.function.0];
    if let Some(external) = &function.external {
        return external.link_name.clone();
    }
    let name = &function.name;
    let mut symbol = format!("disp_f{}_{}", instance.function.0, sanitize(name));
    for ty in &instance.substitutions {
        symbol.push('_');
        symbol.push_str(&type_code(ty));
    }
    symbol
}
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}
pub fn type_code(ty: &hir::Type) -> String {
    match ty {
        hir::Type::Unit => "v".into(),
        hir::Type::Bool => "b".into(),
        hir::Type::Char => "c".into(),
        hir::Type::String => "s".into(),
        hir::Type::CString => "cs".into(),
        hir::Type::CStr => "cz".into(),
        hir::Type::Memory => "mem".into(),
        hir::Type::Path => "p".into(),
        hir::Type::ProcessOutput => "po".into(),
        hir::Type::Url => "url".into(),
        hir::Type::Json => "json".into(),
        hir::Type::IpAddress => "ni".into(),
        hir::Type::SocketAddress => "na".into(),
        hir::Type::TcpStream => "nt".into(),
        hir::Type::TlsStream => "nx".into(),
        hir::Type::HttpRequest => "nq".into(),
        hir::Type::HttpResponse => "nh".into(),
        hir::Type::TcpListener => "nl".into(),
        hir::Type::UdpSocket => "nu".into(),
        hir::Type::UdpDatagram => "nd".into(),
        hir::Type::Instant => "ti".into(),
        hir::Type::Duration => "td".into(),
        hir::Type::Str => "z".into(),
        hir::Type::Array(element, length) => format!("A{length}_{}", type_code(element)),
        hir::Type::Slice(element) => format!("L{}", type_code(element)),
        hir::Type::List(element) => format!("V{}", type_code(element)),
        hir::Type::Map(key, value) => format!("M{}_{}", type_code(key), type_code(value)),
        hir::Type::Set(element) => format!("Q{}", type_code(element)),
        hir::Type::Thread(result) => format!("T{}", type_code(result)),
        hir::Type::Future(result) => format!("U{}", type_code(result)),
        hir::Type::Task(result) => format!("K{}", type_code(result)),
        hir::Type::Mutex(value) => format!("X{}", type_code(value)),
        hir::Type::MutexGuard(value) => format!("Y{}", type_code(value)),
        hir::Type::AtomicInt => "Z".into(),
        hir::Type::Int { signed, width } => format!(
            "{}{}",
            if *signed { 'i' } else { 'u' },
            width.map_or("n".into(), |width| width.to_string())
        ),
        hir::Type::Float { width } => format!("f{width}"),
        hir::Type::Reference { mutable, inner } => {
            format!("r{}{}", if *mutable { 'm' } else { 's' }, type_code(inner))
        }
        hir::Type::RawPointer { mutable, inner } => {
            format!("p{}{}", if *mutable { 'm' } else { 'c' }, type_code(inner))
        }
        hir::Type::Struct(id, args) => format!(
            "S{}{}",
            id.0,
            args.iter().map(type_code).collect::<Vec<_>>().join("_")
        ),
        hir::Type::Enum(id, args) => format!(
            "E{}{}",
            id.0,
            args.iter().map(type_code).collect::<Vec<_>>().join("_")
        ),
        hir::Type::Option(x) => format!("O{}", type_code(x)),
        hir::Type::Result(a, b) => format!("R{}_{}", type_code(a), type_code(b)),
        hir::Type::Generic(name) => format!("G{}", sanitize(name)),
        hir::Type::Function(_, _) => "fn".into(),
        hir::Type::Unknown => "unknown".into(),
    }
}
