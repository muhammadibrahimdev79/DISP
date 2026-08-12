//! Direct, concrete lowering for the scalar/string portion of MIR.
//!
//! Unsupported aggregate/reference functions deliberately fall back to the
//! general backend while their concrete lowering is implemented. This module
//! never changes program semantics merely to make a function eligible.

use super::{
    abi::AbiProgram, allocator::C_ALLOCATOR, layout::substitute, mono, native_types,
    runtime::C_RUNTIME,
};
use crate::{ast, diagnostics::Diagnostic, hir, mir};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Write,
};

pub fn generate(
    program: &mir::Program,
    instances: &mono::MonoProgram,
    abi: &AbiProgram,
    declarations: &str,
) -> Result<Option<String>, Diagnostic> {
    if !instances.instances.iter().all(|instance| {
        let function = &program.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        function
            .locals
            .iter()
            .all(|local| supported(program, &substitute(&local.ty, &substitutions)))
    }) {
        return Ok(None);
    }
    let mut output = String::from(C_ALLOCATOR);
    output.push_str(declarations);
    emit_source_map(program, &mut output);
    output.push_str(C_RUNTIME);
    for instance in &instances.instances {
        writeln!(
            output,
            "/* DISP ABI {:?} */",
            abi.functions
                .get(instance)
                .expect("ABI must cover every function instance")
        )
        .unwrap();
        prototype(program, instance, &mut output);
    }
    for target in callable_targets(program, instances)? {
        callable_wrapper(program, &target, &mut output);
    }
    for target in thread_targets(program, instances)? {
        thread_wrapper(program, &target, &mut output);
    }
    for instance in &instances.instances {
        function(program, instance, &mut output)?;
    }
    let entry_function = &program.functions[instances.entry.function.0];
    if entry_function.asynchronous {
        let substitutions = mono::mapping(entry_function, &instances.entry);
        let result = c_local_type(entry_function, entry_function.return_local, &substitutions);
        writeln!(
            output,
            "int main(void){{disp_native_future future={}();{result} result=({result}){{0}};disp_future_wait(&future,&result,0,0);(void)result;return 0;}}",
            mono::mangle(program, &instances.entry)
        )
        .unwrap();
    } else {
        writeln!(
            output,
            "int main(void){{{} result={}();(void)result;return 0;}}",
            native_types::c_type(&hir::Type::Unit),
            mono::mangle(program, &instances.entry)
        )
        .unwrap();
    }
    Ok(Some(output))
}

fn callable_targets(
    program: &mir::Program,
    instances: &mono::MonoProgram,
) -> Result<BTreeSet<mono::FunctionInstance>, Diagnostic> {
    let mut targets = BTreeSet::new();
    for caller in &instances.instances {
        let function = &program.functions[caller.function.0];
        for block in &function.blocks {
            for statement in &block.statements {
                if let mir::StatementKind::Assign(_, mir::Rvalue::Function(target)) =
                    &statement.kind
                {
                    let target_function = program.functions.get(target.0).ok_or_else(|| {
                        Diagnostic::new(
                            crate::diagnostics::DiagnosticKind::Internal,
                            "native callable references an invalid function",
                            statement.span,
                        )
                    })?;
                    targets.insert(mono::FunctionInstance {
                        function: target_function.id,
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
                    targets.insert(mono::FunctionInstance {
                        function: *target,
                        substitutions: caller.substitutions.clone(),
                    });
                }
            }
        }
    }
    Ok(targets)
}

fn callable_wrapper_name(program: &mir::Program, target: &mono::FunctionInstance) -> String {
    format!("disp_callable_wrap_{}", mono::mangle(program, target))
}

fn callable_env_name(program: &mir::Program, target: &mono::FunctionInstance) -> String {
    format!("disp_callable_env_{}", mono::mangle(program, target))
}

fn callable_drop_name(program: &mir::Program, target: &mono::FunctionInstance) -> String {
    format!("disp_callable_drop_{}", mono::mangle(program, target))
}

fn callable_wrapper(program: &mir::Program, target: &mono::FunctionInstance, output: &mut String) {
    let function = &program.functions[target.function.0];
    let substitutions = mono::mapping(function, target);
    let declared_result = substitute(&function.locals[function.return_local.0].ty, &substitutions);
    let result = if function.asynchronous {
        hir::Type::Future(Box::new(declared_result))
    } else {
        declared_result
    };
    if function.capture_count > 0 {
        let environment = callable_env_name(program, target);
        writeln!(output, "typedef struct {environment} {{").unwrap();
        for index in 0..function.capture_count {
            writeln!(
                output,
                "{} f{index};",
                c_local_type(function, mir::LocalId(index + 1), &substitutions)
            )
            .unwrap();
        }
        writeln!(output, "}} {environment};").unwrap();
        writeln!(
            output,
            "static void {}(void *_raw){{{environment} *_captures=({environment}*)_raw;",
            callable_drop_name(program, target)
        )
        .unwrap();
        for index in 0..function.capture_count {
            let ty = substitute(&function.locals[index + 1].ty, &substitutions);
            output.push_str(&drop_value(program, &format!("_captures->f{index}"), &ty));
        }
        output.push_str("disp_dealloc(_captures);}\n");
    }
    write!(
        output,
        "static {} {}(void *_env",
        native_types::c_type(&result),
        callable_wrapper_name(program, target)
    )
    .unwrap();
    for index in function.capture_count..function.argument_count {
        let local = mir::LocalId(index + 1);
        write!(
            output,
            ",{} a{}",
            c_local_type(function, local, &substitutions),
            index - function.capture_count + 1
        )
        .unwrap();
    }
    let external_unit = function.external.is_some() && matches!(result, hir::Type::Unit);
    output.push_str("){(void)_env;");
    if function.capture_count > 0 {
        write!(
            output,
            "{} *_captures=({}*)_env;",
            callable_env_name(program, target),
            callable_env_name(program, target)
        )
        .unwrap();
    }
    write!(
        output,
        "{}{}(",
        if external_unit { "" } else { "return " },
        mono::mangle(program, target)
    )
    .unwrap();
    for index in 0..function.argument_count {
        if index > 0 {
            output.push(',');
        }
        if index < function.capture_count {
            write!(output, "_captures->f{index}").unwrap();
        } else {
            write!(output, "a{}", index - function.capture_count + 1).unwrap();
        }
    }
    if external_unit {
        output.push_str(");return (disp_native_unit){0};}\n");
    } else {
        output.push_str(");}\n");
    }
}

fn emit_source_map(program: &mir::Program, output: &mut String) {
    output.push_str("static const char *disp_source_location(int *line){");
    for source in &program.source_files {
        let path = source.path.to_string_lossy().replace('\\', "/");
        let path = path
            .chars()
            .flat_map(|character| match character {
                '\\' => "\\\\".chars().collect::<Vec<_>>(),
                '"' => "\\\"".chars().collect(),
                character if character.is_control() => "?".chars().collect(),
                character => vec![character],
            })
            .collect::<String>();
        writeln!(
            output,
            "if(*line>={}&&*line<={}){{*line-={};return \"{}\";}}",
            source.start_line,
            source.end_line,
            source.start_line - 1,
            path
        )
        .unwrap();
    }
    output.push_str("return NULL;}\n");
}

fn thread_targets(
    program: &mir::Program,
    instances: &mono::MonoProgram,
) -> Result<BTreeSet<mono::FunctionInstance>, Diagnostic> {
    let mut targets = BTreeSet::new();
    for caller in &instances.instances {
        let function = &program.functions[caller.function.0];
        for block in &function.blocks {
            if let mir::Terminator::Spawn {
                target,
                arguments,
                substitutions,
                ..
            } = &block.terminator
            {
                let target = mono::resolve_target(
                    program,
                    function,
                    caller,
                    &hir::CallTarget::Function(*target),
                    substitutions,
                    arguments,
                )?
                .expect("a validated spawn target must resolve");
                targets.insert(target);
            }
        }
    }
    Ok(targets)
}

fn thread_context_name(program: &mir::Program, target: &mono::FunctionInstance) -> String {
    format!("disp_thread_ctx_{}", mono::mangle(program, target))
}

fn thread_entry_name(program: &mir::Program, target: &mono::FunctionInstance) -> String {
    format!("disp_thread_entry_{}", mono::mangle(program, target))
}

fn thread_wrapper(program: &mir::Program, target: &mono::FunctionInstance, output: &mut String) {
    let function = &program.functions[target.function.0];
    let substitutions = mono::mapping(function, target);
    let context = thread_context_name(program, target);
    let entry = thread_entry_name(program, target);
    let result_ty = substitute(&function.locals[function.return_local.0].ty, &substitutions);
    writeln!(output, "typedef struct {context} {{").unwrap();
    for index in 0..function.argument_count {
        let local = mir::LocalId(index + 1);
        writeln!(
            output,
            "{} a{};",
            c_local_type(function, local, &substitutions),
            index + 1
        )
        .unwrap();
    }
    writeln!(output, "{} *result;", native_types::c_type(&result_ty)).unwrap();
    writeln!(output, "}} {context};").unwrap();
    writeln!(
        output,
        "static void {entry}(void *raw){{{context} *context=({context}*)raw;"
    )
    .unwrap();
    write!(
        output,
        "*(context->result)={}(",
        mono::mangle(program, target)
    )
    .unwrap();
    for index in 0..function.argument_count {
        if index > 0 {
            output.push(',');
        }
        write!(output, "context->a{}", index + 1).unwrap();
    }
    writeln!(output, ");disp_dealloc(context);}}").unwrap();
}

pub fn supported(program: &mir::Program, ty: &hir::Type) -> bool {
    match ty {
        hir::Type::Unit
        | hir::Type::Bool
        | hir::Type::Char
        | hir::Type::String
        | hir::Type::Str
        | hir::Type::CString
        | hir::Type::CStr
        | hir::Type::Memory
        | hir::Type::Path
        | hir::Type::Instant
        | hir::Type::Duration
        | hir::Type::Int { .. }
        | hir::Type::Float { .. } => true,
        hir::Type::AtomicInt => true,
        hir::Type::Thread(result) => supported(program, result),
        hir::Type::Future(result) => supported(program, result),
        hir::Type::Mutex(value) | hir::Type::MutexGuard(value) => supported(program, value),
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            declaration
                .fields
                .iter()
                .all(|field| supported(program, &substitute(&field.ty, &substitutions)))
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            declaration.variants.iter().all(|variant| {
                variant
                    .payload
                    .iter()
                    .all(|payload| supported(program, &substitute(payload, &substitutions)))
            })
        }
        hir::Type::Option(inner) => supported(program, inner),
        hir::Type::Array(element, _) => supported(program, element),
        hir::Type::Slice(element) => supported(program, element),
        hir::Type::List(element) | hir::Type::Set(element) => supported(program, element),
        hir::Type::Map(key, value) => supported(program, key) && supported(program, value),
        hir::Type::Result(ok, error) => supported(program, ok) && supported(program, error),
        hir::Type::Reference { inner, .. } | hir::Type::RawPointer { inner, .. } => {
            supported(program, inner)
        }
        hir::Type::Function(arguments, result) => {
            arguments
                .iter()
                .all(|argument| supported(program, argument))
                && supported(program, result)
        }
        hir::Type::Generic(name) => matches!(name.as_str(), "ConversionError" | "IoError"),
        _ => false,
    }
}

fn prototype(program: &mir::Program, instance: &mono::FunctionInstance, output: &mut String) {
    let function = &program.functions[instance.function.0];
    let substitutions = mono::mapping(function, instance);
    let result = if function.asynchronous {
        "disp_native_future".into()
    } else if function.external.is_some()
        && matches!(function.locals[function.return_local.0].ty, hir::Type::Unit)
    {
        "void".into()
    } else {
        c_local_type(function, function.return_local, &substitutions)
    };
    write!(
        output,
        "{}{} {}(",
        if function.external.is_some() {
            "extern "
        } else {
            "static "
        },
        result,
        mono::mangle(program, instance)
    )
    .unwrap();
    if function.argument_count == 0 && function.external.is_some() {
        output.push_str("void");
    }
    for index in 0..function.argument_count {
        if index > 0 {
            output.push(',');
        }
        let local = mir::LocalId(index + 1);
        write!(
            output,
            "{} a{}",
            c_local_type(function, local, &substitutions),
            index + 1
        )
        .unwrap();
    }
    output.push_str(");\n");
}

fn function(
    program: &mir::Program,
    instance: &mono::FunctionInstance,
    output: &mut String,
) -> Result<(), Diagnostic> {
    let function = &program.functions[instance.function.0];
    if function.external.is_some() {
        return Ok(());
    }
    let substitutions = mono::mapping(function, instance);
    if function.asynchronous {
        return async_function(program, function, instance, &substitutions, output);
    }
    let symbol = mono::mangle(program, instance);
    write!(
        output,
        "static {} {}(",
        c_local_type(function, function.return_local, &substitutions),
        symbol
    )
    .unwrap();
    for index in 0..function.argument_count {
        if index > 0 {
            output.push(',');
        }
        let local = mir::LocalId(index + 1);
        write!(
            output,
            "{} a{}",
            c_local_type(function, local, &substitutions),
            index + 1
        )
        .unwrap();
    }
    output.push_str("){\n");
    for local in &function.locals {
        writeln!(
            output,
            "{} l{}=({}){{0}};",
            native_types::c_type(&substitute(&local.ty, &substitutions)),
            local.id.0,
            native_types::c_type(&substitute(&local.ty, &substitutions))
        )
        .unwrap();
    }
    for index in 0..function.argument_count {
        writeln!(output, "l{}=a{};", index + 1, index + 1).unwrap();
    }
    output.push_str("goto bb0;\n");
    for (index, block) in function.blocks.iter().enumerate() {
        writeln!(output, "bb{index}:;").unwrap();
        for statement in &block.statements {
            emit_statement(
                program,
                function,
                instance,
                statement,
                &substitutions,
                output,
            )?;
        }
        terminator(
            program,
            function,
            instance,
            &block.terminator,
            &substitutions,
            (false, index),
            output,
        )?;
    }
    output.push_str("}\n");
    Ok(())
}

fn async_function(
    program: &mir::Program,
    function: &mir::Function,
    instance: &mono::FunctionInstance,
    substitutions: &HashMap<String, hir::Type>,
    output: &mut String,
) -> Result<(), Diagnostic> {
    let symbol = mono::mangle(program, instance);
    let context = format!("{symbol}_future_context");
    let result = c_local_type(function, function.return_local, substitutions);
    writeln!(output, "typedef struct {context} {{bool started;size_t pc;").unwrap();
    for local in &function.locals {
        writeln!(
            output,
            "{} l{};",
            native_types::c_type(&substitute(&local.ty, substitutions)),
            local.id.0
        )
        .unwrap();
    }
    writeln!(output, "}} {context};").unwrap();
    for local in &function.locals {
        writeln!(output, "#define l{} (context->l{})", local.id.0, local.id.0).unwrap();
    }
    writeln!(
        output,
        "static bool {symbol}_future_poll(void *raw,void *_output){{{context} *context=({context}*)raw;if(!context->started){{context->started=true;goto bb0;}}switch(context->pc){{"
    )
    .unwrap();
    for index in 0..function.blocks.len() {
        writeln!(output, "case {index}:goto bb{index};").unwrap();
    }
    output.push_str("default:dv_panic(\"invalid async resume state\",0,0);return false;}\n");
    for (index, block) in function.blocks.iter().enumerate() {
        writeln!(output, "bb{index}:;").unwrap();
        for statement in &block.statements {
            emit_statement(
                program,
                function,
                instance,
                statement,
                substitutions,
                output,
            )?;
        }
        terminator(
            program,
            function,
            instance,
            &block.terminator,
            substitutions,
            (true, index),
            output,
        )?;
    }
    output.push_str("}\n");

    writeln!(
        output,
        "static void {symbol}_future_drop(void *raw){{{context} *context=({context}*)raw;"
    )
    .unwrap();
    let mut emitted = HashSet::new();
    for block in &function.blocks {
        for statement in &block.statements {
            if let mir::StatementKind::Drop {
                place,
                flag: Some(flag),
            } = &statement.kind
                && emitted.insert((place.clone(), *flag))
            {
                let ty = place_ty(program, function, place, substitutions);
                let action = drop_value(
                    program,
                    &place_expr(program, function, place, substitutions),
                    &ty,
                );
                if !action.is_empty() {
                    writeln!(output, "if(l{}){{{action}}}", flag.0).unwrap();
                }
            }
        }
    }
    output.push_str("disp_dealloc(context);}\n");
    for local in &function.locals {
        writeln!(output, "#undef l{}", local.id.0).unwrap();
    }

    write!(output, "static disp_native_future {symbol}(").unwrap();
    for index in 0..function.argument_count {
        if index > 0 {
            output.push(',');
        }
        write!(
            output,
            "{} a{}",
            c_local_type(function, mir::LocalId(index + 1), substitutions),
            index + 1
        )
        .unwrap();
    }
    writeln!(
        output,
        "){{{context} *context=({context}*)disp_alloc_zeroed(1,sizeof({context}),_Alignof({context}));"
    )
    .unwrap();
    for index in 0..function.argument_count {
        writeln!(output, "context->l{}=a{};", index + 1, index + 1).unwrap();
    }
    let mut argument_flags = HashSet::new();
    for block in &function.blocks {
        for statement in &block.statements {
            if let mir::StatementKind::Drop {
                place,
                flag: Some(flag),
            } = &statement.kind
                && place.projections.is_empty()
                && place.local.0 > 0
                && place.local.0 <= function.argument_count
                && argument_flags.insert(*flag)
            {
                writeln!(output, "context->l{}=true;", flag.0).unwrap();
            }
        }
    }
    writeln!(
        output,
        "return (disp_native_future){{.context=context,.poll={symbol}_future_poll,.drop={symbol}_future_drop}};}}"
    )
    .unwrap();
    let _ = result;
    Ok(())
}

fn emit_statement(
    program: &mir::Program,
    function: &mir::Function,
    instance: &mono::FunctionInstance,
    statement: &mir::Statement,
    substitutions: &HashMap<String, hir::Type>,
    output: &mut String,
) -> Result<(), Diagnostic> {
    match &statement.kind {
        mir::StatementKind::StorageLive(local) => writeln!(
            output,
            "l{}=({}){{0}};",
            local.0,
            c_local_type(function, *local, substitutions)
        )
        .unwrap(),
        mir::StatementKind::StorageDead(_) => output.push_str(";\n"),
        mir::StatementKind::Drop { place, flag } => {
            let ty = place_ty(program, function, place, substitutions);
            let action = drop_value(
                program,
                &place_expr(program, function, place, substitutions),
                &ty,
            );
            if !action.is_empty() {
                if let Some(flag) = flag {
                    writeln!(output, "if(l{}){{{action}}}", flag.0,).unwrap();
                } else {
                    writeln!(output, "{action}").unwrap();
                }
            } else {
                output.push_str(";\n");
            }
        }
        mir::StatementKind::SetDropFlag { local, initialized } => writeln!(
            output,
            "l{}={};",
            local.0,
            if *initialized { "true" } else { "false" }
        )
        .unwrap(),
        mir::StatementKind::Assign(place, value) => {
            let ty = place_ty(program, function, place, substitutions);
            let expression = rvalue(
                program,
                function,
                instance,
                value,
                &ty,
                statement.span,
                substitutions,
            );
            writeln!(
                output,
                "{}={expression};",
                place_expr(program, function, place, substitutions)
            )
            .unwrap();
        }
        mir::StatementKind::Nop => output.push_str(";\n"),
    }
    Ok(())
}

fn terminator(
    program: &mir::Program,
    function: &mir::Function,
    instance: &mono::FunctionInstance,
    terminator: &mir::Terminator,
    substitutions: &HashMap<String, hir::Type>,
    emission: (bool, usize),
    output: &mut String,
) -> Result<(), Diagnostic> {
    let (async_poll, block_index) = emission;
    match terminator {
        mir::Terminator::Goto(block) => writeln!(output, "goto bb{};", block.0).unwrap(),
        mir::Terminator::SwitchBool {
            condition,
            true_block,
            false_block,
        } => writeln!(
            output,
            "if({})goto bb{};else goto bb{};",
            operand(
                program,
                function,
                condition,
                &hir::Type::Bool,
                substitutions
            ),
            true_block.0,
            false_block.0
        )
        .unwrap(),
        mir::Terminator::SwitchValue {
            discriminant,
            targets,
            otherwise,
        } => {
            let ty = operand_ty(program, function, discriminant, substitutions);
            for (constant, block) in targets {
                writeln!(
                    output,
                    "if(dv_equal({},{}))goto bb{};",
                    box_value(
                        program,
                        &operand(program, function, discriminant, &ty, substitutions),
                        &ty,
                    ),
                    to_dv(&constant_expr(constant, &ty), &ty),
                    block.0
                )
                .unwrap();
            }
            writeln!(output, "goto bb{};", otherwise.0).unwrap();
        }
        mir::Terminator::SwitchEnum {
            discriminant,
            targets,
            otherwise,
        } => {
            let ty = operand_ty(program, function, discriminant, substitutions);
            let value = operand(program, function, discriminant, &ty, substitutions);
            for (variant, block) in targets {
                writeln!(
                    output,
                    "if(({}).tag=={})goto bb{};",
                    value,
                    variant_index(program, &ty, *variant),
                    block.0
                )
                .unwrap();
            }
            writeln!(output, "goto bb{};", otherwise.0).unwrap();
        }
        mir::Terminator::Call {
            target,
            arguments,
            destination,
            next,
            substitutions: call_substitutions,
            span,
            ..
        } => {
            let destination_ty = place_ty(program, function, destination, substitutions);
            let call = match target {
                hir::CallTarget::Intrinsic(name) if name == "Async.yield" => {
                    "disp_future_yield()".into()
                }
                hir::CallTarget::Callable => {
                    let callable_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let hir::Type::Function(parameters, result) = &callable_ty else {
                        unreachable!("validated callable target must have function type")
                    };
                    let callable = operand(
                        program,
                        function,
                        &arguments[0],
                        &callable_ty,
                        substitutions,
                    );
                    let mut signature = format!("{} (*)(void *", native_types::c_type(result));
                    for parameter in parameters {
                        write!(signature, ",{}", native_types::c_type(parameter)).unwrap();
                    }
                    signature.push(')');
                    let values = arguments[1..]
                        .iter()
                        .zip(parameters)
                        .map(|(argument, expected)| {
                            operand(program, function, argument, expected, substitutions)
                        })
                        .collect::<Vec<_>>();
                    let suffix = if values.is_empty() {
                        String::new()
                    } else {
                        format!(",{}", values.join(","))
                    };
                    format!("((({signature})(({callable}).code))(({callable}).env{suffix}))")
                }
                hir::CallTarget::Intrinsic(name) if name == "print" => {
                    let argument_ty = operand_ty(program, function, &arguments[0], substitutions);
                    format!(
                        "(dv_print({}),({}){{0}})",
                        box_value(
                            program,
                            &operand(
                                program,
                                function,
                                &arguments[0],
                                &argument_ty,
                                substitutions,
                            ),
                            &argument_ty,
                        ),
                        native_types::c_type(&hir::Type::Unit)
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "array_get" => {
                    let array_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let hir::Type::Array(_, length) = array_ty else {
                        unreachable!()
                    };
                    let array = operand(program, function, &arguments[0], &array_ty, substitutions);
                    let index_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let index = operand(program, function, &arguments[1], &index_ty, substitutions);
                    format!(
                        "((uint64_t)({index})>={length}?(dv_panic(\"array index out of bounds\",{},{}),({}){{0}}):({array}).values[(uint64_t)({index})])",
                        span.start.line,
                        span.start.column,
                        native_types::c_type(&destination_ty)
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "array_len" => {
                    let array_ty = operand_ty(program, function, &arguments[0], substitutions);
                    match array_ty {
                        hir::Type::Array(_, length) => length.to_string(),
                        hir::Type::Slice(_) => {
                            let value =
                                operand(program, function, &arguments[0], &array_ty, substitutions);
                            format!("({value}).len")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "slice_is_empty" => {
                    let ty = operand_ty(program, function, &arguments[0], substitutions);
                    let value = operand(program, function, &arguments[0], &ty, substitutions);
                    match ty {
                        hir::Type::Array(_, length) => (length == 0).to_string(),
                        hir::Type::Slice(_) => format!("({value}).len==0"),
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "Thread.join" => {
                    let thread_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let value =
                        operand(program, function, &arguments[0], &thread_ty, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{disp_native_thread _thread={value};disp_thread_wait(&_thread);{result_c} _result=*({result_c}*)_thread.result;disp_dealloc(_thread.result);_result;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Mutex.new" => {
                    let hir::Type::Mutex(value_ty) = &destination_ty else {
                        unreachable!()
                    };
                    let value = operand(program, function, &arguments[0], value_ty, substitutions);
                    let value_c = native_types::c_type(value_ty);
                    format!(
                        "({{{value_c} *_data=({value_c}*)disp_alloc(sizeof({value_c}),_Alignof({value_c}));*_data={value};(disp_native_mutex){{.state=disp_mutex_create(_data)}};}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "Mutex.share" | "Mutex.lock") =>
                {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    if name == "Mutex.share" {
                        format!(
                            "({{disp_mutex_retain(({receiver})->state);(disp_native_mutex){{.state=({receiver})->state}};}})"
                        )
                    } else {
                        format!(
                            "disp_mutex_lock(({receiver})->state,{},{})",
                            span.start.line, span.start.column
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "AtomicInt.new" => {
                    let value = operand(
                        program,
                        function,
                        &arguments[0],
                        &hir::Type::Int {
                            signed: true,
                            width: None,
                        },
                        substitutions,
                    );
                    format!(
                        "(disp_native_atomic_int){{.state=disp_atomic_int_create((int64_t)({value}))}}"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("AtomicInt.") => {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    match name.as_str() {
                        "AtomicInt.share" => format!(
                            "({{disp_atomic_int_retain(({receiver})->state);(disp_native_atomic_int){{.state=({receiver})->state}};}})"
                        ),
                        "AtomicInt.load" => format!("disp_atomic_int_load(({receiver})->state)"),
                        "AtomicInt.store" => {
                            let value = operand(
                                program,
                                function,
                                &arguments[1],
                                &hir::Type::Int {
                                    signed: true,
                                    width: None,
                                },
                                substitutions,
                            );
                            format!(
                                "(disp_atomic_int_store(({receiver})->state,(int64_t)({value})),(disp_native_unit){{0}})"
                            )
                        }
                        "AtomicInt.fetch_add" | "AtomicInt.add" => {
                            let value = operand(
                                program,
                                function,
                                &arguments[1],
                                &hir::Type::Int {
                                    signed: true,
                                    width: None,
                                },
                                substitutions,
                            );
                            let fetch = format!(
                                "disp_atomic_int_fetch_add(({receiver})->state,(int64_t)({value}),{},{})",
                                span.start.line, span.start.column
                            );
                            if name == "AtomicInt.add" {
                                format!("({fetch}+(int64_t)({value}))")
                            } else {
                                fetch
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "String.new" => {
                    "disp_string_with_capacity(0)".into()
                }
                hir::CallTarget::Intrinsic(name) if name == "String.with_capacity" => {
                    let ty = operand_ty(program, function, &arguments[0], substitutions);
                    let value = operand(program, function, &arguments[0], &ty, substitutions);
                    format!("disp_string_with_capacity((size_t)({value}))")
                }
                hir::CallTarget::Intrinsic(name) if name == "CString.new" => {
                    let source_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let source =
                        operand(program, function, &arguments[0], &source_ty, substitutions);
                    let hir::Type::Reference { inner, .. } = source_ty else {
                        unreachable!("CString.new source must be borrowed")
                    };
                    let (data, len) = match *inner {
                        hir::Type::String | hir::Type::Str => {
                            (format!("({source})->data"), format!("({source})->len"))
                        }
                        hir::Type::CStr => (format!("*({source})"), format!("strlen(*({source}))")),
                        _ => unreachable!("type checking validates CString source"),
                    };
                    let result_c = native_types::c_type(&destination_ty);
                    let message = "CString source contains an interior NUL byte";
                    format!(
                        "({{const char *_data={data};size_t _len={len};{result_c} _result={{0}};if(_len&&memchr(_data,0,_len)){{_result.tag=1;_result.payload.v1.f0=disp_owned_bytes(\"{message}\",{});}}else{{_result.tag=0;_result.payload.v0.f0=disp_cstring_from_bytes(_data,_len);}}_result;}})",
                        message.len()
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Memory.allocate" => {
                    let size_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let align_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let size = operand(program, function, &arguments[0], &size_ty, substitutions);
                    let align = operand(program, function, &arguments[1], &align_ty, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{size_t _size=(size_t)({size});size_t _align=(size_t)({align});{result_c} _result={{0}};const char *_error=NULL;if(!_align||(_align&(_align-1)))_error=\"Memory alignment must be a non-zero power of two\";else if(_align>((size_t)1<<20))_error=\"Memory alignment exceeds the supported maximum\";else if(_size>SIZE_MAX-sizeof(DispAllocationHeader)-(_align-1))_error=\"Memory size overflow\";if(_error){{_result.tag=1;_result.payload.v1.f0=disp_owned_bytes(_error,strlen(_error));}}else{{_result.tag=0;_result.payload.v0.f0=(disp_native_memory){{.data=_size?(uint8_t*)disp_alloc_zeroed(1,_size,_align):NULL,.len=_size,.align=_align}};}}_result;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Path.new" => {
                    let (source, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    format!(
                        "disp_path_from_bytes(({source})->data,({source})->len,{},{})",
                        span.start.line, span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("Path.") => {
                    let (path, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "Path.join" => {
                            let (child, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            format!(
                                "disp_path_join({path},({child})->data,({child})->len,{},{})",
                                span.start.line, span.start.column
                            )
                        }
                        "Path.len" => format!("({path})->len"),
                        "Path.is_empty" => format!("({path})->len==0"),
                        "Path.is_absolute" => format!(
                            "((({path})->len>0&&((({path})->data[0]=='/'||({path})->data[0]=='\\\\'))||(({path})->len>2&&({path})->data[1]==':')))"
                        ),
                        "Path.as_string" => {
                            format!("disp_owned_bytes(({path})->data,({path})->len)")
                        }
                        "Path.name" => {
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{option_c} _r={{0}};size_t _end=({path})->len;while(_end>1&&((({path})->data[_end-1]=='/')||(({path})->data[_end-1]=='\\\\')))_end--;size_t _start=_end;while(_start&&({path})->data[_start-1]!='/'&&({path})->data[_start-1]!='\\\\')_start--;if(_end>_start){{_r.tag=1;_r.payload.v1.f0=disp_owned_bytes(({path})->data+_start,_end-_start);}}_r;}})"
                            )
                        }
                        "Path.extension" => {
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{option_c} _r={{0}};size_t _end=({path})->len;while(_end>1&&((({path})->data[_end-1]=='/')||(({path})->data[_end-1]=='\\\\')))_end--;size_t _start=_end;while(_start&&({path})->data[_start-1]!='/'&&({path})->data[_start-1]!='\\\\')_start--;size_t _dot=_end;while(_dot>_start&&({path})->data[_dot-1]!='.')_dot--;if(_dot>_start+1&&_dot<_end){{_r.tag=1;_r.payload.v1.f0=disp_owned_bytes(({path})->data+_dot,_end-_dot);}}_r;}})"
                            )
                        }
                        "Path.parent" => {
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{option_c} _r={{0}};size_t _end=({path})->len;while(_end>1&&((({path})->data[_end-1]=='/')||(({path})->data[_end-1]=='\\\\')))_end--;size_t _split=_end;while(_split&&({path})->data[_split-1]!='/'&&({path})->data[_split-1]!='\\\\')_split--;if(({path})->len){{size_t _parent=_split?(_split==1?1:_split-1):0;_r.tag=1;_r.payload.v1.f0=disp_path_from_bytes(({path})->data,_parent,0,0);}}_r;}})"
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("File.") => {
                    let (path, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    if name == "File.exists" {
                        format!("disp_file_exists({path})")
                    } else {
                        let result_c = native_types::c_type(&destination_ty);
                        match name.as_str() {
                            "File.read_text" => format!(
                                "({{{result_c} _r={{0}};disp_native_string _value={{0}},_error={{0}};if(disp_file_read_text({path},&_value,&_error)){{_r.tag=0;_r.payload.v0.f0=_value;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            ),
                            "File.read_bytes" => {
                                let hir::Type::Result(ok, _) = &destination_ty else {
                                    unreachable!()
                                };
                                let list_c = native_types::c_type(ok);
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_string _bytes={{0}},_error={{0}};if(disp_file_read_text({path},&_bytes,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                            "File.size" | "File.modified_seconds" => {
                                let field = if name == "File.size" {
                                    "_size"
                                } else {
                                    "_modified"
                                };
                                format!(
                                    "({{{result_c} _r={{0}};uint64_t _size=0,_modified=0;disp_native_string _error={{0}};if(disp_file_metadata({path},&_size,&_modified,&_error)){{_r.tag=0;_r.payload.v0.f0={field};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                            "File.write_text" | "File.append_text" => {
                                let (text, _) = system_argument(
                                    program,
                                    function,
                                    &arguments[1],
                                    substitutions,
                                );
                                let append = name == "File.append_text";
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_file_write_text({path},({text})->data,({text})->len,{append},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                            "File.write_bytes" | "File.append_bytes" => {
                                let (bytes, _) = system_argument(
                                    program,
                                    function,
                                    &arguments[1],
                                    substitutions,
                                );
                                let append = name == "File.append_bytes";
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_file_write_text({path},(const char*)({bytes})->data,({bytes})->len,{append},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                            "File.remove" => format!(
                                "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_file_remove({path},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            ),
                            "File.copy" | "File.move" => {
                                let (to, _) = system_argument(
                                    program,
                                    function,
                                    &arguments[1],
                                    substitutions,
                                );
                                let helper = if name == "File.copy" {
                                    "disp_file_copy"
                                } else {
                                    "disp_file_move"
                                };
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_string _error={{0}};if({helper}({path},{to},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("Directory.") => {
                    let (path, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    if name == "Directory.exists" {
                        format!("disp_directory_exists({path})")
                    } else {
                        let result_c = native_types::c_type(&destination_ty);
                        match name.as_str() {
                            "Directory.read" => {
                                let hir::Type::Result(ok, _) = &destination_ty else {
                                    unreachable!()
                                };
                                let list_c = native_types::c_type(ok);
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_path *_items=NULL;size_t _len=0,_cap=0;disp_native_string _error={{0}};if(disp_directory_read({path},&_items,&_len,&_cap,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=_items,.len=_len,.cap=_cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                            _ => {
                                let helper = match name.as_str() {
                                    "Directory.create" => "disp_directory_create",
                                    "Directory.create_all" => "disp_directory_create_all",
                                    "Directory.remove" => "disp_directory_remove",
                                    _ => unreachable!(),
                                };
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_string _error={{0}};if({helper}({path},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                        }
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if name.starts_with("Time.")
                        || name.starts_with("Duration.")
                        || name == "Instant.elapsed" =>
                {
                    match name.as_str() {
                        "Time.now" => "(disp_native_instant){.nanos=disp_time_now_nanos()}".into(),
                        "Time.unix_seconds" => "disp_time_unix_seconds()".into(),
                        "Time.sleep" => {
                            let actual =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let value =
                                operand(program, function, &arguments[0], &actual, substitutions);
                            format!("(disp_time_sleep(({value}).nanos),(disp_native_unit){{0}})")
                        }
                        "Duration.from_nanos"
                        | "Duration.from_millis"
                        | "Duration.from_seconds" => {
                            let actual =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let value =
                                operand(program, function, &arguments[0], &actual, substitutions);
                            let factor = if name.ends_with("millis") {
                                "1000000ULL"
                            } else if name.ends_with("seconds") {
                                "1000000000ULL"
                            } else {
                                "1ULL"
                            };
                            let negative_check = if matches!(
                                actual,
                                hir::Type::Int { signed: true, .. }
                            ) {
                                format!(
                                    "if(({value})<0)dv_panic(\"Duration value cannot be negative\",{},{});",
                                    span.start.line, span.start.column
                                )
                            } else {
                                String::new()
                            };
                            format!(
                                "({{uint64_t _nanos;{negative_check}if(__builtin_mul_overflow((uint64_t)({value}),(uint64_t){factor},&_nanos))dv_panic(\"Duration overflow\",{},{});(disp_native_duration){{.nanos=_nanos}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        "Instant.elapsed" => {
                            let actual =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let value =
                                operand(program, function, &arguments[0], &actual, substitutions);
                            format!(
                                "(disp_native_duration){{.nanos=disp_time_now_nanos()-({value}).nanos}}"
                            )
                        }
                        "Duration.nanos" | "Duration.millis" | "Duration.seconds" => {
                            let actual =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let value =
                                operand(program, function, &arguments[0], &actual, substitutions);
                            let divisor = if name.ends_with("millis") {
                                "1000000ULL"
                            } else if name.ends_with("seconds") {
                                "1000000000ULL"
                            } else {
                                "1ULL"
                            };
                            format!("({value}).nanos/{divisor}")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "List.new" | "List.with_capacity" | "List.of") =>
                {
                    let hir::Type::List(element) = &destination_ty else {
                        unreachable!()
                    };
                    if name == "List.new" {
                        format!("({}){{0}}", native_types::c_type(&destination_ty))
                    } else if name == "List.of" {
                        let list_ty = native_types::c_type(&destination_ty);
                        let element_ty = native_types::c_type(element);
                        let length = arguments.len();
                        let stores = arguments
                            .iter()
                            .enumerate()
                            .map(|(index, argument)| {
                                let actual = operand_ty(program, function, argument, substitutions);
                                let value =
                                    operand(program, function, argument, &actual, substitutions);
                                format!("_r.data[{index}]={value};")
                            })
                            .collect::<String>();
                        format!(
                            "({{size_t _cap={length};{list_ty} _r={{0}};_r.data=({element_ty}*)disp_alloc(sizeof({element_ty})*_cap,_Alignof({element_ty}));_r.len=_cap;_r.cap=_cap;{stores}_r;}})"
                        )
                    } else {
                        let capacity_ty =
                            operand_ty(program, function, &arguments[0], substitutions);
                        let capacity = operand(
                            program,
                            function,
                            &arguments[0],
                            &capacity_ty,
                            substitutions,
                        );
                        let list_ty = native_types::c_type(&destination_ty);
                        let element_ty = native_types::c_type(element);
                        format!(
                            "({{size_t _cap=(size_t)({capacity});size_t _bytes;if(__builtin_mul_overflow(_cap,sizeof({element_ty}),&_bytes))disp_allocation_failure(\"List capacity overflow\");{list_ty} _r={{0}};if(_cap){{_r.data=({element_ty}*)disp_alloc(_bytes,_Alignof({element_ty}));_r.cap=_cap;}}_r;}})"
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Map.new"
                            | "Map.with_capacity"
                            | "Map.of"
                            | "Set.new"
                            | "Set.with_capacity"
                            | "Set.of"
                    ) =>
                {
                    collection_constructor(
                        program,
                        function,
                        name,
                        arguments,
                        &destination_ty,
                        substitutions,
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "List.len" | "List.capacity" | "List.is_empty"
                    ) =>
                {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    match name.as_str() {
                        "List.len" => format!("({receiver})->len"),
                        "List.capacity" => format!("({receiver})->cap"),
                        _ => format!("({receiver})->len==0"),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "List.push"
                            | "List.pop"
                            | "List.get"
                            | "List.get_mut"
                            | "List.insert"
                            | "List.remove"
                            | "List.clear"
                            | "List.iter"
                    ) =>
                {
                    list_intrinsic(
                        program,
                        function,
                        name,
                        arguments,
                        &destination_ty,
                        substitutions,
                        *span,
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if name.starts_with("Map.") || name.starts_with("Set.") =>
                {
                    collection_intrinsic(
                        program,
                        function,
                        name,
                        arguments,
                        &destination_ty,
                        substitutions,
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "collection.iter" => {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    let hir::Type::Reference { inner, .. } = &receiver_ty else {
                        unreachable!()
                    };
                    let slice_c = native_types::c_type(&destination_ty);
                    match &**inner {
                        hir::Type::Array(_, length) => {
                            format!("({slice_c}){{.data=({receiver})->values,.len={length}}}")
                        }
                        hir::Type::Slice(_) => format!(
                            "({slice_c}){{.data=({receiver})->data,.len=({receiver})->len}}"
                        ),
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "RawPointer.offset" | "RawPointer.read" | "RawPointer.write"
                    ) =>
                {
                    let pointer_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let pointer =
                        operand(program, function, &arguments[0], &pointer_ty, substitutions);
                    match name.as_str() {
                        "RawPointer.offset" => {
                            let offset_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let offset = operand(
                                program,
                                function,
                                &arguments[1],
                                &offset_ty,
                                substitutions,
                            );
                            format!("(({pointer})+(ptrdiff_t)({offset}))")
                        }
                        "RawPointer.read" => format!("(*({pointer}))"),
                        "RawPointer.write" => {
                            let value_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let value =
                                operand(program, function, &arguments[1], &value_ty, substitutions);
                            format!("((*({pointer})={value}),(disp_native_unit){{0}})")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Memory.len"
                            | "Memory.alignment"
                            | "Memory.is_empty"
                            | "Memory.read"
                            | "Memory.write"
                            | "Memory.fill"
                            | "Memory.copy_from"
                            | "Memory.as_ptr"
                            | "Memory.as_mut_ptr"
                    ) =>
                {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    match name.as_str() {
                        "Memory.len" => format!("({receiver})->len"),
                        "Memory.alignment" => format!("({receiver})->align"),
                        "Memory.is_empty" => format!("(({receiver})->len==0)"),
                        "Memory.as_ptr" | "Memory.as_mut_ptr" => {
                            format!("({receiver})->data")
                        }
                        "Memory.read" => {
                            let index_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let index =
                                operand(program, function, &arguments[1], &index_ty, substitutions);
                            format!(
                                "({{size_t _index=(size_t)({index});if(_index>=({receiver})->len)dv_panic(\"Memory index out of bounds\",{},{});({receiver})->data[_index];}})",
                                span.start.line, span.start.column
                            )
                        }
                        "Memory.write" => {
                            let index_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let value_ty =
                                operand_ty(program, function, &arguments[2], substitutions);
                            let index =
                                operand(program, function, &arguments[1], &index_ty, substitutions);
                            let value =
                                operand(program, function, &arguments[2], &value_ty, substitutions);
                            format!(
                                "({{size_t _index=(size_t)({index});if(_index>=({receiver})->len)dv_panic(\"Memory index out of bounds\",{},{});({receiver})->data[_index]=(uint8_t)({value});(disp_native_unit){{0}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        "Memory.fill" => {
                            let value_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let value =
                                operand(program, function, &arguments[1], &value_ty, substitutions);
                            format!(
                                "({{if(({receiver})->len)memset(({receiver})->data,(uint8_t)({value}),({receiver})->len);(disp_native_unit){{0}};}})"
                            )
                        }
                        "Memory.copy_from" => {
                            let destination_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let source_ty =
                                operand_ty(program, function, &arguments[2], substitutions);
                            let offset_ty =
                                operand_ty(program, function, &arguments[3], substitutions);
                            let count_ty =
                                operand_ty(program, function, &arguments[4], substitutions);
                            let destination = operand(
                                program,
                                function,
                                &arguments[1],
                                &destination_ty,
                                substitutions,
                            );
                            let source = operand(
                                program,
                                function,
                                &arguments[2],
                                &source_ty,
                                substitutions,
                            );
                            let offset = operand(
                                program,
                                function,
                                &arguments[3],
                                &offset_ty,
                                substitutions,
                            );
                            let count =
                                operand(program, function, &arguments[4], &count_ty, substitutions);
                            format!(
                                "({{size_t _destination=(size_t)({destination});size_t _offset=(size_t)({offset});size_t _count=(size_t)({count});if(_destination>({receiver})->len||_count>({receiver})->len-_destination||_offset>({source})->len||_count>({source})->len-_offset)dv_panic(\"Memory copy range is out of bounds\",{},{});if(_count)memmove(({receiver})->data+_destination,({source})->data+_offset,_count);(disp_native_unit){{0}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "CString.len"
                            | "CString.is_empty"
                            | "CString.to_string"
                            | "CString.as_c_str"
                    ) =>
                {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    let hir::Type::Reference { inner, .. } = receiver_ty else {
                        unreachable!("CString method receiver must be borrowed")
                    };
                    let (data, len) = match *inner {
                        hir::Type::CString => {
                            (format!("({receiver})->data"), format!("({receiver})->len"))
                        }
                        hir::Type::CStr => {
                            (format!("*({receiver})"), format!("strlen(*({receiver}))"))
                        }
                        _ => unreachable!("type checking validates CString methods"),
                    };
                    match name.as_str() {
                        "CString.len" => len,
                        "CString.is_empty" => format!("({len}==0)"),
                        "CString.to_string" => format!("disp_owned_bytes({data},{len})"),
                        "CString.as_c_str" => data,
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "String.len" | "String.capacity" | "String.is_empty"
                    ) =>
                {
                    let ty = operand_ty(program, function, &arguments[0], substitutions);
                    let value = operand(program, function, &arguments[0], &ty, substitutions);
                    let field = if name == "String.capacity" {
                        "cap"
                    } else {
                        "len"
                    };
                    if name == "String.is_empty" {
                        format!("(({value})->len==0)")
                    } else {
                        format!("({value})->{field}")
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "String.push" | "String.push_str" | "String.clear"
                    ) =>
                {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    let action = match name.as_str() {
                        "String.push" => {
                            let ty = operand_ty(program, function, &arguments[1], substitutions);
                            let value =
                                operand(program, function, &arguments[1], &ty, substitutions);
                            format!("disp_string_push_char({receiver},{value})")
                        }
                        "String.push_str" => {
                            let ty = operand_ty(program, function, &arguments[1], substitutions);
                            let value =
                                operand(program, function, &arguments[1], &ty, substitutions);
                            format!(
                                "disp_string_push_bytes({receiver},({value})->data,({value})->len)"
                            )
                        }
                        _ => format!("({receiver})->len=0"),
                    };
                    format!("({action},(disp_native_unit){{0}})")
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "String.contains" | "String.starts_with" | "String.ends_with"
                    ) =>
                {
                    let left_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let right_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let left = operand(program, function, &arguments[0], &left_ty, substitutions);
                    let right = operand(program, function, &arguments[1], &right_ty, substitutions);
                    let function = match name.as_str() {
                        "String.contains" => "disp_string_contains",
                        "String.starts_with" => "disp_string_starts_with",
                        _ => "disp_string_ends_with",
                    };
                    format!(
                        "{function}(({left})->data,({left})->len,({right})->data,({right})->len)"
                    )
                }
                hir::CallTarget::Intrinsic(name) => {
                    let values = arguments
                        .iter()
                        .map(|argument| {
                            let ty = operand_ty(program, function, argument, substitutions);
                            box_value(
                                program,
                                &operand(program, function, argument, &ty, substitutions),
                                &ty,
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    unbox_value(
                        &format!(
                            "dv_intrinsic(\"{}\",{},(DV[]){{{values}}},{},{})",
                            escape(name),
                            arguments.len(),
                            span.start.line,
                            span.start.column
                        ),
                        &destination_ty,
                    )
                }
                _ => {
                    let target = mono::resolve_target(
                        program,
                        function,
                        instance,
                        target,
                        call_substitutions,
                        arguments,
                    )?
                    .expect("validated direct call must resolve");
                    let target_function = &program.functions[target.function.0];
                    let target_map = mono::mapping(target_function, &target);
                    let arguments = arguments
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            let expected =
                                substitute(&target_function.locals[index + 1].ty, &target_map);
                            operand(program, function, value, &expected, substitutions)
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    let call = format!("{}({arguments})", mono::mangle(program, &target));
                    if target_function.external.is_some()
                        && matches!(destination_ty, hir::Type::Unit)
                    {
                        format!("({call},(disp_native_unit){{0}})")
                    } else {
                        call
                    }
                }
            };
            writeln!(
                output,
                "{}={call};goto bb{};",
                place_expr(program, function, destination, substitutions),
                next.0
            )
            .unwrap();
        }
        mir::Terminator::Spawn {
            target,
            arguments,
            destination,
            next,
            substitutions: call_substitutions,
            span,
        } => {
            let destination_ty = place_ty(program, function, destination, substitutions);
            let hir::Type::Thread(result_ty) = &destination_ty else {
                unreachable!("spawn destination must be Thread<T>")
            };
            let target = mono::resolve_target(
                program,
                function,
                instance,
                &hir::CallTarget::Function(*target),
                call_substitutions,
                arguments,
            )?
            .expect("validated spawn target must resolve");
            let target_function = &program.functions[target.function.0];
            let target_map = mono::mapping(target_function, &target);
            let context = thread_context_name(program, &target);
            let entry = thread_entry_name(program, &target);
            let stores = arguments
                .iter()
                .enumerate()
                .map(|(index, argument)| {
                    let expected = substitute(&target_function.locals[index + 1].ty, &target_map);
                    let value = operand(program, function, argument, &expected, substitutions);
                    format!("_context->a{}={value};", index + 1)
                })
                .collect::<String>();
            let result_c = native_types::c_type(result_ty);
            let expression = format!(
                "({{{result_c} *_result=({result_c}*)disp_alloc_zeroed(1,sizeof({result_c}),_Alignof({result_c}));{context} *_context=({context}*)disp_alloc(sizeof({context}),_Alignof({context}));{stores}_context->result=_result;disp_native_thread _thread={{.handle=disp_thread_start({entry},_context,{},{}) ,.result=_result}};_thread;}})",
                span.start.line, span.start.column
            );
            writeln!(
                output,
                "{}={expression};goto bb{};",
                place_expr(program, function, destination, substitutions),
                next.0
            )
            .unwrap();
        }
        mir::Terminator::Await {
            future,
            destination,
            next,
            span,
        } => {
            let future_ty = operand_ty(program, function, future, substitutions);
            let future = operand(program, function, future, &future_ty, substitutions);
            let destination = place_expr(program, function, destination, substitutions);
            if async_poll {
                writeln!(
                    output,
                    "if(!({future}).poll(({future}).context,&({destination}))){{context->pc={block_index};return false;}}if(({future}).drop)({future}).drop(({future}).context);({future})=(disp_native_future){{0}};goto bb{};",
                    next.0
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "disp_future_wait(&({future}),&({destination}),{},{});goto bb{};",
                    span.start.line, span.start.column, next.0
                )
                .unwrap();
            }
        }
        mir::Terminator::Return if async_poll => writeln!(
            output,
            "*({}*)_output=l0;return true;",
            c_local_type(function, function.return_local, substitutions)
        )
        .unwrap(),
        mir::Terminator::Return => output.push_str("return l0;\n"),
        mir::Terminator::Unreachable => {
            output.push_str("dv_panic(\"entered unreachable MIR block\",0,0);\n")
        }
    }
    Ok(())
}

fn rvalue(
    program: &mir::Program,
    function: &mir::Function,
    instance: &mono::FunctionInstance,
    value: &mir::Rvalue,
    expected: &hir::Type,
    span: crate::diagnostics::Span,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    match value {
        mir::Rvalue::Use(value) => operand(program, function, value, expected, substitutions),
        mir::Rvalue::Function(target) => {
            let target = mono::FunctionInstance {
                function: *target,
                substitutions: vec![],
            };
            format!(
                "(disp_native_callable){{.code=(void (*)(void)){},.env=NULL,.drop=NULL}}",
                callable_wrapper_name(program, &target)
            )
        }
        mir::Rvalue::Closure {
            function: target,
            captures,
        } => {
            let target = mono::FunctionInstance {
                function: *target,
                substitutions: instance.substitutions.clone(),
            };
            let target_function = &program.functions[target.function.0];
            let target_map = mono::mapping(target_function, &target);
            if captures.is_empty() {
                return format!(
                    "(disp_native_callable){{.code=(void (*)(void)){},.env=NULL,.drop=NULL}}",
                    callable_wrapper_name(program, &target)
                );
            }
            let environment = callable_env_name(program, &target);
            let stores = captures
                .iter()
                .enumerate()
                .map(|(index, capture)| {
                    let ty = substitute(&target_function.locals[index + 1].ty, &target_map);
                    format!(
                        "_captures->f{index}={};",
                        operand(program, function, capture, &ty, substitutions)
                    )
                })
                .collect::<String>();
            format!(
                "({{{environment} *_captures=({environment}*)disp_alloc(sizeof({environment}),_Alignof({environment}));{stores}(disp_native_callable){{.code=(void (*)(void)){},.env=_captures,.drop={}}};}})",
                callable_wrapper_name(program, &target),
                callable_drop_name(program, &target)
            )
        }
        mir::Rvalue::UnaryOp(operator, value) => {
            let input_ty = operand_ty(program, function, value, substitutions);
            from_dv(
                &format!(
                    "dv_unary({},{},{},{})",
                    unary(*operator),
                    to_dv(
                        &operand(program, function, value, &input_ty, substitutions),
                        &input_ty,
                    ),
                    span.start.line,
                    span.start.column
                ),
                expected,
            )
        }
        mir::Rvalue::BinaryOp(operator, left, right) => {
            let left_ty = operand_ty(program, function, left, substitutions);
            let right_ty = operand_ty(program, function, right, substitutions);
            from_dv(
                &format!(
                    "dv_binary({},{},{},{},{})",
                    binary(*operator),
                    to_dv(
                        &operand(program, function, left, &left_ty, substitutions),
                        &left_ty,
                    ),
                    to_dv(
                        &operand(program, function, right, &right_ty, substitutions),
                        &right_ty,
                    ),
                    span.start.line,
                    span.start.column
                ),
                expected,
            )
        }
        mir::Rvalue::Aggregate(kind, values) => {
            aggregate(program, function, kind, values, expected, substitutions)
        }
        mir::Rvalue::Discriminant(place) => format!(
            "({})({}).tag",
            native_types::c_type(expected),
            place_expr(program, function, place, substitutions)
        ),
        mir::Rvalue::Len(place) => {
            let ty = place_ty(program, function, place, substitutions);
            match ty {
                hir::Type::Array(_, length) => length.to_string(),
                hir::Type::Slice(_)
                | hir::Type::List(_)
                | hir::Type::Set(_)
                | hir::Type::String
                | hir::Type::Str => {
                    format!(
                        "({}).len",
                        place_expr(program, function, place, substitutions)
                    )
                }
                _ => unreachable!("validated MIR Len requires a collection"),
            }
        }
        mir::Rvalue::BorrowShared(place)
        | mir::Rvalue::BorrowMut(place)
        | mir::Rvalue::RawAddress { place, .. } => {
            format!(
                "({})&({})",
                native_types::c_type(expected),
                place_expr(program, function, place, substitutions)
            )
        }
        mir::Rvalue::Cast { operand: value, .. } => {
            format!(
                "({})({})",
                native_types::c_type(expected),
                operand(program, function, value, expected, substitutions)
            )
        }
    }
}

fn native_key_equal(ty: &hir::Type, left: &str, right: &str) -> String {
    match ty {
        hir::Type::String | hir::Type::Str => format!(
            "(({left}).len==({right}).len&&((({left}).len==0)||memcmp(({left}).data,({right}).data,({left}).len)==0))"
        ),
        _ => format!("(({left})==({right}))"),
    }
}

fn system_argument(
    program: &mir::Program,
    function: &mir::Function,
    argument: &mir::Operand,
    substitutions: &HashMap<String, hir::Type>,
) -> (String, hir::Type) {
    let actual = operand_ty(program, function, argument, substitutions);
    let value = operand(program, function, argument, &actual, substitutions);
    match actual {
        hir::Type::Reference { inner, .. } => (value, *inner),
        other => (format!("&({value})"), other),
    }
}

fn collection_constructor(
    program: &mir::Program,
    function: &mir::Function,
    name: &str,
    arguments: &[mir::Operand],
    destination: &hir::Type,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    let collection_c = native_types::c_type(destination);
    if name.ends_with(".new") {
        return format!("({collection_c}){{0}}");
    }
    if name.ends_with(".with_capacity") {
        let actual = operand_ty(program, function, &arguments[0], substitutions);
        let cap = operand(program, function, &arguments[0], &actual, substitutions);
        return match destination {
            hir::Type::Map(key, value) => {
                let kc = native_types::c_type(key);
                let vc = native_types::c_type(value);
                format!(
                    "({{size_t _cap=(size_t)({cap});{collection_c} _r={{0}};if(_cap){{_r.keys=({kc}*)disp_alloc(sizeof({kc})*_cap,_Alignof({kc}));_r.values=({vc}*)disp_alloc(sizeof({vc})*_cap,_Alignof({vc}));_r.states=(uint8_t*)disp_alloc_zeroed(_cap,1,1);_r.cap=_cap;}}_r;}})"
                )
            }
            hir::Type::Set(element) => {
                let ec = native_types::c_type(element);
                format!(
                    "({{size_t _cap=(size_t)({cap});{collection_c} _r={{0}};if(_cap){{_r.values=({ec}*)disp_alloc(sizeof({ec})*_cap,_Alignof({ec}));_r.states=(uint8_t*)disp_alloc_zeroed(_cap,1,1);_r.cap=_cap;}}_r;}})"
                )
            }
            _ => unreachable!(),
        };
    }
    let count = match destination {
        hir::Type::Map(_, _) => arguments.len() / 2,
        _ => arguments.len(),
    };
    match destination {
        hir::Type::Map(key, value) => {
            let kc = native_types::c_type(key);
            let vc = native_types::c_type(value);
            let stores = arguments.chunks_exact(2).map(|pair| {
                let key_value=operand(program,function,&pair[0],key,substitutions);
                let mapped_value=operand(program,function,&pair[1],value,substitutions);
                let equal=native_key_equal(key,"_r.keys[_i]","_key");
                let old_drop=drop_value(program,"_r.values[_index]",value);
                format!("{{{kc} _key={key_value};{vc} _value={mapped_value};size_t _index=_r.len;for(size_t _i=0;_i<_r.len;_i++)if({equal}){{_index=_i;break;}}if(_index<_r.len){{{old_drop}_r.values[_index]=_value;}}else{{_r.keys[_r.len]=_key;_r.values[_r.len]=_value;_r.len++;}}}}")
            }).collect::<String>();
            format!(
                "({{{collection_c} _r={{0}};size_t _cap={count};if(_cap){{_r.keys=({kc}*)disp_alloc(sizeof({kc})*_cap,_Alignof({kc}));_r.values=({vc}*)disp_alloc(sizeof({vc})*_cap,_Alignof({vc}));_r.states=(uint8_t*)disp_alloc_zeroed(_cap,1,1);_r.cap=_cap;{stores}}}_r;}})"
            )
        }
        hir::Type::Set(element) => {
            let ec = native_types::c_type(element);
            let stores=arguments.iter().map(|argument| { let value=operand(program,function,argument,element,substitutions);let equal=native_key_equal(element,"_r.values[_i]","_value");let duplicate_drop=drop_value(program,"_value",element);format!("{{{ec} _value={value};bool _found=false;for(size_t _i=0;_i<_r.len;_i++)if({equal}){{_found=true;break;}}if(_found){{{duplicate_drop}}}else{{_r.values[_r.len++]=_value;}}}}") }).collect::<String>();
            format!(
                "({{{collection_c} _r={{0}};size_t _cap={count};if(_cap){{_r.values=({ec}*)disp_alloc(sizeof({ec})*_cap,_Alignof({ec}));_r.states=(uint8_t*)disp_alloc_zeroed(_cap,1,1);_r.cap=_cap;{stores}}}_r;}})"
            )
        }
        _ => unreachable!(),
    }
}

fn collection_intrinsic(
    program: &mir::Program,
    function: &mir::Function,
    name: &str,
    arguments: &[mir::Operand],
    destination: &hir::Type,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
    let receiver = operand(
        program,
        function,
        &arguments[0],
        &receiver_ty,
        substitutions,
    );
    let hir::Type::Reference { inner, .. } = &receiver_ty else {
        unreachable!()
    };
    let collection_c = native_types::c_type(inner);
    if name.ends_with(".len") {
        return format!("({receiver})->len");
    }
    if name.ends_with(".capacity") {
        return format!("({receiver})->cap");
    }
    if name.ends_with(".is_empty") {
        return format!("({receiver})->len==0");
    }
    if name.ends_with(".clear") {
        return match &**inner {
            hir::Type::Map(key, value) => {
                let key_drop = drop_value(program, "_collection->keys[_i]", key);
                let value_drop = drop_value(program, "_collection->values[_i]", value);
                format!(
                    "({{{collection_c} *_collection={receiver};for(size_t _i=0;_i<_collection->len;_i++){{{key_drop}{value_drop}}}_collection->len=0;(disp_native_unit){{0}};}})"
                )
            }
            hir::Type::Set(element) => {
                let element_drop = drop_value(program, "_collection->values[_i]", element);
                format!(
                    "({{{collection_c} *_collection={receiver};for(size_t _i=0;_i<_collection->len;_i++){{{element_drop}}}_collection->len=0;(disp_native_unit){{0}};}})"
                )
            }
            _ => unreachable!(),
        };
    }
    if matches!(name, "Map.keys" | "Map.values" | "Set.iter") {
        let slice_c = native_types::c_type(destination);
        let field = if name == "Map.keys" { "keys" } else { "values" };
        return format!("({slice_c}){{.data=({receiver})->{field},.len=({receiver})->len}}");
    }
    match &**inner {
        hir::Type::Map(key, value) => {
            let key_c = native_types::c_type(key);
            let value_c = native_types::c_type(value);
            let key_actual = operand_ty(program, function, &arguments[1], substitutions);
            let key_operand = operand(program, function, &arguments[1], &key_actual, substitutions);
            let key_value = if matches!(key_actual, hir::Type::Reference { .. }) {
                format!("*({key_operand})")
            } else {
                key_operand
            };
            let equal = native_key_equal(key, "_map->keys[_i]", "_key");
            let find = format!(
                "size_t _index=_map->len;for(size_t _i=0;_i<_map->len;_i++)if({equal}){{_index=_i;break;}}"
            );
            match name {
                "Map.has" => format!(
                    "({{{collection_c} *_map={receiver};{key_c} _key={key_value};{find}_index<_map->len;}})"
                ),
                "Map.get" | "Map.get_mut" => {
                    let oc = native_types::c_type(destination);
                    format!(
                        "({{{collection_c} *_map={receiver};{key_c} _key={key_value};{find}{oc} _r={{0}};if(_index<_map->len){{_r.tag=1;_r.payload.v1.f0=&_map->values[_index];}}_r;}})"
                    )
                }
                "Map.set" => {
                    let val = operand(program, function, &arguments[2], value, substitutions);
                    let oc = native_types::c_type(destination);
                    let duplicate_key_drop = drop_value(program, "_key", key);
                    format!(
                        "({{{collection_c} *_map={receiver};{key_c} _key={key_value};{value_c} _value={val};{find}{oc} _old={{0}};if(_index<_map->len){{{duplicate_key_drop}_old.tag=1;_old.payload.v1.f0=_map->values[_index];_map->values[_index]=_value;}}else{{if(_map->len==_map->cap){{size_t _cap;if(_map->cap&&__builtin_mul_overflow(_map->cap,(size_t)2,&_cap))disp_allocation_failure(\"Map capacity overflow\");if(!_map->cap)_cap=4;size_t _kb,_vb;if(__builtin_mul_overflow(sizeof({key_c}),_cap,&_kb)||__builtin_mul_overflow(sizeof({value_c}),_cap,&_vb))disp_allocation_failure(\"Map capacity overflow\");_map->keys=({key_c}*)disp_realloc(_map->keys,_kb,_Alignof({key_c}));_map->values=({value_c}*)disp_realloc(_map->values,_vb,_Alignof({value_c}));_map->states=(uint8_t*)disp_realloc(_map->states,_cap,1);_map->cap=_cap;}}_map->keys[_map->len]=_key;_map->values[_map->len]=_value;_map->len++;}}_old;}})"
                    )
                }
                "Map.remove" => {
                    let oc = native_types::c_type(destination);
                    let key_drop = drop_value(program, "_map->keys[_index]", key);
                    format!(
                        "({{{collection_c} *_map={receiver};{key_c} _key={key_value};{find}{oc} _r={{0}};if(_index<_map->len){{{key_drop}_r.tag=1;_r.payload.v1.f0=_map->values[_index];memmove(_map->keys+_index,_map->keys+_index+1,(_map->len-_index-1)*sizeof({key_c}));memmove(_map->values+_index,_map->values+_index+1,(_map->len-_index-1)*sizeof({value_c}));_map->len--;}}_r;}})"
                    )
                }
                "Map.clear" => format!(
                    "({{{collection_c} *_map={receiver};_map->len=0;(disp_native_unit){{0}};}})"
                ),
                _ => unreachable!(),
            }
        }
        hir::Type::Set(element) => {
            let ec = native_types::c_type(element);
            let actual = operand_ty(program, function, &arguments[1], substitutions);
            let raw = operand(program, function, &arguments[1], &actual, substitutions);
            let val = if matches!(actual, hir::Type::Reference { .. }) {
                format!("*({raw})")
            } else {
                raw
            };
            let equal = native_key_equal(element, "_set->values[_i]", "_value");
            let find = format!(
                "size_t _index=_set->len;for(size_t _i=0;_i<_set->len;_i++)if({equal}){{_index=_i;break;}}"
            );
            match name {
                "Set.has" => format!(
                    "({{{collection_c} *_set={receiver};{ec} _value={val};{find}_index<_set->len;}})"
                ),
                "Set.add" => {
                    let duplicate_drop = drop_value(program, "_value", element);
                    format!(
                        "({{{collection_c} *_set={receiver};{ec} _value={val};{find}bool _added=_index==_set->len;if(_added){{if(_set->len==_set->cap){{size_t _cap;if(_set->cap&&__builtin_mul_overflow(_set->cap,(size_t)2,&_cap))disp_allocation_failure(\"Set capacity overflow\");if(!_set->cap)_cap=4;size_t _bytes;if(__builtin_mul_overflow(sizeof({ec}),_cap,&_bytes))disp_allocation_failure(\"Set capacity overflow\");_set->values=({ec}*)disp_realloc(_set->values,_bytes,_Alignof({ec}));_set->states=(uint8_t*)disp_realloc(_set->states,_cap,1);_set->cap=_cap;}}_set->values[_set->len++]=_value;}}else{{{duplicate_drop}}}_added;}})"
                    )
                }
                "Set.remove" => {
                    let removed_drop = drop_value(program, "_set->values[_index]", element);
                    format!(
                        "({{{collection_c} *_set={receiver};{ec} _value={val};{find}bool _removed=_index<_set->len;if(_removed){{{removed_drop}memmove(_set->values+_index,_set->values+_index+1,(_set->len-_index-1)*sizeof({ec}));_set->len--;}}_removed;}})"
                    )
                }
                "Set.clear" => format!(
                    "({{{collection_c} *_set={receiver};_set->len=0;(disp_native_unit){{0}};}})"
                ),
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

fn list_intrinsic(
    program: &mir::Program,
    function: &mir::Function,
    name: &str,
    arguments: &[mir::Operand],
    destination: &hir::Type,
    substitutions: &HashMap<String, hir::Type>,
    span: crate::diagnostics::Span,
) -> String {
    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
    let receiver = operand(
        program,
        function,
        &arguments[0],
        &receiver_ty,
        substitutions,
    );
    let hir::Type::Reference { inner, .. } = &receiver_ty else {
        unreachable!()
    };
    let hir::Type::List(element) = &**inner else {
        unreachable!()
    };
    let list_ty = native_types::c_type(inner);
    let element_c = native_types::c_type(element);
    let unit = "(disp_native_unit){0}";
    let reserve = format!(
        "size_t _need;if(__builtin_add_overflow(_list->len,(size_t)1,&_need))disp_allocation_failure(\"List length overflow\");if(_need>_list->cap){{size_t _cap=_list->cap?_list->cap:4;while(_cap<_need){{size_t _grown;if(__builtin_mul_overflow(_cap,(size_t)2,&_grown)){{_cap=_need;break;}}_cap=_grown;}}size_t _bytes;if(__builtin_mul_overflow(_cap,sizeof({element_c}),&_bytes))disp_allocation_failure(\"List capacity overflow\");_list->data=({element_c}*)disp_realloc(_list->data,_bytes,_Alignof({element_c}));_list->cap=_cap;}}"
    );
    match name {
        "List.iter" => {
            let slice_c = native_types::c_type(destination);
            format!("({slice_c}){{.data=({receiver})->data,.len=({receiver})->len}}")
        }
        "List.push" => {
            let value = operand(program, function, &arguments[1], element, substitutions);
            format!(
                "({{{list_ty} *_list={receiver};{reserve}_list->data[_list->len++]={value};{unit};}})"
            )
        }
        "List.pop" => {
            let option_ty = native_types::c_type(destination);
            format!(
                "({{{list_ty} *_list={receiver};{option_ty} _r={{0}};if(_list->len){{_r.tag=1;_r.payload.v1.f0=_list->data[--_list->len];}}_r;}})"
            )
        }
        "List.get" | "List.get_mut" => {
            let index_ty = operand_ty(program, function, &arguments[1], substitutions);
            let index = operand(program, function, &arguments[1], &index_ty, substitutions);
            let option_ty = native_types::c_type(destination);
            format!(
                "({{{list_ty} *_list={receiver};size_t _index=(size_t)({index});{option_ty} _r={{0}};if(_index<_list->len){{_r.tag=1;_r.payload.v1.f0=&_list->data[_index];}}_r;}})"
            )
        }
        "List.insert" => {
            let index_ty = operand_ty(program, function, &arguments[1], substitutions);
            let index = operand(program, function, &arguments[1], &index_ty, substitutions);
            let value = operand(program, function, &arguments[2], element, substitutions);
            format!(
                "({{{list_ty} *_list={receiver};size_t _index=(size_t)({index});if(_index>_list->len)dv_panic(\"List insertion index out of bounds\",{},{});{reserve}memmove(_list->data+_index+1,_list->data+_index,(_list->len-_index)*sizeof({element_c}));_list->data[_index]={value};_list->len++;{unit};}})",
                span.start.line, span.start.column
            )
        }
        "List.remove" => {
            let index_ty = operand_ty(program, function, &arguments[1], substitutions);
            let index = operand(program, function, &arguments[1], &index_ty, substitutions);
            format!(
                "({{{list_ty} *_list={receiver};size_t _index=(size_t)({index});if(_index>=_list->len)dv_panic(\"List removal index out of bounds\",{},{});{element_c} _r=_list->data[_index];memmove(_list->data+_index,_list->data+_index+1,(_list->len-_index-1)*sizeof({element_c}));_list->len--;_r;}})",
                span.start.line, span.start.column
            )
        }
        "List.clear" => {
            let element_drop = drop_value(program, "_list->data[_i]", element);
            format!(
                "({{{list_ty} *_list={receiver};for(size_t _i=0;_i<_list->len;_i++){{{element_drop}}}_list->len=0;{unit};}})"
            )
        }
        _ => unreachable!(),
    }
}

fn operand(
    program: &mir::Program,
    function: &mir::Function,
    value: &mir::Operand,
    expected: &hir::Type,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    match value {
        mir::Operand::Move(place_value) | mir::Operand::Copy(place_value) => {
            let expression = place_expr(program, function, place_value, substitutions);
            let actual = place_ty(program, function, place_value, substitutions);
            match (&actual, expected) {
                (
                    hir::Type::Reference {
                        mutable: false,
                        inner: actual,
                    },
                    hir::Type::Reference {
                        mutable: false,
                        inner: expected_inner,
                    },
                ) if matches!(
                    (&**actual, &**expected_inner),
                    (hir::Type::String, hir::Type::Str)
                ) =>
                {
                    format!("({})({expression})", native_types::c_type(expected))
                }
                (hir::Type::Result(_, actual_error), hir::Type::Result(_, expected_error))
                    if actual != *expected && actual_error == expected_error =>
                {
                    format!(
                        "({}){{.tag=1,.payload.v1={{.f0=({expression}).payload.v1.f0}}}}",
                        native_types::c_type(expected)
                    )
                }
                (hir::Type::Option(_), hir::Type::Option(_)) if actual != *expected => format!(
                    "({}){{.tag=0,.payload.v0={{}}}}",
                    native_types::c_type(expected)
                ),
                _ => expression,
            }
        }
        mir::Operand::Constant(value) => constant_expr(value, expected),
    }
}

fn aggregate(
    program: &mir::Program,
    function: &mir::Function,
    kind: &mir::AggregateKind,
    values: &[mir::Operand],
    expected: &hir::Type,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    match kind {
        mir::AggregateKind::Array => {
            let hir::Type::Array(element, _) = expected else {
                unreachable!()
            };
            let fields = values
                .iter()
                .map(|value| operand(program, function, value, element, substitutions))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "({}){{.values={{{fields}}}}}",
                native_types::c_type(expected)
            )
        }
        mir::AggregateKind::Struct(_) => {
            let fields = values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let field_ty = aggregate_field_ty(program, expected, None, index);
                    format!(
                        ".f{index}={}",
                        operand(program, function, value, &field_ty, substitutions)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("({}){{{fields}}}", native_types::c_type(expected))
        }
        mir::AggregateKind::Enum(_, variant) => {
            let index = variant_index(program, expected, *variant);
            let fields = values
                .iter()
                .enumerate()
                .map(|(field, value)| {
                    let field_ty = aggregate_field_ty(program, expected, Some(*variant), field);
                    format!(
                        ".f{field}={}",
                        operand(program, function, value, &field_ty, substitutions)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "({}){{.tag={index},.payload.v{index}={{{fields}}}}}",
                native_types::c_type(expected)
            )
        }
    }
}

fn aggregate_field_ty(
    program: &mir::Program,
    aggregate: &hir::Type,
    variant: Option<hir::VariantId>,
    field: usize,
) -> hir::Type {
    match aggregate {
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            substitute(&declaration.fields[field].ty, &substitutions)
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            let declaration_variant = declaration
                .variants
                .iter()
                .find(|candidate| Some(candidate.id) == variant)
                .unwrap();
            substitute(&declaration_variant.payload[field], &substitutions)
        }
        hir::Type::Option(inner) => (**inner).clone(),
        hir::Type::Result(ok, error) => {
            if variant == Some(hir::builtin_variant("Ok")) {
                (**ok).clone()
            } else {
                (**error).clone()
            }
        }
        hir::Type::Array(element, _) => (**element).clone(),
        _ => unreachable!(),
    }
}

fn variant_index(program: &mir::Program, ty: &hir::Type, variant: hir::VariantId) -> usize {
    match ty {
        hir::Type::Enum(id, _) => {
            program.enums[id.0]
                .variants
                .iter()
                .find(|candidate| candidate.id == variant)
                .unwrap()
                .index
        }
        hir::Type::Option(_) => usize::from(variant == hir::builtin_variant("Some")),
        hir::Type::Result(_, _) => usize::from(variant == hir::builtin_variant("Err")),
        _ => unreachable!(),
    }
}

fn constant_expr(value: &mir::Constant, expected: &hir::Type) -> String {
    match value {
        mir::Constant::Signed(value, _) => format!("({}){value}", native_types::c_type(expected)),
        mir::Constant::Unsigned(value, _) => {
            let high = (*value >> 64) as u64;
            let low = *value as u64;
            format!(
                "({})(((unsigned __int128){high}ULL<<64)|{low}ULL)",
                native_types::c_type(expected)
            )
        }
        mir::Constant::Float(value, _) => format!("({}){value:?}", native_types::c_type(expected)),
        mir::Constant::Bool(value) => value.to_string(),
        mir::Constant::Char(value) => format!("(uint32_t){}", *value as u32),
        mir::Constant::String(value) => format!(
            "(disp_native_string){{(char*)\"{}\",{},0}}",
            escape(value),
            value.len()
        ),
        mir::Constant::Unit => "(disp_native_unit){0}".into(),
    }
}

fn to_dv(value: &str, ty: &hir::Type) -> String {
    match ty {
        hir::Type::Unit => "dv_unit()".into(),
        hir::Type::Bool => format!("dv_bool({value})"),
        hir::Type::Char => format!("dv_char({value})"),
        hir::Type::String => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::CString => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::CStr => format!("dv_string(({value}),strlen({value}))"),
        hir::Type::Memory => "dv_string(\"<Memory>\",8)".into(),
        hir::Type::Path => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::Instant | hir::Type::Duration => {
            format!("dv_u((unsigned __int128)({value}).nanos,64)")
        }
        hir::Type::Str => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::Int {
            signed: true,
            width,
        } => {
            format!("dv_i((__int128)({value}),{})", width.unwrap_or(64))
        }
        hir::Type::Int {
            signed: false,
            width,
        } => {
            format!("dv_u((unsigned __int128)({value}),{})", width.unwrap_or(64))
        }
        hir::Type::Float { width } => format!("dv_f((double)({value}),{width})"),
        hir::Type::Reference { inner, .. } => to_dv(&format!("*({value})"), inner),
        hir::Type::Generic(_) => format!("dv_string(({value}).data,({value}).len)"),
        _ => unreachable!(),
    }
}

fn box_value(program: &mir::Program, value: &str, ty: &hir::Type) -> String {
    match ty {
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            let fields = declaration
                .fields
                .iter()
                .map(|field| {
                    let ty = substitute(&field.ty, &substitutions);
                    box_value(program, &format!("({value}).f{}", field.index), &ty)
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "dv_aggregate(\"{}\",NULL,0,{},(DV[]){{{fields}}})",
                escape(&declaration.name),
                declaration.fields.len()
            )
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            declaration
                .variants
                .iter()
                .rev()
                .fold("dv_unit()".to_string(), |otherwise, variant| {
                    let fields = variant
                        .payload
                        .iter()
                        .enumerate()
                        .map(|(field, ty)| {
                            let ty = substitute(ty, &substitutions);
                            box_value(
                                program,
                                &format!("({value}).payload.v{}.f{field}", variant.index),
                                &ty,
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    format!(
                        "(({value}).tag=={}?dv_aggregate(\"{}\",\"{}\",{}, {},(DV[]){{{fields}}}):{otherwise})",
                        variant.index,
                        escape(&declaration.name),
                        escape(&variant.name),
                        variant.id.0,
                        variant.payload.len()
                    )
                })
        }
        hir::Type::Option(inner) => {
            let some = box_value(program, &format!("({value}).payload.v1.f0"), inner);
            format!(
                "(({value}).tag==1?dv_aggregate(\"Option\",\"Some\",{},1,(DV[]){{{some}}}):dv_aggregate(\"Option\",\"None\",{},0,(DV[]){{0}}))",
                hir::builtin_variant("Some").0,
                hir::builtin_variant("None").0
            )
        }
        hir::Type::Result(ok, error) => {
            let ok = box_value(program, &format!("({value}).payload.v0.f0"), ok);
            let error = box_value(program, &format!("({value}).payload.v1.f0"), error);
            format!(
                "(({value}).tag==0?dv_aggregate(\"Result\",\"Ok\",{},1,(DV[]){{{ok}}}):dv_aggregate(\"Result\",\"Err\",{},1,(DV[]){{{error}}}))",
                hir::builtin_variant("Ok").0,
                hir::builtin_variant("Err").0
            )
        }
        hir::Type::Reference { inner, .. } | hir::Type::RawPointer { inner, .. } => {
            box_value(program, &format!("*({value})"), inner)
        }
        hir::Type::Generic(_) => to_dv(value, ty),
        _ => to_dv(value, ty),
    }
}

fn from_dv(value: &str, ty: &hir::Type) -> String {
    match ty {
        hir::Type::Unit => format!("((void)({value}),(disp_native_unit){{0}})"),
        hir::Type::Bool => format!("({value}).as.boolean"),
        hir::Type::Char => format!("({value}).as.ch"),
        hir::Type::String => {
            format!("(disp_native_string){{({value}).as.string.data,({value}).as.string.len}}")
        }
        hir::Type::Str => {
            format!("(disp_native_str){{({value}).as.string.data,({value}).as.string.len}}")
        }
        hir::Type::Generic(_) => {
            format!("(disp_native_string){{({value}).as.string.data,({value}).as.string.len}}")
        }
        hir::Type::Int { signed: true, .. } => {
            format!("({})({value}).as.si", native_types::c_type(ty))
        }
        hir::Type::Int { signed: false, .. } => {
            format!("({})({value}).as.ui", native_types::c_type(ty))
        }
        hir::Type::Float { .. } => format!("({})({value}).as.fp", native_types::c_type(ty)),
        _ => unreachable!(),
    }
}

fn unbox_value(value: &str, ty: &hir::Type) -> String {
    match ty {
        hir::Type::Option(inner) => {
            let some = unbox_value("*dv_field(&_v,0)", inner);
            format!(
                "({{DV _v={value};{} _r={{0}};if(dv_disc(_v)=={}){{_r.tag=1;_r.payload.v1.f0={some};}}else{{_r.tag=0;}}dv_drop(&_v);_r;}})",
                native_types::c_type(ty),
                hir::builtin_variant("Some").0
            )
        }
        hir::Type::Result(ok, error) => {
            let ok_value = unbox_value("*dv_field(&_v,0)", ok);
            let error_value = unbox_value("*dv_field(&_v,0)", error);
            format!(
                "({{DV _v={value};{} _r={{0}};if(dv_disc(_v)=={}){{_r.tag=0;_r.payload.v0.f0={ok_value};}}else{{_r.tag=1;_r.payload.v1.f0={error_value};}}dv_drop(&_v);_r;}})",
                native_types::c_type(ty),
                hir::builtin_variant("Ok").0
            )
        }
        _ => from_dv(value, ty),
    }
}

fn operand_ty(
    program: &mir::Program,
    function: &mir::Function,
    operand: &mir::Operand,
    substitutions: &HashMap<String, hir::Type>,
) -> hir::Type {
    match operand {
        mir::Operand::Move(place) | mir::Operand::Copy(place) => {
            place_ty(program, function, place, substitutions)
        }
        mir::Operand::Constant(mir::Constant::Bool(_)) => hir::Type::Bool,
        mir::Operand::Constant(mir::Constant::Char(_)) => hir::Type::Char,
        mir::Operand::Constant(mir::Constant::String(_)) => hir::Type::String,
        mir::Operand::Constant(mir::Constant::Float(_, width)) => {
            hir::Type::Float { width: *width }
        }
        mir::Operand::Constant(mir::Constant::Signed(_, width)) => hir::Type::Int {
            signed: true,
            width: *width,
        },
        mir::Operand::Constant(mir::Constant::Unsigned(_, width)) => hir::Type::Int {
            signed: false,
            width: *width,
        },
        mir::Operand::Constant(mir::Constant::Unit) => hir::Type::Unit,
    }
}

fn place_ty(
    program: &mir::Program,
    function: &mir::Function,
    place: &mir::Place,
    substitutions: &HashMap<String, hir::Type>,
) -> hir::Type {
    let mut ty = substitute(&function.locals[place.local.0].ty, substitutions);
    for projection in &place.projections {
        ty = match projection {
            mir::Projection::SafeDereference | mir::Projection::RawDereference => match ty {
                hir::Type::Reference { inner, .. } | hir::Type::RawPointer { inner, .. } => *inner,
                hir::Type::MutexGuard(inner) => *inner,
                _ => unreachable!(),
            },
            mir::Projection::Field(index) => aggregate_field_ty(program, &ty, None, *index),
            mir::Projection::VariantField(variant, index) => {
                aggregate_field_ty(program, &ty, Some(*variant), *index)
            }
            mir::Projection::Index { .. } => match ty {
                hir::Type::Array(element, _)
                | hir::Type::Slice(element)
                | hir::Type::List(element) => *element,
                hir::Type::Set(element) => *element,
                _ => unreachable!(),
            },
            mir::Projection::Subslice { .. } => match ty {
                hir::Type::Array(element, _)
                | hir::Type::Slice(element)
                | hir::Type::List(element) => hir::Type::Slice(element),
                hir::Type::String | hir::Type::Str => hir::Type::Str,
                _ => unreachable!(),
            },
        };
    }
    ty
}

fn drop_value(program: &mir::Program, value: &str, ty: &hir::Type) -> String {
    drop_value_depth(program, value, ty, 0)
}

fn drop_value_depth(program: &mir::Program, value: &str, ty: &hir::Type, depth: usize) -> String {
    match ty {
        hir::Type::String => format!("disp_string_drop(&({value}));"),
        hir::Type::CString => format!("disp_cstring_drop(&({value}));"),
        hir::Type::Memory => format!("disp_memory_drop(&({value}));"),
        hir::Type::Path => format!("disp_path_drop(&({value}));"),
        hir::Type::Thread(result) => {
            let result_c = native_types::c_type(result);
            let result_drop = drop_value_depth(
                program,
                &format!("*({result_c}*)({value}).result"),
                result,
                depth + 1,
            );
            format!(
                "{{disp_thread_wait(&({value}));if(({value}).result){{{result_drop}disp_dealloc(({value}).result);({value}).result=NULL;}}}}"
            )
        }
        hir::Type::Mutex(value_ty) => {
            let value_c = native_types::c_type(value_ty);
            let value_drop = drop_value_depth(
                program,
                &format!("*({value_c}*)({value}).state->data"),
                value_ty,
                depth + 1,
            );
            format!(
                "{{if(({value}).state&&disp_mutex_release(({value}).state)){{{value_drop}disp_dealloc(({value}).state->data);disp_dealloc(({value}).state);}}({value}).state=NULL;}}"
            )
        }
        hir::Type::MutexGuard(value_ty) => {
            let value_c = native_types::c_type(value_ty);
            let value_drop = drop_value_depth(
                program,
                &format!("*({value_c}*)({value}).state->data"),
                value_ty,
                depth + 1,
            );
            format!(
                "{{if(({value}).state){{disp_mutex_unlock(({value}).state);if(disp_mutex_release(({value}).state)){{{value_drop}disp_dealloc(({value}).state->data);disp_dealloc(({value}).state);}}({value}).state=NULL;}}}}"
            )
        }
        hir::Type::AtomicInt => format!(
            "{{if(({value}).state&&disp_atomic_int_release(({value}).state))disp_dealloc(({value}).state);({value}).state=NULL;}}"
        ),
        hir::Type::List(element) => {
            let index = format!("_drop_i{depth}");
            let element_drop = drop_value_depth(
                program,
                &format!("({value}).data[{index}]"),
                element,
                depth + 1,
            );
            format!(
                "{{for(size_t {index}=0;{index}<({value}).len;{index}++){{{element_drop}}}disp_dealloc(({value}).data);({value}).data=NULL;({value}).len=0;({value}).cap=0;}}"
            )
        }
        hir::Type::Map(key, element) => {
            let index = format!("_drop_i{depth}");
            let key_drop =
                drop_value_depth(program, &format!("({value}).keys[{index}]"), key, depth + 1);
            let value_drop = drop_value_depth(
                program,
                &format!("({value}).values[{index}]"),
                element,
                depth + 1,
            );
            format!(
                "{{for(size_t {index}=0;{index}<({value}).len;{index}++){{{key_drop}{value_drop}}}disp_dealloc(({value}).keys);disp_dealloc(({value}).values);disp_dealloc(({value}).states);({value})=({}){{0}};}}",
                native_types::c_type(ty)
            )
        }
        hir::Type::Set(element) => {
            let index = format!("_drop_i{depth}");
            let element_drop = drop_value_depth(
                program,
                &format!("({value}).values[{index}]"),
                element,
                depth + 1,
            );
            format!(
                "{{for(size_t {index}=0;{index}<({value}).len;{index}++){{{element_drop}}}disp_dealloc(({value}).values);disp_dealloc(({value}).states);({value})=({}){{0}};}}",
                native_types::c_type(ty)
            )
        }
        hir::Type::Array(element, length) => {
            let index = format!("_drop_i{depth}");
            let element_drop = drop_value_depth(
                program,
                &format!("({value}).values[{index}]"),
                element,
                depth + 1,
            );
            format!("{{for(size_t {index}=0;{index}<{length};{index}++){{{element_drop}}}}}")
        }
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            declaration
                .fields
                .iter()
                .map(|field| {
                    drop_value_depth(
                        program,
                        &format!("({value}).f{}", field.index),
                        &substitute(&field.ty, &substitutions),
                        depth + 1,
                    )
                })
                .collect::<String>()
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            let cases = declaration
                .variants
                .iter()
                .map(|variant| {
                    let fields = variant
                        .payload
                        .iter()
                        .enumerate()
                        .map(|(field, ty)| {
                            drop_value_depth(
                                program,
                                &format!("({value}).payload.v{}.f{field}", variant.index),
                                &substitute(ty, &substitutions),
                                depth + 1,
                            )
                        })
                        .collect::<String>();
                    format!("case {}:{{{fields}}}break;", variant.index)
                })
                .collect::<String>();
            format!("{{switch(({value}).tag){{{cases}default:break;}}}}")
        }
        hir::Type::Option(inner) => {
            let payload = drop_value_depth(
                program,
                &format!("({value}).payload.v1.f0"),
                inner,
                depth + 1,
            );
            format!("{{if(({value}).tag==1){{{payload}}}}}")
        }
        hir::Type::Result(ok, error) => {
            let ok_drop =
                drop_value_depth(program, &format!("({value}).payload.v0.f0"), ok, depth + 1);
            let error_drop = drop_value_depth(
                program,
                &format!("({value}).payload.v1.f0"),
                error,
                depth + 1,
            );
            format!("{{if(({value}).tag==0){{{ok_drop}}}else{{{error_drop}}}}}")
        }
        hir::Type::Function(_, _) => format!(
            "{{if(({value}).drop)({value}).drop(({value}).env);({value})=(disp_native_callable){{0}};}}"
        ),
        hir::Type::Future(_) => format!(
            "{{if(({value}).drop)({value}).drop(({value}).context);({value})=(disp_native_future){{0}};}}"
        ),
        _ => String::new(),
    }
}

fn c_local_type(
    function: &mir::Function,
    local: mir::LocalId,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    native_types::c_type(&substitute(&function.locals[local.0].ty, substitutions))
}

fn place_expr(
    program: &mir::Program,
    function: &mir::Function,
    place: &mir::Place,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    let mut expression = format!("l{}", place.local.0);
    let mut ty = substitute(&function.locals[place.local.0].ty, substitutions);
    for projection in &place.projections {
        match projection {
            mir::Projection::SafeDereference | mir::Projection::RawDereference => {
                ty = match ty {
                    hir::Type::Reference { inner, .. } | hir::Type::RawPointer { inner, .. } => {
                        expression = format!("(*({expression}))");
                        *inner
                    }
                    hir::Type::MutexGuard(inner) => {
                        expression = format!(
                            "(*({}*)({expression}).state->data)",
                            native_types::c_type(&inner)
                        );
                        *inner
                    }
                    _ => unreachable!(),
                };
            }
            mir::Projection::Field(index) => {
                expression = format!("({expression}).f{index}");
                ty = aggregate_field_ty(program, &ty, None, *index);
            }
            mir::Projection::VariantField(variant, index) => {
                let variant_index = variant_index(program, &ty, *variant);
                expression = format!("({expression}).payload.v{variant_index}.f{index}");
                ty = aggregate_field_ty(program, &ty, Some(*variant), *index);
            }
            mir::Projection::Index { index, span } => {
                let (data, len, element) = match ty {
                    hir::Type::Array(element, length) => (
                        format!("({expression}).values"),
                        length.to_string(),
                        *element,
                    ),
                    hir::Type::Slice(element) | hir::Type::List(element) => (
                        format!("({expression}).data"),
                        format!("({expression}).len"),
                        *element,
                    ),
                    hir::Type::Set(element) => (
                        format!("({expression}).values"),
                        format!("({expression}).len"),
                        *element,
                    ),
                    _ => unreachable!(),
                };
                expression = format!(
                    "(*((uint64_t)l{}<(uint64_t)({len})?&({data}[(uint64_t)l{}]):(dv_panic(\"index out of bounds\",{},{}),&({data}[0]))))",
                    index.0, index.0, span.start.line, span.start.column
                );
                ty = element;
            }
            mir::Projection::Subslice { start, end, span } => {
                if matches!(ty, hir::Type::String | hir::Type::Str) {
                    let data = format!("({expression}).data");
                    let len = format!("({expression}).len");
                    expression = format!(
                        "((uint64_t)l{}<=(uint64_t)l{}&&(uint64_t)l{}<=(uint64_t)({len})&&disp_utf8_boundary({data},(size_t)({len}),(size_t)l{})&&disp_utf8_boundary({data},(size_t)({len}),(size_t)l{})?(disp_native_str){{.data=({data})+(uint64_t)l{},.len=(uint64_t)l{}-(uint64_t)l{}}}:(dv_panic(\"string slice is out of bounds or not on UTF-8 boundaries\",{},{}),(disp_native_str){{0}}))",
                        start.0,
                        end.0,
                        end.0,
                        start.0,
                        end.0,
                        start.0,
                        end.0,
                        start.0,
                        span.start.line,
                        span.start.column,
                    );
                    ty = hir::Type::Str;
                    continue;
                }
                let (data, len, element) = match ty {
                    hir::Type::Array(element, length) => (
                        format!("({expression}).values"),
                        length.to_string(),
                        *element,
                    ),
                    hir::Type::Slice(element) | hir::Type::List(element) => (
                        format!("({expression}).data"),
                        format!("({expression}).len"),
                        *element,
                    ),
                    _ => unreachable!(),
                };
                let slice_ty = hir::Type::Slice(Box::new(element.clone()));
                expression = format!(
                    "((uint64_t)l{}<=(uint64_t)l{}&&(uint64_t)l{}<=(uint64_t)({len})?({}){{.data=({data})+(uint64_t)l{},.len=(uint64_t)l{}-(uint64_t)l{}}}:(dv_panic(\"subslice range out of bounds\",{},{}),({}){{0}}))",
                    start.0,
                    end.0,
                    end.0,
                    native_types::c_type(&slice_ty),
                    start.0,
                    end.0,
                    start.0,
                    span.start.line,
                    span.start.column,
                    native_types::c_type(&slice_ty)
                );
                ty = slice_ty;
            }
        }
    }
    expression
}

fn binary(operator: ast::BinaryOperator) -> usize {
    match operator {
        ast::BinaryOperator::Add => 0,
        ast::BinaryOperator::Subtract => 1,
        ast::BinaryOperator::Multiply => 2,
        ast::BinaryOperator::Divide => 3,
        ast::BinaryOperator::Remainder => 4,
        ast::BinaryOperator::Equal => 6,
        ast::BinaryOperator::NotEqual => 7,
        ast::BinaryOperator::Less => 8,
        ast::BinaryOperator::LessEqual => 9,
        ast::BinaryOperator::Greater => 10,
        ast::BinaryOperator::GreaterEqual => 11,
        ast::BinaryOperator::And => 12,
        ast::BinaryOperator::Or => 13,
    }
}

fn unary(operator: ast::UnaryOperator) -> usize {
    match operator {
        ast::UnaryOperator::Negate => 0,
        ast::UnaryOperator::Not => 1,
    }
}

fn escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'\\' => "\\\\".into(),
            b'\"' => "\\\"".into(),
            b'\n' => "\\n".into(),
            b'\r' => "\\r".into(),
            b'\t' => "\\t".into(),
            32..=126 => (byte as char).to_string(),
            _ => format!("\\{byte:03o}"),
        })
        .collect()
}
