//! Direct, concrete lowering for the scalar/string portion of MIR.
//!
//! Unsupported aggregate/reference functions deliberately fall back to the
//! general backend while their concrete lowering is implemented. This module
//! never changes program semantics merely to make a function eligible.

use super::{
    abi::AbiProgram, allocator::C_ALLOCATOR, layout::substitute, mono, native_types,
    runtime::C_RUNTIME,
};
use crate::{ast, diagnostics::Diagnostic, hir, limits, mir};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Write,
};

pub fn generate(
    program: &mir::Program,
    instances: &mono::MonoProgram,
    abi: &AbiProgram,
    declarations: &str,
    library: bool,
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
    let mut output = limits::native_prelude();
    output.push_str(C_ALLOCATOR);
    output.push_str(declarations);
    emit_source_map(program, &mut output);
    output.push_str(C_RUNTIME);
    let (json_encoders, json_decoders) = json_codec_types(program, instances);
    emit_json_codecs(program, &json_encoders, &json_decoders, &mut output);
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
    let context_callbacks = c_context_callback_types(program, instances);
    let exported = instances
        .instances
        .iter()
        .filter(|instance| program.functions[instance.function.0].exported)
        .collect::<Vec<_>>();
    if !exported.is_empty() || !context_callbacks.is_empty() {
        output.push_str(
            "#ifdef _WIN32\n#define DISP_C_EXPORT __declspec(dllexport)\n#else\n#define DISP_C_EXPORT __attribute__((visibility(\"default\")))\n#endif\n\
             DISP_C_EXPORT const char *disp_c_last_error(void){return disp_ffi_last_error;}\n\
             DISP_C_EXPORT int32_t disp_c_thread_attach(void){if(disp_ffi_thread_attached){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"current thread is already attached to DISP\");return 4;}disp_ffi_thread_attached=true;disp_ffi_last_error[0]=0;return 0;}\n\
             DISP_C_EXPORT int32_t disp_c_thread_detach(void){if(!disp_ffi_thread_attached){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"current thread is not attached to DISP\");return 3;}if(disp_ffi_panic_target){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"cannot detach during an active DISP export\");return 5;}disp_ffi_thread_attached=false;disp_ffi_last_error[0]=0;return 0;}\n",
        );
        for instance in &exported {
            export_prototype(program, instance, &mut output);
        }
    }
    for callback in &context_callbacks {
        emit_c_context_callback(callback, &mut output);
    }
    for result in task_result_types(program, instances) {
        task_result_drop_wrapper(program, &result, &mut output);
    }
    for (operation, result) in async_operations(program, instances) {
        async_poll_wrapper(&operation, &result, &mut output);
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
    if !exported.is_empty() {
        for instance in exported {
            export_wrapper(program, instance, &mut output);
        }
    }
    if library {
        return Ok(Some(output));
    }
    let entry_function = &program.functions[instances.entry.function.0];
    let entry_arguments = if entry_function.argument_count == 1 {
        let argument_ty = substitute(
            &entry_function.locals[1].ty,
            &mono::mapping(entry_function, &instances.entry),
        );
        let argument_c = native_types::c_type(&argument_ty);
        format!(
            "{argument_c} args=({argument_c}){{0}};if(disp_program_argc>0){{args.data=(disp_native_string*)disp_alloc_zeroed((size_t)disp_program_argc,sizeof(disp_native_string),_Alignof(disp_native_string));args.len=args.cap=(size_t)disp_program_argc;for(size_t i=0;i<args.len;i++){{size_t n=strlen(disp_program_argv[i]);if(!disp_utf8_valid(disp_program_argv[i],n))dv_panic(\"program argument is not valid UTF-8\",0,0);args.data[i]=disp_owned_bytes(disp_program_argv[i],n);}}}}"
        )
    } else {
        String::new()
    };
    let call_arguments = if entry_function.argument_count == 1 {
        "args"
    } else {
        ""
    };
    if entry_function.asynchronous {
        let substitutions = mono::mapping(entry_function, &instances.entry);
        let result = c_local_type(entry_function, entry_function.return_local, &substitutions);
        writeln!(
            output,
            "int main(int argc,char **argv){{disp_program_arguments_init(argc,argv);{entry_arguments}disp_native_future future={}({call_arguments});{result} result=({result}){{0}};disp_future_wait(&future,&result,0,0);(void)result;disp_program_arguments_drop();return 0;}}",
            mono::mangle(program, &instances.entry)
        )
        .unwrap();
    } else {
        writeln!(
            output,
            "int main(int argc,char **argv){{disp_program_arguments_init(argc,argv);{entry_arguments}{} result={}({call_arguments});(void)result;disp_program_arguments_drop();return 0;}}",
            native_types::c_type(&hir::Type::Unit),
            mono::mangle(program, &instances.entry)
        )
        .unwrap();
    }
    Ok(Some(output))
}

fn c_context_callback_name(handler: &hir::Type) -> String {
    format!("disp_c_context_callback_{}", mono::type_code(handler))
}

fn c_context_callback_types(
    program: &mir::Program,
    instances: &mono::MonoProgram,
) -> BTreeSet<hir::Type> {
    let mut handlers = BTreeSet::new();
    for instance in &instances.instances {
        let function = &program.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        for block in &function.blocks {
            if let mir::Terminator::Call {
                target: hir::CallTarget::Intrinsic(name),
                arguments,
                ..
            } = &block.terminator
                && name.starts_with("CRegistration.register_async:")
            {
                handlers.insert(operand_ty(program, function, &arguments[0], &substitutions));
            }
        }
    }
    handlers
}

fn emit_c_context_callback(handler: &hir::Type, output: &mut String) {
    let hir::Type::Function(parameters, result) = handler else {
        unreachable!("validated captured callback handler has function type")
    };
    write!(
        output,
        "static int32_t {}(void *raw",
        c_context_callback_name(handler)
    )
    .unwrap();
    for (index, parameter) in parameters.iter().enumerate() {
        write!(
            output,
            ",{} a{}",
            native_types::c_type(parameter),
            index + 1
        )
        .unwrap();
    }
    let has_result = !matches!(**result, hir::Type::Unit);
    if has_result {
        write!(output, ",{} *out_result", native_types::c_type(result)).unwrap();
    }
    output.push_str("){if(!disp_ffi_thread_attached){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"current thread is not attached to DISP\");return 3;}if(disp_ffi_panic_target){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"nested DISP C callback entry is unavailable\");return 2;}if(!raw){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"DISP callback context is null\");return 2;}");
    if has_result {
        output.push_str("if(!out_result){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"callback result pointer is null\");return 2;}");
    }
    output.push_str("disp_native_callable *callback=(disp_native_callable*)raw;if(!callback->code){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"DISP callback code is null\");return 2;}jmp_buf target;disp_ffi_panic_target=&target;if(setjmp(target)){disp_ffi_panic_target=NULL;return 1;}");
    let mut signature = format!("{} (*)(void *", native_types::c_type(result));
    for parameter in parameters {
        write!(signature, ",{}", native_types::c_type(parameter)).unwrap();
    }
    signature.push(')');
    let arguments = (0..parameters.len())
        .map(|index| format!(",a{}", index + 1))
        .collect::<String>();
    if has_result {
        write!(
            output,
            "{} value=(({})callback->code)(callback->env{});",
            native_types::c_type(result),
            signature,
            arguments
        )
        .unwrap();
    } else {
        write!(
            output,
            "(void)((({})callback->code)(callback->env{}));",
            signature, arguments
        )
        .unwrap();
    }
    output.push_str("disp_ffi_panic_target=NULL;disp_ffi_last_error[0]=0;");
    if has_result {
        output.push_str("*out_result=value;");
    }
    output.push_str("return 0;}\n");
}

fn export_prototype(
    program: &mir::Program,
    instance: &mono::FunctionInstance,
    output: &mut String,
) {
    let function = &program.functions[instance.function.0];
    let substitutions = mono::mapping(function, instance);
    let result_ty = substitute(&function.locals[function.return_local.0].ty, &substitutions);
    write!(output, "DISP_C_EXPORT int32_t {}(", function.name).unwrap();
    let mut count = 0usize;
    for index in 0..function.argument_count {
        if count > 0 {
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
        count += 1;
    }
    if !matches!(result_ty, hir::Type::Unit) {
        if count > 0 {
            output.push(',');
        }
        write!(output, "{} *out_result", native_types::c_type(&result_ty)).unwrap();
        count += 1;
    }
    if count == 0 {
        output.push_str("void");
    }
    output.push_str(");\n");
}

fn export_wrapper(program: &mir::Program, instance: &mono::FunctionInstance, output: &mut String) {
    let function = &program.functions[instance.function.0];
    let substitutions = mono::mapping(function, instance);
    let result_ty = substitute(&function.locals[function.return_local.0].ty, &substitutions);
    write!(output, "DISP_C_EXPORT int32_t {}(", function.name).unwrap();
    let mut count = 0usize;
    for index in 0..function.argument_count {
        if count > 0 {
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
        count += 1;
    }
    let has_result = !matches!(result_ty, hir::Type::Unit);
    if has_result {
        if count > 0 {
            output.push(',');
        }
        write!(output, "{} *out_result", native_types::c_type(&result_ty)).unwrap();
        count += 1;
    }
    if count == 0 {
        output.push_str("void");
    }
    output.push_str("){if(!disp_ffi_thread_attached){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"current thread is not attached to DISP\");return 3;}if(disp_ffi_panic_target){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"nested DISP C export entry is unavailable\");return 2;}");
    if has_result {
        output.push_str("if(!out_result){snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),\"export result pointer is null\");return 2;}");
    }
    output.push_str("jmp_buf _target;disp_ffi_panic_target=&_target;if(setjmp(_target)){disp_ffi_allocation_boundary_abort();disp_ffi_panic_target=NULL;return 1;}disp_ffi_allocation_boundary_begin();");
    if has_result {
        write!(
            output,
            "{} _result={}(",
            native_types::c_type(&result_ty),
            mono::mangle(program, instance)
        )
        .unwrap();
    } else {
        write!(output, "{}(", mono::mangle(program, instance)).unwrap();
    }
    for index in 0..function.argument_count {
        if index > 0 {
            output.push(',');
        }
        write!(output, "a{}", index + 1).unwrap();
    }
    output.push_str(");disp_ffi_allocation_boundary_finish();disp_ffi_panic_target=NULL;disp_ffi_last_error[0]=0;");
    if has_result {
        output.push_str("*out_result=_result;");
    }
    output.push_str("return 0;}\n");
}

fn json_codec_types(
    program: &mir::Program,
    instances: &mono::MonoProgram,
) -> (BTreeSet<hir::Type>, BTreeSet<hir::Type>) {
    let mut encoders = BTreeSet::new();
    let mut decoders = BTreeSet::new();
    for instance in &instances.instances {
        let function = &program.functions[instance.function.0];
        let mapping = mono::mapping(function, instance);
        for block in &function.blocks {
            if let mir::Terminator::Call {
                target: hir::CallTarget::Intrinsic(name),
                substitutions,
                ..
            } = &block.terminator
                && let Some(ty) = substitutions.first()
            {
                let ty = substitute(ty, &mapping);
                match name.as_str() {
                    "Json.from" => collect_json_codec_types(program, &ty, &mut encoders),
                    "Json.decode" => collect_json_codec_types(program, &ty, &mut decoders),
                    _ => {}
                }
            }
            if let mir::Terminator::Call {
                target: hir::CallTarget::Data(plan),
                arguments,
                ..
            } = &block.terminator
            {
                let plan = &program.data_plans[plan.0];
                match &plan.operation {
                    hir::DataOperation::Add { .. } => {
                        for field in &program.structs[plan.schema.0].fields {
                            collect_json_codec_types(program, &field.ty, &mut encoders);
                        }
                    }
                    hir::DataOperation::Find { .. } => {
                        for field in &program.structs[plan.schema.0].fields {
                            collect_json_codec_types(program, &field.ty, &mut decoders);
                        }
                    }
                    hir::DataOperation::Aggregate { .. } => {
                        for field in &program.structs[plan.schema.0].fields {
                            collect_json_codec_types(program, &field.ty, &mut decoders);
                        }
                    }
                    hir::DataOperation::Remove { .. } => {}
                }
                for argument in arguments.iter().skip(1) {
                    let ty = operand_ty(program, function, argument, &mapping);
                    let ty = match ty {
                        hir::Type::Reference { inner, .. } => *inner,
                        other => other,
                    };
                    if !matches!(&plan.operation, hir::DataOperation::Add { .. }) {
                        collect_json_codec_types(program, &ty, &mut encoders);
                    }
                }
            }
        }
    }
    (encoders, decoders)
}

fn collect_json_codec_types(
    program: &mir::Program,
    ty: &hir::Type,
    types: &mut BTreeSet<hir::Type>,
) {
    if !types.insert(ty.clone()) {
        return;
    }
    match ty {
        hir::Type::Array(element, _) | hir::Type::List(element) | hir::Type::Option(element) => {
            collect_json_codec_types(program, element, types)
        }
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            collect_json_codec_types(program, key, types);
            collect_json_codec_types(program, value, types);
        }
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            for field in &declaration.fields {
                collect_json_codec_types(program, &substitute(&field.ty, &substitutions), types);
            }
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            for variant in &declaration.variants {
                for payload in &variant.payload {
                    collect_json_codec_types(program, &substitute(payload, &substitutions), types);
                }
            }
        }
        _ => {}
    }
}

fn json_encoder_name(ty: &hir::Type) -> String {
    format!("disp_json_encode_{}", mono::type_code(ty))
}

fn json_decoder_name(ty: &hir::Type) -> String {
    format!("disp_json_decode_{}", mono::type_code(ty))
}

fn emit_json_codecs(
    program: &mir::Program,
    encoders: &BTreeSet<hir::Type>,
    decoders: &BTreeSet<hir::Type>,
    output: &mut String,
) {
    if !encoders.is_empty() || !decoders.is_empty() {
        output.push_str("static bool disp_json_codec_error(disp_native_string *error,const char *message){*error=disp_owned_bytes(message,strlen(message));return false;}\n");
    }
    for ty in encoders {
        writeln!(
            output,
            "static bool {}(const {} *value,disp_native_json *json,disp_native_string *error);",
            json_encoder_name(ty),
            native_types::c_type(ty)
        )
        .unwrap();
    }
    for ty in decoders {
        writeln!(
            output,
            "static bool {}(const disp_native_json *json,{} *value,disp_native_string *error);",
            json_decoder_name(ty),
            native_types::c_type(ty)
        )
        .unwrap();
    }
    for ty in encoders {
        emit_json_encoder(program, ty, output);
    }
    for ty in decoders {
        emit_json_decoder(program, ty, output);
    }
}

fn emit_json_encoder(program: &mir::Program, ty: &hir::Type, output: &mut String) {
    let name = json_encoder_name(ty);
    let c_ty = native_types::c_type(ty);
    write!(
        output,
        "static bool {name}(const {c_ty} *value,disp_native_json *json,disp_native_string *error){{"
    )
    .unwrap();
    match ty {
        hir::Type::Bool => output.push_str(
            "(void)error;*json=*value?disp_json_literal(\"true\",4):disp_json_literal(\"false\",5);return true;",
        ),
        hir::Type::Int { signed: true, .. } => output.push_str(
            "(void)error;*json=disp_json_from_i128((__int128)*value);return true;",
        ),
        hir::Type::Int { signed: false, .. } => output.push_str(
            "(void)error;*json=disp_json_from_u128((unsigned __int128)*value);return true;",
        ),
        hir::Type::Float { .. } => {
            output.push_str("return disp_json_from_f64((double)*value,json,error);")
        }
        hir::Type::String | hir::Type::Str => {
            output.push_str("return disp_json_from_string(value->data,value->len,json,error);")
        }
        hir::Type::Json => output.push_str(
            "(void)error;*json=disp_json_copy_range(value->data,value->data+value->len);return true;",
        ),
        hir::Type::Char => {
            output.push_str("return disp_json_from_char(*value,json,error);")
        }
        hir::Type::Unit => output.push_str(
            "(void)value;(void)error;*json=disp_json_literal(\"null\",4);return true;",
        ),
        hir::Type::Array(element, length) => emit_json_encode_sequence(
            element,
            "value->values",
            &length.to_string(),
            output,
        ),
        hir::Type::List(element) => {
            emit_json_encode_sequence(element, "value->data", "value->len", output)
        }
        hir::Type::Map(key, mapped) => {
            debug_assert!(matches!(key.as_ref(), hir::Type::String));
            let mapped_name = json_encoder_name(mapped);
            output.push_str("disp_native_json *_items=value->len?(disp_native_json*)disp_alloc_zeroed(value->len,sizeof(disp_native_json),_Alignof(disp_native_json)):NULL;size_t _done=0;for(size_t _i=0;_i<value->len;_i++){if(!");
            write!(output, "{mapped_name}(&value->values[_i],&_items[_i],error)").unwrap();
            output.push_str("){for(size_t _j=0;_j<_done;_j++)disp_json_drop(&_items[_j]);disp_dealloc(_items);return false;}_done++;}bool _ok=disp_json_from_object(value->keys,_items,value->len,json,error);for(size_t _i=0;_i<_done;_i++)disp_json_drop(&_items[_i]);disp_dealloc(_items);return _ok;");
        }
        hir::Type::Option(inner) => {
            let inner_name = json_encoder_name(inner);
            write!(
                output,
                "if(value->tag==0){{(void)error;*json=disp_json_literal(\"null\",4);return true;}}return {inner_name}(&value->payload.v1.f0,json,error);"
            )
            .unwrap();
        }
        hir::Type::Result(ok, error_ty) => {
            let ok_name = json_encoder_name(ok);
            let error_name = json_encoder_name(error_ty);
            write!(output,"disp_native_json _payload={{0}};const char *_key=NULL;size_t _key_len=0;bool _ok=false;if(value->tag==0){{_key=\"Ok\";_key_len=2;_ok={ok_name}(&value->payload.v0.f0,&_payload,error);}}else{{_key=\"Err\";_key_len=3;_ok={error_name}(&value->payload.v1.f0,&_payload,error);}}if(!_ok)return false;disp_native_string _name={{.data=(char*)_key,.len=_key_len,.cap=0}};_ok=disp_json_from_object(&_name,&_payload,1,json,error);disp_json_drop(&_payload);return _ok;").unwrap();
        }
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            let count = declaration.fields.len();
            if count == 0 {
                output.push_str("(void)value;return disp_json_from_object(NULL,NULL,0,json,error);");
            } else {
                write!(output, "disp_native_string _keys[{count}]={{").unwrap();
                for field in &declaration.fields {
                    write!(
                        output,
                        "{{.data=(char*)\"{}\",.len={},.cap=0}},",
                        escape(&field.name),
                        field.name.len()
                    )
                    .unwrap();
                }
                write!(output, "}};disp_native_json _items[{count}]={{{{0}}}};size_t _done=0;")
                    .unwrap();
                for field in &declaration.fields {
                    let field_ty = substitute(&field.ty, &substitutions);
                    write!(
                        output,
                        "if(!{}(&value->f{},&_items[{}],error))goto fail;_done++;",
                        json_encoder_name(&field_ty),
                        field.index,
                        field.index
                    )
                    .unwrap();
                }
                write!(output,"{{bool _ok=disp_json_from_object(_keys,_items,{count},json,error);for(size_t _i=0;_i<_done;_i++)disp_json_drop(&_items[_i]);return _ok;}}fail:for(size_t _i=0;_i<_done;_i++)disp_json_drop(&_items[_i]);return false;").unwrap();
            }
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            output.push_str("switch(value->tag){");
            for variant in &declaration.variants {
                write!(output, "case {}:", variant.index).unwrap();
                if variant.payload.is_empty() {
                    write!(
                        output,
                        "return disp_json_from_string(\"{}\",{},json,error);",
                        escape(&variant.name),
                        variant.name.len()
                    )
                    .unwrap();
                } else {
                    output.push_str("{disp_native_json _payload={0};bool _ok=false;");
                    if variant.payload.len() == 1 {
                        let payload_ty = substitute(&variant.payload[0], &substitutions);
                        write!(
                            output,
                            "_ok={}(&value->payload.v{}.f0,&_payload,error);",
                            json_encoder_name(&payload_ty),
                            variant.index
                        )
                        .unwrap();
                    } else {
                        let count = variant.payload.len();
                        write!(output, "disp_native_json _parts[{count}]={{{{0}}}};size_t _done=0;")
                            .unwrap();
                        for (index, payload) in variant.payload.iter().enumerate() {
                            let payload_ty = substitute(payload, &substitutions);
                            write!(output,"if(!{}(&value->payload.v{}.f{},&_parts[{}],error))goto variant_fail_{};_done++;",json_encoder_name(&payload_ty),variant.index,index,index,variant.index).unwrap();
                        }
                        write!(output,"_ok=disp_json_from_array(_parts,{count},&_payload,error);for(size_t _i=0;_i<_done;_i++)disp_json_drop(&_parts[_i]);if(!_ok)return false;goto variant_ready_{};variant_fail_{}:for(size_t _i=0;_i<_done;_i++)disp_json_drop(&_parts[_i]);return false;variant_ready_{}:;",variant.index,variant.index,variant.index).unwrap();
                    }
                    write!(output,"if(!_ok)return false;disp_native_string _key={{.data=(char*)\"{}\",.len={},.cap=0}};_ok=disp_json_from_object(&_key,&_payload,1,json,error);disp_json_drop(&_payload);return _ok;}}",escape(&variant.name),variant.name.len()).unwrap();
                }
            }
            output.push_str("default:return false;}");
        }
        _ => output.push_str("(void)value;(void)json;const char *_message=\"unsupported automatic JSON encoder\";*error=disp_owned_bytes(_message,strlen(_message));return false;"),
    }
    output.push_str("}\n");
}

fn emit_json_encode_sequence(element: &hir::Type, data: &str, length: &str, output: &mut String) {
    let encode = json_encoder_name(element);
    write!(output,"size_t _length=(size_t)({length});disp_native_json *_items=_length?(disp_native_json*)disp_alloc_zeroed(_length,sizeof(disp_native_json),_Alignof(disp_native_json)):NULL;size_t _done=0;for(size_t _i=0;_i<_length;_i++){{if(!{encode}(&({data})[_i],&_items[_i],error)){{for(size_t _j=0;_j<_done;_j++)disp_json_drop(&_items[_j]);disp_dealloc(_items);return false;}}_done++;}}bool _ok=disp_json_from_array(_items,_length,json,error);for(size_t _i=0;_i<_done;_i++)disp_json_drop(&_items[_i]);disp_dealloc(_items);return _ok;").unwrap();
}

fn emit_json_decoder(program: &mir::Program, ty: &hir::Type, output: &mut String) {
    let name = json_decoder_name(ty);
    let c_ty = native_types::c_type(ty);
    write!(output,"static bool {name}(const disp_native_json *json,{c_ty} *value,disp_native_string *error){{*value=({c_ty}){{0}};").unwrap();
    match ty {
        hir::Type::Bool => output.push_str("return disp_json_as_bool(json,value,error);"),
        hir::Type::Int { signed: true, width } => {
            let bits = width.unwrap_or(64);
            write!(output,"__int128 _parsed=0;if(!disp_json_as_i128(json,&_parsed,error))return false;").unwrap();
            if bits < 128 {
                write!(output,"{{__int128 _minimum=-((__int128)1<<{}),_maximum=((__int128)1<<{})-1;if(_parsed<_minimum||_parsed>_maximum)return disp_json_codec_error(error,\"JSON integer is outside the destination type range\");}}",bits-1,bits-1).unwrap();
            }
            output.push_str("*value=(_Bool)0+(");
            write!(output, "{c_ty}").unwrap();
            output.push_str(")_parsed;return true;");
        }
        hir::Type::Int {
            signed: false,
            width,
        } => {
            let bits = width.unwrap_or(64);
            output.push_str("unsigned __int128 _parsed=0;if(!disp_json_as_u128(json,&_parsed,error))return false;");
            if bits < 128 {
                write!(output,"if(_parsed>(((unsigned __int128)1<<{bits})-1))return disp_json_codec_error(error,\"JSON integer is outside the destination type range\");").unwrap();
            }
            write!(output, "*value=({c_ty})_parsed;return true;").unwrap();
        }
        hir::Type::Float { width } => {
            output.push_str("double _parsed=0;if(!disp_json_as_f64(json,&_parsed,error))return false;");
            if *width == 32 {
                output.push_str("if(!isfinite((float)_parsed))return disp_json_codec_error(error,\"JSON number is outside the f32 range\");");
            }
            write!(output, "*value=({c_ty})_parsed;return true;").unwrap();
        }
        hir::Type::String => output.push_str("return disp_json_as_text(json,value,error);"),
        hir::Type::Json => output.push_str("(void)error;*value=disp_json_copy_range(json->data,json->data+json->len);return true;"),
        hir::Type::Char => output.push_str("return disp_json_as_char(json,value,error);"),
        hir::Type::Unit => output.push_str("if(!disp_json_is_kind(json,\"null\"))return disp_json_codec_error(error,\"expected JSON null\");return true;"),
        hir::Type::Array(element, length) => {
            write!(output,"size_t _length=0;if(!disp_json_collection_len(json,&_length)||_length!={length})return disp_json_codec_error(error,\"JSON array length does not match the fixed array type\");size_t _done=0;for(size_t _i=0;_i<_length;_i++){{disp_native_json _item={{0}};if(!disp_json_at(json,_i,&_item))goto fail;if(!{}(&_item,&value->values[_i],error)){{disp_json_drop(&_item);goto fail;}}disp_json_drop(&_item);_done++;}}return true;fail:for(size_t _i=0;_i<_done;_i++){{{}}}return false;",json_decoder_name(element),drop_value(program,"value->values[_i]",element)).unwrap();
        }
        hir::Type::List(element) => {
            let element_c = native_types::c_type(element);
            write!(output,"size_t _length=0;if(!disp_json_collection_len(json,&_length)||strcmp(disp_json_kind_name(json),\"array\"))return disp_json_codec_error(error,\"expected JSON array\");if(_length)value->data=({element_c}*)disp_alloc_zeroed(_length,sizeof({element_c}),_Alignof({element_c}));value->cap=_length;for(size_t _i=0;_i<_length;_i++){{disp_native_json _item={{0}};if(!disp_json_at(json,_i,&_item))goto fail;if(!{}(&_item,&value->data[_i],error)){{disp_json_drop(&_item);goto fail;}}disp_json_drop(&_item);value->len++;}}return true;fail:for(size_t _i=0;_i<value->len;_i++){{{}}}disp_dealloc(value->data);*value=({c_ty}){{0}};return false;",json_decoder_name(element),drop_value(program,"value->data[_i]",element)).unwrap();
        }
        hir::Type::Map(key, mapped) => {
            debug_assert!(matches!(key.as_ref(), hir::Type::String));
            let mapped_c = native_types::c_type(mapped);
            write!(output,"size_t _length=0;if(!disp_json_collection_len(json,&_length)||strcmp(disp_json_kind_name(json),\"object\"))return disp_json_codec_error(error,\"expected JSON object\");if(_length){{value->keys=(disp_native_string*)disp_alloc_zeroed(_length,sizeof(disp_native_string),_Alignof(disp_native_string));value->values=({mapped_c}*)disp_alloc_zeroed(_length,sizeof({mapped_c}),_Alignof({mapped_c}));value->states=(uint8_t*)disp_alloc_zeroed(_length,1,1);value->cap=_length;}}for(size_t _i=0;_i<_length;_i++){{disp_native_json _item={{0}};if(!disp_json_object_entry_at(json,_i,&value->keys[_i],&_item))goto fail;if(!{}(&_item,&value->values[_i],error)){{disp_json_drop(&_item);disp_string_drop(&value->keys[_i]);goto fail;}}disp_json_drop(&_item);value->len++;}}return true;fail:for(size_t _i=0;_i<value->len;_i++){{disp_string_drop(&value->keys[_i]);{}}}disp_dealloc(value->keys);disp_dealloc(value->values);disp_dealloc(value->states);*value=({c_ty}){{0}};return false;",json_decoder_name(mapped),drop_value(program,"value->values[_i]",mapped)).unwrap();
        }
        hir::Type::Option(inner) => {
            write!(output,"if(disp_json_is_kind(json,\"null\")){{value->tag=0;return true;}}value->tag=1;if(!{}(json,&value->payload.v1.f0,error)){{*value=({c_ty}){{0}};return false;}}return true;",json_decoder_name(inner)).unwrap();
        }
        hir::Type::Result(ok, error_ty) => {
            write!(output,"size_t _length=0;if(!disp_json_collection_len(json,&_length)||_length!=1||strcmp(disp_json_kind_name(json),\"object\"))return disp_json_codec_error(error,\"JSON Result must contain exactly one Ok or Err member\");disp_native_json _payload={{0}};if(disp_json_get(json,\"Ok\",2,&_payload)){{value->tag=0;bool _ok={}(&_payload,&value->payload.v0.f0,error);disp_json_drop(&_payload);if(!_ok)*value=({c_ty}){{0}};return _ok;}}if(disp_json_get(json,\"Err\",3,&_payload)){{value->tag=1;bool _ok={}(&_payload,&value->payload.v1.f0,error);disp_json_drop(&_payload);if(!_ok)*value=({c_ty}){{0}};return _ok;}}return disp_json_codec_error(error,\"JSON Result member must be named Ok or Err\");",json_decoder_name(ok),json_decoder_name(error_ty)).unwrap();
        }
        hir::Type::Struct(id, arguments) => emit_json_decode_struct(program, *id, arguments, c_ty.as_str(), output),
        hir::Type::Enum(id, arguments) => emit_json_decode_enum(program, *id, arguments, c_ty.as_str(), output),
        _ => output.push_str("(void)json;return disp_json_codec_error(error,\"unsupported automatic JSON decoder\");"),
    }
    output.push_str("}\n");
}

fn emit_json_decode_struct(
    program: &mir::Program,
    id: hir::StructId,
    arguments: &[hir::Type],
    c_ty: &str,
    output: &mut String,
) {
    let declaration = &program.structs[id.0];
    let substitutions = declaration
        .generic_parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<HashMap<_, _>>();
    write!(output,"size_t _length=0;if(!disp_json_collection_len(json,&_length)||strcmp(disp_json_kind_name(json),\"object\")||_length!={})return disp_json_codec_error(error,\"JSON object does not exactly match struct {}\");size_t _done=0;disp_native_json _field={{0}};",declaration.fields.len(),escape(&declaration.name)).unwrap();
    for field in &declaration.fields {
        let field_ty = substitute(&field.ty, &substitutions);
        write!(output,"if(!disp_json_get(json,\"{}\",{},&_field)){{disp_json_codec_error(error,\"JSON object is missing field {}\");goto fail;}}if(!{}(&_field,&value->f{},error)){{disp_json_drop(&_field);goto fail;}}disp_json_drop(&_field);_done++;",escape(&field.name),field.name.len(),escape(&field.name),json_decoder_name(&field_ty),field.index).unwrap();
    }
    output.push_str("return true;fail:switch(_done){");
    for done in (1..=declaration.fields.len()).rev() {
        let field = &declaration.fields[done - 1];
        let field_ty = substitute(&field.ty, &substitutions);
        write!(
            output,
            "case {done}:{}",
            drop_value(program, &format!("value->f{}", field.index), &field_ty)
        )
        .unwrap();
    }
    write!(output, "default:break;}}*value=({c_ty}){{0}};return false;").unwrap();
}

fn emit_json_decode_enum(
    program: &mir::Program,
    id: hir::EnumId,
    arguments: &[hir::Type],
    c_ty: &str,
    output: &mut String,
) {
    let declaration = &program.enums[id.0];
    let substitutions = declaration
        .generic_parameters
        .iter()
        .cloned()
        .zip(arguments.iter().cloned())
        .collect::<HashMap<_, _>>();
    output.push_str("if(disp_json_is_kind(json,\"string\")){disp_native_string _name={0};if(!disp_json_as_text(json,&_name,error))return false;");
    for variant in declaration
        .variants
        .iter()
        .filter(|variant| variant.payload.is_empty())
    {
        write!(output,"if(_name.len=={}&&!memcmp(_name.data,\"{}\",{})){{disp_string_drop(&_name);value->tag={};return true;}}",variant.name.len(),escape(&variant.name),variant.name.len(),variant.index).unwrap();
    }
    write!(output,"disp_string_drop(&_name);return disp_json_codec_error(error,\"unknown unit variant for enum {}\");}}size_t _length=0;if(!disp_json_collection_len(json,&_length)||_length!=1||strcmp(disp_json_kind_name(json),\"object\"))return disp_json_codec_error(error,\"JSON enum must contain exactly one variant member\");disp_native_json _payload={{0}};",escape(&declaration.name)).unwrap();
    for variant in declaration
        .variants
        .iter()
        .filter(|variant| !variant.payload.is_empty())
    {
        write!(
            output,
            "if(disp_json_get(json,\"{}\",{},&_payload)){{value->tag={};",
            escape(&variant.name),
            variant.name.len(),
            variant.index
        )
        .unwrap();
        if variant.payload.len() == 1 {
            let payload_ty = substitute(&variant.payload[0], &substitutions);
            write!(output,"bool _ok={}(&_payload,&value->payload.v{}.f0,error);disp_json_drop(&_payload);if(!_ok)*value=({c_ty}){{0}};return _ok;",json_decoder_name(&payload_ty),variant.index).unwrap();
        } else {
            write!(output,"size_t _parts=0;if(!disp_json_collection_len(&_payload,&_parts)||_parts!={}){{disp_json_drop(&_payload);*value=({c_ty}){{0}};return disp_json_codec_error(error,\"JSON enum payload has the wrong length\");}}size_t _done=0;",variant.payload.len()).unwrap();
            for (index, payload) in variant.payload.iter().enumerate() {
                let payload_ty = substitute(payload, &substitutions);
                write!(output,"{{disp_native_json _part={{0}};if(!disp_json_at(&_payload,{index},&_part))goto enum_fail_{};if(!{}(&_part,&value->payload.v{}.f{index},error)){{disp_json_drop(&_part);goto enum_fail_{};}}disp_json_drop(&_part);_done++;}}",variant.index,json_decoder_name(&payload_ty),variant.index,variant.index).unwrap();
            }
            output.push_str("disp_json_drop(&_payload);return true;");
            write!(
                output,
                "enum_fail_{}:disp_json_drop(&_payload);switch(_done){{",
                variant.index
            )
            .unwrap();
            for done in (1..=variant.payload.len()).rev() {
                let payload_ty = substitute(&variant.payload[done - 1], &substitutions);
                write!(
                    output,
                    "case {done}:{}",
                    drop_value(
                        program,
                        &format!("value->payload.v{}.f{}", variant.index, done - 1),
                        &payload_ty
                    )
                )
                .unwrap();
            }
            write!(output, "default:break;}}*value=({c_ty}){{0}};return false;").unwrap();
        }
        output.push('}');
    }
    write!(
        output,
        "return disp_json_codec_error(error,\"unknown payload variant for enum {}\");",
        escape(&declaration.name)
    )
    .unwrap();
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

fn task_result_types(program: &mir::Program, instances: &mono::MonoProgram) -> BTreeSet<hir::Type> {
    let mut results = BTreeSet::new();
    for instance in &instances.instances {
        let function = &program.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        for local in &function.locals {
            if let hir::Type::Task(result) = substitute(&local.ty, &substitutions) {
                results.insert(*result);
            }
        }
    }
    results
}

fn task_result_drop_name(result: &hir::Type) -> String {
    format!("disp_task_result_drop_{}", mono::type_code(result))
}

fn task_result_drop_wrapper(program: &mir::Program, result: &hir::Type, output: &mut String) {
    let result_c = native_types::c_type(result);
    let drop = drop_value(program, &format!("*({result_c}*)raw"), result);
    writeln!(
        output,
        "static void {}(void *raw){{(void)raw;{drop}}}",
        task_result_drop_name(result)
    )
    .unwrap();
}

fn async_operations(
    program: &mir::Program,
    instances: &mono::MonoProgram,
) -> BTreeSet<(String, hir::Type)> {
    let mut operations = BTreeSet::new();
    for instance in &instances.instances {
        let function = &program.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        for block in &function.blocks {
            if let mir::Terminator::Call {
                target: hir::CallTarget::Intrinsic(name),
                destination,
                ..
            } = &block.terminator
                && matches!(
                    name.as_str(),
                    "Async.read_text"
                        | "Async.read_bytes"
                        | "Async.write_text"
                        | "Async.write_bytes"
                        | "Async.connect"
                        | "Async.connect_timeout"
                        | "Async.resolve"
                        | "Async.resolve_timeout"
                        | "Tls.connect"
                        | "Tls.connect_timeout"
                        | "Http.get"
                        | "Http.get_timeout"
                        | "Http.post"
                        | "Http.post_timeout"
                        | "Http.post_json"
                        | "Http.post_json_timeout"
                        | "Http.put"
                        | "Http.put_timeout"
                        | "Http.patch"
                        | "Http.patch_timeout"
                        | "Http.delete"
                        | "Http.delete_timeout"
                        | "HttpRequest.send"
                        | "HttpRequest.send_timeout"
                        | "TcpListener.accept"
                        | "TcpListener.accept_timeout"
                        | "TcpStream.read_async"
                        | "TcpStream.read_async_timeout"
                        | "TcpStream.write_async"
                        | "TcpStream.write_async_timeout"
                        | "TlsStream.read_async"
                        | "TlsStream.read_async_timeout"
                        | "TlsStream.write_async"
                        | "TlsStream.write_async_timeout"
                        | "UdpSocket.receive_from_async"
                        | "UdpSocket.receive_from_async_timeout"
                        | "UdpSocket.send_to_async"
                        | "UdpSocket.send_to_async_timeout"
                )
            {
                let hir::Type::Future(result) =
                    place_ty(program, function, destination, &substitutions)
                else {
                    continue;
                };
                operations.insert((name.clone(), *result));
            }
        }
    }
    operations
}

fn async_poll_name(operation: &str, result: &hir::Type) -> String {
    format!(
        "disp_{}_poll_{}",
        operation.replace('.', "_"),
        mono::type_code(result)
    )
}

fn async_poll_wrapper(operation: &str, result: &hir::Type, output: &mut String) {
    let result_c = native_types::c_type(result);
    let poll = async_poll_name(operation, result);
    if matches!(operation, "Async.connect" | "Async.connect_timeout") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_connect_state *state=(disp_connect_state*)raw;if(!disp_connect_poll(state))return false;bool ok=false;disp_native_tcp_stream stream={{0}};disp_native_string error={{0}};disp_connect_take(state,&ok,&stream,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=stream;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if matches!(operation, "Async.resolve" | "Async.resolve_timeout") {
        let hir::Type::Result(value, _) = result else {
            unreachable!("DNS future must contain Result")
        };
        let list_c = native_types::c_type(value);
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_dns_state *state=(disp_dns_state*)raw;if(!disp_dns_poll(state))return false;bool ok=false;disp_native_ip_list addresses={{0}};disp_native_string error={{0}};disp_dns_take(state,&ok,&addresses,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=({list_c}){{.data=addresses.data,.len=addresses.len,.cap=addresses.cap}};}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if matches!(operation, "Tls.connect" | "Tls.connect_timeout") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_tls_handshake_state *state=(disp_tls_handshake_state*)raw;if(!disp_tls_handshake_poll(state))return false;bool ok=false;disp_native_tls_stream stream={{0}};disp_native_string error={{0}};disp_tls_handshake_take(state,&ok,&stream,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=stream;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if operation.starts_with("Http.") || operation.starts_with("HttpRequest.send") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_http_request_state *state=(disp_http_request_state*)raw;if(!disp_http_request_poll(state))return false;bool ok=false;disp_native_http_response response={{0}};disp_native_string error={{0}};disp_http_request_take(state,&ok,&response,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=response;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if operation.starts_with("TcpStream.read_async") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_socket_io_state *state=(disp_socket_io_state*)raw;if(!disp_socket_io_poll(state))return false;bool ok=false;size_t written=0;disp_native_string bytes={{0}},error={{0}};disp_socket_io_take(state,&ok,&bytes,&written,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=({}){{.data=(uint8_t*)bytes.data,.len=bytes.len,.cap=bytes.cap}};}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}",
            match result {
                hir::Type::Result(value, _) => native_types::c_type(value),
                _ => unreachable!("TCP read future must contain Result"),
            }
        )
        .unwrap();
        return;
    }
    if operation.starts_with("TcpStream.write_async") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_socket_io_state *state=(disp_socket_io_state*)raw;if(!disp_socket_io_poll(state))return false;bool ok=false;size_t written=0;disp_native_string bytes={{0}},error={{0}};disp_socket_io_take(state,&ok,&bytes,&written,&error);disp_string_drop(&bytes);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=(uint64_t)written;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if operation.starts_with("TlsStream.read_async") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_tls_io_state *state=(disp_tls_io_state*)raw;if(!disp_tls_io_poll(state))return false;bool ok=false;size_t written=0;disp_native_string bytes={{0}},error={{0}};disp_tls_io_take(state,&ok,&bytes,&written,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=({}){{.data=(uint8_t*)bytes.data,.len=bytes.len,.cap=bytes.cap}};}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}",
            match result {
                hir::Type::Result(value, _) => native_types::c_type(value),
                _ => unreachable!("TLS read future must contain Result"),
            }
        )
        .unwrap();
        return;
    }
    if operation.starts_with("TlsStream.write_async") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_tls_io_state *state=(disp_tls_io_state*)raw;if(!disp_tls_io_poll(state))return false;bool ok=false;size_t written=0;disp_native_string bytes={{0}},error={{0}};disp_tls_io_take(state,&ok,&bytes,&written,&error);disp_string_drop(&bytes);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=(uint64_t)written;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if operation.starts_with("UdpSocket.receive_from_async") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_udp_io_state *state=(disp_udp_io_state*)raw;if(!disp_udp_io_poll(state))return false;bool ok=false;size_t sent=0;disp_native_udp_datagram datagram={{0}};disp_native_string error={{0}};disp_udp_io_take(state,&ok,&datagram,&sent,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=datagram;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if operation.starts_with("UdpSocket.send_to_async") {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_udp_io_state *state=(disp_udp_io_state*)raw;if(!disp_udp_io_poll(state))return false;bool ok=false;size_t sent=0;disp_native_udp_datagram datagram={{0}};disp_native_string error={{0}};disp_udp_io_take(state,&ok,&datagram,&sent,&error);disp_udp_datagram_drop(&datagram);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=(uint64_t)sent;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    if matches!(
        operation,
        "TcpListener.accept" | "TcpListener.accept_timeout"
    ) {
        writeln!(
            output,
            "static bool {poll}(void *raw,void *output){{disp_accept_state *state=(disp_accept_state*)raw;if(!disp_accept_poll(state))return false;bool ok=false;disp_native_tcp_stream stream={{0}};disp_native_string error={{0}};disp_accept_take(state,&ok,&stream,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{result->tag=0;result->payload.v0.f0=stream;}}else{{result->tag=1;result->payload.v1.f0=error;}}return true;}}"
        )
        .unwrap();
        return;
    }
    writeln!(
        output,
        "static bool {poll}(void *raw,void *output){{disp_async_file_state *state=(disp_async_file_state*)raw;if(!disp_async_file_poll(state))return false;bool ok=false;disp_native_string value={{0}},error={{0}};disp_async_file_take(state,&ok,&value,&error);{result_c} *result=({result_c}*)output;*result=({result_c}){{0}};if(ok){{"
    )
    .unwrap();
    match operation {
        "Async.read_text" => output.push_str("result->tag=0;result->payload.v0.f0=value;"),
        "Async.read_bytes" => {
            let hir::Type::Result(value, _) = result else {
                unreachable!()
            };
            let list_c = native_types::c_type(value);
            write!(output,"result->tag=0;result->payload.v0.f0=({list_c}){{.data=(uint8_t*)value.data,.len=value.len,.cap=value.cap}};").unwrap();
        }
        "Async.write_text" | "Async.write_bytes" => {
            output.push_str("result->tag=0;disp_string_drop(&value);")
        }
        _ => unreachable!(),
    }
    output.push_str(
        "}else{result->tag=1;result->payload.v1.f0=error;disp_string_drop(&value);}return true;}\n",
    );
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

fn atomic_operation(name: &str) -> Option<&'static str> {
    if name == "load" || name.starts_with("load_") {
        Some("load")
    } else if name == "store" || name.starts_with("store_") {
        Some("store")
    } else if name == "add" || name.starts_with("add_") {
        Some("add")
    } else if name == "fetch_add" || name.starts_with("fetch_add_") {
        Some("fetch_add")
    } else {
        None
    }
}

fn atomic_c_order(name: &str) -> &'static str {
    if name.ends_with("_relaxed") {
        "memory_order_relaxed"
    } else if name.ends_with("_acquire") {
        "memory_order_acquire"
    } else if name.ends_with("_release") {
        "memory_order_release"
    } else if name.ends_with("_acq_rel") {
        "memory_order_acq_rel"
    } else {
        "memory_order_seq_cst"
    }
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
        | hir::Type::SecretBytes
        | hir::Type::AeadEnvelope
        | hir::Type::Ed25519SigningKey
        | hir::Type::Path
        | hir::Type::Url
        | hir::Type::Json
        | hir::Type::IpAddress
        | hir::Type::SocketAddress
        | hir::Type::TcpStream
        | hir::Type::TlsStream
        | hir::Type::HttpRequest
        | hir::Type::HttpResponse
        | hir::Type::TcpListener
        | hir::Type::UdpSocket
        | hir::Type::UdpDatagram
        | hir::Type::Instant
        | hir::Type::Duration
        | hir::Type::ProcessCommand
        | hir::Type::ChildProcess
        | hir::Type::ProcessOutput
        | hir::Type::Database
        | hir::Type::DataStore
        | hir::Type::CRegistration
        | hir::Type::Int { .. }
        | hir::Type::Float { .. } => true,
        hir::Type::AtomicInt => true,
        hir::Type::Thread(result) => supported(program, result),
        hir::Type::Future(result) | hir::Type::Task(result) => supported(program, result),
        hir::Type::Mutex(value) | hir::Type::MutexGuard(value) | hir::Type::Channel(value) => {
            supported(program, value)
        }
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
        hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. }
        | hir::Type::MemoryPointer { inner, .. } => supported(program, inner),
        hir::Type::Function(arguments, result) | hir::Type::CFunction(arguments, result) => {
            arguments
                .iter()
                .all(|argument| supported(program, argument))
                && supported(program, result)
        }
        hir::Type::Generic(name) => {
            matches!(
                name.as_str(),
                "ConversionError"
                    | "IoError"
                    | "NetworkError"
                    | "HttpError"
                    | "DataError"
                    | "CryptoError"
            )
        }
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
    output.push_str("){\ndisp_runtime_enter_call();\n");
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
        output.push_str("disp_runtime_charge_steps(1);\n");
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
        output.push_str("disp_runtime_charge_steps(1);\n");
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
                hir::CallTarget::Data(plan) => data_call(
                    program,
                    function,
                    *plan,
                    arguments,
                    &destination_ty,
                    substitutions,
                ),
                hir::CallTarget::Intrinsic(name) if name == "Async.yield" => {
                    "disp_future_yield()".into()
                }
                hir::CallTarget::Intrinsic(name) if name == "Async.spawn" => {
                    let future_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let future =
                        operand(program, function, &arguments[0], &future_ty, substitutions);
                    let hir::Type::Task(result) = &destination_ty else {
                        unreachable!("Async.spawn destination must be Task<T>")
                    };
                    let result_c = native_types::c_type(result);
                    format!(
                        "disp_task_spawn({future},sizeof({result_c}),_Alignof({result_c}),{})",
                        task_result_drop_name(result)
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Async.sleep" => {
                    let duration_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let duration = operand(
                        program,
                        function,
                        &arguments[0],
                        &duration_ty,
                        substitutions,
                    );
                    format!("disp_future_sleep({duration})")
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "MemoryPointer.offset" | "MemoryPointer.read" | "MemoryPointer.write"
                    ) =>
                {
                    let pointer_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let pointer =
                        operand(program, function, &arguments[0], &pointer_ty, substitutions);
                    let hir::Type::MemoryPointer { inner, .. } = pointer_ty else {
                        unreachable!()
                    };
                    let element_c = native_types::c_type(&inner);
                    match name.as_str() {
                        "MemoryPointer.offset" => {
                            let offset_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let offset = operand(
                                program,
                                function,
                                &arguments[1],
                                &offset_ty,
                                substitutions,
                            );
                            format!(
                                "disp_memory_pointer_offset({pointer},(int64_t)({offset}),{},{})",
                                span.start.line, span.start.column
                            )
                        }
                        "MemoryPointer.read" => format!(
                            "(*({element_c}*)disp_memory_pointer_access({pointer},sizeof({element_c}),_Alignof({element_c}),{},{}))",
                            span.start.line, span.start.column
                        ),
                        "MemoryPointer.write" => {
                            let value_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let value =
                                operand(program, function, &arguments[1], &value_ty, substitutions);
                            format!(
                                "((*({element_c}*)disp_memory_pointer_access({pointer},sizeof({element_c}),_Alignof({element_c}),{},{})={value}),(disp_native_unit){{0}})",
                                span.start.line, span.start.column
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "Async.connect" | "Async.connect_timeout") =>
                {
                    let hir::Type::Future(result) = &destination_ty else {
                        unreachable!("Async.connect destination must be Future<T>")
                    };
                    let address_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let address =
                        operand(program, function, &arguments[0], &address_ty, substitutions);
                    let (has_timeout, timeout) = if arguments.len() == 2 {
                        let timeout_ty =
                            operand_ty(program, function, &arguments[1], substitutions);
                        let timeout =
                            operand(program, function, &arguments[1], &timeout_ty, substitutions);
                        ("true", format!("({timeout}).nanos"))
                    } else {
                        ("false", "0".into())
                    };
                    let poll = async_poll_name(name, result);
                    format!(
                        "({{disp_connect_state *_state=disp_connect_create({address},{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_connect_drop}};}})",
                        span.start.line, span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "Async.resolve" | "Async.resolve_timeout") =>
                {
                    let hir::Type::Future(result) = &destination_ty else {
                        unreachable!("Async.resolve destination must be Future<T>")
                    };
                    let (host, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (has_timeout, timeout) = if arguments.len() == 2 {
                        let timeout_ty =
                            operand_ty(program, function, &arguments[1], substitutions);
                        let timeout =
                            operand(program, function, &arguments[1], &timeout_ty, substitutions);
                        ("true", format!("({timeout}).nanos"))
                    } else {
                        ("false", "0".into())
                    };
                    let poll = async_poll_name(name, result);
                    format!(
                        "({{disp_dns_state *_state=disp_dns_create(({host})->data,({host})->len,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_dns_drop}};}})",
                        span.start.line, span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "Tls.connect" | "Tls.connect_timeout") =>
                {
                    let hir::Type::Future(result) = &destination_ty else {
                        unreachable!("Tls.connect destination must be Future<T>")
                    };
                    let stream_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let stream =
                        operand(program, function, &arguments[0], &stream_ty, substitutions);
                    let (server_name, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let (has_timeout, timeout) = if arguments.len() == 3 {
                        let timeout_ty =
                            operand_ty(program, function, &arguments[2], substitutions);
                        let timeout =
                            operand(program, function, &arguments[2], &timeout_ty, substitutions);
                        ("true", format!("({timeout}).nanos"))
                    } else {
                        ("false", "0".into())
                    };
                    let poll = async_poll_name(name, result);
                    format!(
                        "({{disp_native_tcp_stream _tcp={stream};disp_tls_handshake_state *_state=disp_tls_handshake_create(_tcp.state,({server_name})->data,({server_name})->len,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_tls_handshake_drop}};}})",
                        span.start.line, span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Http.request" => {
                    let result_c = native_types::c_type(&destination_ty);
                    let (method, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (url, _) = system_argument(program, function, &arguments[1], substitutions);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_http_request _request={{0}};disp_native_string _error={{0}};if(disp_http_builder_create(({method})->data,({method})->len,({url})->data,({url})->len,&_request,&_error)){{_r.tag=0;_r.payload.v0.f0=_request;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Http.get"
                            | "Http.get_timeout"
                            | "Http.post"
                            | "Http.post_timeout"
                            | "Http.post_json"
                            | "Http.post_json_timeout"
                            | "Http.put"
                            | "Http.put_timeout"
                            | "Http.patch"
                            | "Http.patch_timeout"
                            | "Http.delete"
                            | "Http.delete_timeout"
                    ) =>
                {
                    let hir::Type::Future(result) = &destination_ty else {
                        unreachable!("HTTP operation destination must be Future<T>")
                    };
                    let (url, _) = system_argument(program, function, &arguments[0], substitutions);
                    let method = if name.starts_with("Http.post") {
                        "POST"
                    } else if name.starts_with("Http.put") {
                        "PUT"
                    } else if name.starts_with("Http.patch") {
                        "PATCH"
                    } else if name.starts_with("Http.delete") {
                        "DELETE"
                    } else {
                        "GET"
                    };
                    let has_body = matches!(method, "POST" | "PUT" | "PATCH");
                    let (body_data, body_len, headers, headers_len) = if has_body {
                        let body_ty = operand_ty(program, function, &arguments[1], substitutions);
                        let (body, _) =
                            system_argument(program, function, &arguments[1], substitutions);
                        let text = matches!(body_ty, hir::Type::String | hir::Type::Str);
                        let json = matches!(body_ty, hir::Type::Json);
                        (
                            format!("(const char*)({body})->data"),
                            format!("({body})->len"),
                            if json {
                                "\"Content-Type: application/json\\r\\n\""
                            } else if text {
                                "\"Content-Type: text/plain; charset=utf-8\\r\\n\""
                            } else {
                                "NULL"
                            },
                            if json {
                                "32"
                            } else if text {
                                "41"
                            } else {
                                "0"
                            },
                        )
                    } else {
                        ("NULL".into(), "0".into(), "NULL", "0")
                    };
                    let timeout_index = usize::from(!has_body);
                    let timeout = if name.ends_with("_timeout") {
                        let timeout_index = if has_body { 2 } else { timeout_index };
                        let timeout_ty =
                            operand_ty(program, function, &arguments[timeout_index], substitutions);
                        let timeout = operand(
                            program,
                            function,
                            &arguments[timeout_index],
                            &timeout_ty,
                            substitutions,
                        );
                        format!("({timeout}).nanos")
                    } else {
                        "30000000000ULL".into()
                    };
                    let poll = async_poll_name(name, result);
                    format!(
                        "({{disp_http_request_state *_state=disp_http_request_create(\"{method}\",{},({url})->data,({url})->len,{headers},{headers_len},{body_data},{body_len},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_http_request_drop}};}})",
                        method.len(),
                        span.start.line,
                        span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Async.read_text"
                            | "Async.read_bytes"
                            | "Async.write_text"
                            | "Async.write_bytes"
                    ) =>
                {
                    let hir::Type::Future(result) = &destination_ty else {
                        unreachable!("async I/O destination must be Future<T>")
                    };
                    let path_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let path = operand(program, function, &arguments[0], &path_ty, substitutions);
                    let operation = match name.as_str() {
                        "Async.read_text" => "DISP_ASYNC_READ_TEXT",
                        "Async.read_bytes" => "DISP_ASYNC_READ_BYTES",
                        "Async.write_text" => "DISP_ASYNC_WRITE_TEXT",
                        "Async.write_bytes" => "DISP_ASYNC_WRITE_BYTES",
                        _ => unreachable!(),
                    };
                    let poll = async_poll_name(name, result);
                    let input = if matches!(name.as_str(), "Async.write_text" | "Async.write_bytes")
                    {
                        let input_ty = operand_ty(program, function, &arguments[1], substitutions);
                        let input =
                            operand(program, function, &arguments[1], &input_ty, substitutions);
                        if name == "Async.write_text" {
                            format!("disp_native_string _input={input};")
                        } else {
                            let input_c = native_types::c_type(&input_ty);
                            format!(
                                "{input_c} _bytes={input};disp_native_string _input={{.data=(char*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};"
                            )
                        }
                    } else {
                        "disp_native_string _input={0};".into()
                    };
                    format!(
                        "({{{input}disp_async_file_state *_state=disp_async_file_create({operation},{path},_input,{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_async_file_drop}};}})",
                        span.start.line, span.start.column
                    )
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
                hir::CallTarget::ForeignCallable => {
                    let callback_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let hir::Type::CFunction(parameters, result) = &callback_ty else {
                        unreachable!("validated foreign callback must have CFunction type")
                    };
                    let callback = operand(
                        program,
                        function,
                        &arguments[0],
                        &callback_ty,
                        substitutions,
                    );
                    let values = arguments[1..]
                        .iter()
                        .zip(parameters)
                        .map(|(argument, expected)| {
                            operand(program, function, argument, expected, substitutions)
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    if matches!(**result, hir::Type::Unit) {
                        format!(
                            "({{if(!({callback}))dv_panic(\"null C callback\",{},{}) ;({callback})({values});(disp_native_unit){{0}};}})",
                            span.start.line, span.start.column
                        )
                    } else {
                        let result_c = native_types::c_type(result);
                        format!(
                            "({{if(!({callback}))dv_panic(\"null C callback\",{},{}) ;{result_c} _callback_result=({callback})({values});_callback_result;}})",
                            span.start.line, span.start.column
                        )
                    }
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
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "Task.cancel" | "Task.is_finished") =>
                {
                    let task_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let task = operand(program, function, &arguments[0], &task_ty, substitutions);
                    if name == "Task.cancel" {
                        format!(
                            "({{disp_native_task _task={task};disp_task_cancel(&_task);(disp_native_unit){{0}};}})"
                        )
                    } else {
                        format!(
                            "disp_task_is_finished({task},{},{})",
                            span.start.line, span.start.column
                        )
                    }
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
                hir::CallTarget::Intrinsic(name) if name == "Channel.bounded" => {
                    let hir::Type::Result(ok_ty, _) = &destination_ty else {
                        unreachable!()
                    };
                    let hir::Type::Channel(value_ty) = ok_ty.as_ref() else {
                        unreachable!()
                    };
                    let capacity_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let capacity = operand(
                        program,
                        function,
                        &arguments[0],
                        &capacity_ty,
                        substitutions,
                    );
                    let value_c = native_types::c_type(value_ty);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{size_t _capacity=(size_t)({capacity});{result_c} _result={{0}};if(!_capacity){{_result.tag=1;_result.payload.v1.f0=disp_owned_bytes(\"Channel capacity must be greater than zero\",42);}}else if(_capacity>SIZE_MAX/sizeof({value_c})){{_result.tag=1;_result.payload.v1.f0=disp_owned_bytes(\"Channel capacity overflow\",25);}}else{{_result.tag=0;_result.payload.v0.f0=(disp_native_channel){{.state=disp_channel_create(_capacity,sizeof({value_c}),_Alignof({value_c}))}};}}_result;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("Channel.") => {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let value_ty = match &receiver_ty {
                        hir::Type::Channel(value_ty) => value_ty.as_ref(),
                        hir::Type::Reference { inner, .. } => {
                            let hir::Type::Channel(value_ty) = inner.as_ref() else {
                                unreachable!()
                            };
                            value_ty.as_ref()
                        }
                        _ => unreachable!(),
                    };
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    match name.as_str() {
                        "Channel.share" => format!(
                            "({{disp_channel_retain(({receiver})->state);(disp_native_channel){{.state=({receiver})->state}};}})"
                        ),
                        "Channel.send" => {
                            let value =
                                operand(program, function, &arguments[1], value_ty, substitutions);
                            let value_c = native_types::c_type(value_ty);
                            let value_drop = drop_value(program, "_message", value_ty);
                            format!(
                                "({{{value_c} _message={value};bool _sent=disp_channel_send(({receiver})->state,&_message,{},{});if(!_sent){{{value_drop}}}_sent;}})",
                                span.start.line, span.start.column
                            )
                        }
                        "Channel.receive" => {
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _result={{0}};if(disp_channel_receive(({receiver})->state,&_result.payload.v1.f0,{},{}))_result.tag=1;_result;}})",
                                span.start.line, span.start.column
                            )
                        }
                        "Channel.close" => format!(
                            "(disp_channel_close(({receiver})->state),(disp_native_unit){{0}})"
                        ),
                        "Channel.len" => {
                            format!("(uint64_t)disp_channel_len(({receiver})->state)")
                        }
                        "Channel.capacity" => {
                            format!("(uint64_t)disp_channel_capacity(({receiver})->state)")
                        }
                        "Channel.is_closed" => {
                            format!("disp_channel_is_closed(({receiver})->state)")
                        }
                        _ => unreachable!(),
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
                    let method = name.strip_prefix("AtomicInt.").unwrap();
                    let operation = atomic_operation(method).unwrap_or(method);
                    let order = atomic_c_order(method);
                    match operation {
                        "share" => format!(
                            "({{disp_atomic_int_retain(({receiver})->state);(disp_native_atomic_int){{.state=({receiver})->state}};}})"
                        ),
                        "load" => format!("disp_atomic_int_load(({receiver})->state,{order})"),
                        "store" => {
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
                                "(disp_atomic_int_store(({receiver})->state,(int64_t)({value}),{order}),(disp_native_unit){{0}})"
                            )
                        }
                        "fetch_add" | "add" => {
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
                                "disp_atomic_int_fetch_add(({receiver})->state,(int64_t)({value}),{order},{},{})",
                                span.start.line, span.start.column
                            );
                            if operation == "add" {
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
                hir::CallTarget::Intrinsic(name) if name.starts_with("CExport.callback:") => {
                    let symbol = name
                        .strip_prefix("CExport.callback:")
                        .expect("checked callback intrinsic has an export symbol");
                    format!("(({}){symbol})", native_types::c_type(&destination_ty))
                }
                hir::CallTarget::Intrinsic(name) if name == "CRegistration.adopt" => {
                    let context_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let release_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let context =
                        operand(program, function, &arguments[0], &context_ty, substitutions);
                    let release =
                        operand(program, function, &arguments[1], &release_ty, substitutions);
                    format!(
                        "disp_c_registration_open((void*)({context}),NULL,(void (*)(void*))({release}),NULL,{},{})",
                        span.start.line, span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "CRegistration.adopt_async" => {
                    let context_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let quiesce_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let release_ty = operand_ty(program, function, &arguments[2], substitutions);
                    let context =
                        operand(program, function, &arguments[0], &context_ty, substitutions);
                    let quiesce =
                        operand(program, function, &arguments[1], &quiesce_ty, substitutions);
                    let release =
                        operand(program, function, &arguments[2], &release_ty, substitutions);
                    format!(
                        "({{if(!({quiesce}))dv_panic(\"C registration quiesce callback is null\",{},{});disp_c_registration_open((void*)({context}),(void (*)(void*))({quiesce}),(void (*)(void*))({release}),NULL,{},{});}})",
                        span.start.line, span.start.column, span.start.line, span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if name.starts_with("CRegistration.register_async:") =>
                {
                    let handler_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let register_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let quiesce_ty = operand_ty(program, function, &arguments[2], substitutions);
                    let release_ty = operand_ty(program, function, &arguments[3], substitutions);
                    let handler =
                        operand(program, function, &arguments[0], &handler_ty, substitutions);
                    let register = operand(
                        program,
                        function,
                        &arguments[1],
                        &register_ty,
                        substitutions,
                    );
                    let quiesce =
                        operand(program, function, &arguments[2], &quiesce_ty, substitutions);
                    let release =
                        operand(program, function, &arguments[3], &release_ty, substitutions);
                    let hir::Type::Function(parameters, result) = &handler_ty else {
                        unreachable!("validated captured callback handler has function type")
                    };
                    let mut trampoline_parameters = vec![hir::Type::RawPointer {
                        mutable: true,
                        inner: Box::new(hir::Type::Unit),
                    }];
                    trampoline_parameters.extend(parameters.clone());
                    if !matches!(**result, hir::Type::Unit) {
                        trampoline_parameters.push(hir::Type::RawPointer {
                            mutable: true,
                            inner: result.clone(),
                        });
                    }
                    let trampoline_ty = hir::Type::CFunction(
                        trampoline_parameters,
                        Box::new(hir::Type::Int {
                            signed: true,
                            width: Some(32),
                        }),
                    );
                    let trampoline = c_context_callback_name(&handler_ty);
                    format!(
                        "({{if(!({register}))dv_panic(\"C callback register function is null\",{},{});if(!({quiesce}))dv_panic(\"C registration quiesce callback is null\",{},{});if(!({release}))dv_panic(\"C registration release callback is null\",{},{});disp_native_callable *_callback=(disp_native_callable*)disp_alloc(sizeof(disp_native_callable),_Alignof(disp_native_callable));*_callback={handler};void *_context=({register})(({}){trampoline},(void*)_callback);if(!_context){{if(_callback->drop)_callback->drop(_callback->env);disp_dealloc(_callback);dv_panic(\"C callback provider returned a null registration context\",{},{});}}disp_c_registration_open(_context,(void (*)(void*))({quiesce}),(void (*)(void*))({release}),_callback,{},{});}})",
                        span.start.line,
                        span.start.column,
                        span.start.line,
                        span.start.column,
                        span.start.line,
                        span.start.column,
                        native_types::c_type(&trampoline_ty),
                        span.start.line,
                        span.start.column,
                        span.start.line,
                        span.start.column
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "CRegistration.close" => {
                    let registration_ty =
                        operand_ty(program, function, &arguments[0], substitutions);
                    let registration = operand(
                        program,
                        function,
                        &arguments[0],
                        &registration_ty,
                        substitutions,
                    );
                    format!(
                        "({{disp_native_c_registration _registration={registration};disp_c_registration_close(&_registration);(disp_native_unit){{0}};}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "CRegistration.is_active" => {
                    let registration_ty =
                        operand_ty(program, function, &arguments[0], substitutions);
                    let registration = operand(
                        program,
                        function,
                        &arguments[0],
                        &registration_ty,
                        substitutions,
                    );
                    format!("(({registration})->active)")
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
                hir::CallTarget::Intrinsic(name) if name == "Url.parse" => {
                    let (source, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_url _url={{0}};disp_native_string _error={{0}};if(disp_url_parse(({source})->data,({source})->len,&_url,&_error)){{_r.tag=0;_r.payload.v0.f0=_url;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Json.parse" => {
                    let (source, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_json _json={{0}};disp_native_string _error={{0}};if(disp_json_parse(({source})->data,({source})->len,&_json,&_error)){{_r.tag=0;_r.payload.v0.f0=_json;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(name.as_str(), "Json.from" | "Json.decode") =>
                {
                    let codec_ty = substitute(
                        call_substitutions
                            .first()
                            .expect("JSON codec call carries its concrete type"),
                        substitutions,
                    );
                    let (source, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let value_c = native_types::c_type(&codec_ty);
                    let call = if name == "Json.from" {
                        format!(
                            "{}(({value_c}*){source},&_value,&_error)",
                            json_encoder_name(&codec_ty)
                        )
                    } else {
                        format!(
                            "{}((const disp_native_json*){source},&_value,&_error)",
                            json_decoder_name(&codec_ty)
                        )
                    };
                    if name == "Json.from" {
                        format!(
                            "({{{result_c} _r={{0}};disp_native_json _value={{0}};disp_native_string _error={{0}};if({call}){{_r.tag=0;_r.payload.v0.f0=_value;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    } else {
                        format!(
                            "({{{result_c} _r={{0}};{value_c} _value={{0}};disp_native_string _error={{0}};if({call}){{_r.tag=0;_r.payload.v0.f0=_value;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Json.null"
                            | "Json.bool"
                            | "Json.int"
                            | "Json.uint"
                            | "Json.float"
                            | "Json.string"
                            | "Json.array"
                            | "Json.object"
                    ) =>
                {
                    match name.as_str() {
                        "Json.null" => "disp_json_literal(\"null\",4)".into(),
                        "Json.bool" => {
                            let actual =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let value =
                                operand(program, function, &arguments[0], &actual, substitutions);
                            format!(
                                "({value})?disp_json_literal(\"true\",4):disp_json_literal(\"false\",5)"
                            )
                        }
                        "Json.int" | "Json.uint" => {
                            let actual =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let value =
                                operand(program, function, &arguments[0], &actual, substitutions);
                            if name == "Json.int" {
                                format!("disp_json_from_i128((__int128)({value}))")
                            } else {
                                format!("disp_json_from_u128((unsigned __int128)({value}))")
                            }
                        }
                        "Json.float" | "Json.string" | "Json.array" | "Json.object" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let call = match name.as_str() {
                                "Json.float" => {
                                    let actual =
                                        operand_ty(program, function, &arguments[0], substitutions);
                                    let value = operand(
                                        program,
                                        function,
                                        &arguments[0],
                                        &actual,
                                        substitutions,
                                    );
                                    format!("disp_json_from_f64((double)({value}),&_value,&_error)")
                                }
                                "Json.string" => {
                                    let (value, _) = system_argument(
                                        program,
                                        function,
                                        &arguments[0],
                                        substitutions,
                                    );
                                    format!(
                                        "disp_json_from_string(({value})->data,({value})->len,&_value,&_error)"
                                    )
                                }
                                "Json.array" => {
                                    let actual =
                                        operand_ty(program, function, &arguments[0], substitutions);
                                    let value = operand(
                                        program,
                                        function,
                                        &arguments[0],
                                        &actual,
                                        substitutions,
                                    );
                                    format!(
                                        "disp_json_from_array(({value}).data,({value}).len,&_value,&_error)"
                                    )
                                }
                                "Json.object" => {
                                    let actual =
                                        operand_ty(program, function, &arguments[0], substitutions);
                                    let value = operand(
                                        program,
                                        function,
                                        &arguments[0],
                                        &actual,
                                        substitutions,
                                    );
                                    format!(
                                        "disp_json_from_object(({value}).keys,({value}).values,({value}).len,&_value,&_error)"
                                    )
                                }
                                _ => unreachable!(),
                            };
                            format!(
                                "({{{result_c} _r={{0}};disp_native_json _value={{0}};disp_native_string _error={{0}};if({call}){{_r.tag=0;_r.payload.v0.f0=_value;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "SocketAddress.new" => {
                    let host_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let port_ty = operand_ty(program, function, &arguments[1], substitutions);
                    let port = operand(program, function, &arguments[1], &port_ty, substitutions);
                    let (port_c, invalid) =
                        if matches!(port_ty, hir::Type::Int { signed: true, .. }) {
                            ("__int128", "_port<0||_port>65535")
                        } else {
                            ("unsigned __int128", "_port>65535")
                        };
                    if matches!(host_ty, hir::Type::IpAddress) {
                        let host =
                            operand(program, function, &arguments[0], &host_ty, substitutions);
                        format!(
                            "({{{port_c} _port=({port_c})({port});if({invalid})dv_panic(\"socket port is outside 0 through 65535\",{},{});disp_native_ip_address _ip={host};disp_socket_address_from_ip(&_ip,(uint64_t)_port,{},{});}})",
                            span.start.line, span.start.column, span.start.line, span.start.column
                        )
                    } else {
                        let (host, _) =
                            system_argument(program, function, &arguments[0], substitutions);
                        format!(
                            "({{{port_c} _port=({port_c})({port});if({invalid})dv_panic(\"socket port is outside 0 through 65535\",{},{});disp_socket_address_create(({host})->data,({host})->len,(uint64_t)_port,{},{});}})",
                            span.start.line, span.start.column, span.start.line, span.start.column
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "IpAddress.parse" => {
                    let (source, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_ip_address _address={{0}};disp_native_string _error={{0}};if(disp_ip_address_parse(({source})->data,({source})->len,&_address,&_error)){{_r.tag=0;_r.payload.v0.f0=_address;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Dns.resolve" => {
                    let (host, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let hir::Type::Result(value, _) = &destination_ty else {
                        unreachable!("DNS resolution must return Result")
                    };
                    let list_c = native_types::c_type(value);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_ip_list _addresses={{0}};disp_native_string _error={{0}};if(disp_dns_resolve(({host})->data,({host})->len,&_addresses,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=_addresses.data,.len=_addresses.len,.cap=_addresses.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "TcpListener.bind" => {
                    let address_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let address =
                        operand(program, function, &arguments[0], &address_ty, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_tcp_listener _listener={{0}};disp_native_string _error={{0}};if(disp_tcp_listener_bind({address},&_listener,&_error)){{_r.tag=0;_r.payload.v0.f0=_listener;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "UdpSocket.bind" => {
                    let address_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let address =
                        operand(program, function, &arguments[0], &address_ty, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_udp_socket _socket={{0}};disp_native_string _error={{0}};if(disp_udp_socket_bind({address},&_socket,&_error)){{_r.tag=0;_r.payload.v0.f0=_socket;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("TcpStream.") => {
                    let (stream, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "TcpStream.close" => {
                            format!("(disp_tcp_stream_close({stream}),(disp_native_unit){{0}})")
                        }
                        "TcpStream.read" => {
                            let limit_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let limit =
                                operand(program, function, &arguments[1], &limit_ty, substitutions);
                            let (limit_c, invalid) =
                                if matches!(limit_ty, hir::Type::Int { signed: true, .. }) {
                                    ("__int128", "_limit<0||_limit>16777216")
                                } else {
                                    ("unsigned __int128", "_limit>16777216")
                                };
                            let result_c = native_types::c_type(&destination_ty);
                            let hir::Type::Result(value, _) = &destination_ty else {
                                unreachable!("TCP read must return Result")
                            };
                            let list_c = native_types::c_type(value);
                            format!(
                                "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"TCP read limit exceeds the 16 MiB safety limit\",{},{});{result_c} _r={{0}};disp_native_string _bytes={{0}},_error={{0}};if(disp_tcp_stream_read({stream},(size_t)_limit,&_bytes,&_error,{},{})){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                                span.start.line,
                                span.start.column,
                                span.start.line,
                                span.start.column
                            )
                        }
                        "TcpStream.write" => {
                            let bytes_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let bytes =
                                operand(program, function, &arguments[1], &bytes_ty, substitutions);
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};size_t _written=0;disp_native_string _error={{0}};if(disp_tcp_stream_write({stream},(const char*)({bytes}).data,({bytes}).len,&_written,&_error)){{_r.tag=0;_r.payload.v0.f0=(uint64_t)_written;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "TcpStream.read_async" | "TcpStream.read_async_timeout" => {
                            let hir::Type::Future(result) = &destination_ty else {
                                unreachable!("TCP async read must return Future")
                            };
                            let limit_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let limit =
                                operand(program, function, &arguments[1], &limit_ty, substitutions);
                            let (limit_c, invalid) =
                                if matches!(limit_ty, hir::Type::Int { signed: true, .. }) {
                                    ("__int128", "_limit<0||_limit>16777216")
                                } else {
                                    ("unsigned __int128", "_limit>16777216")
                                };
                            let (has_timeout, timeout) = if arguments.len() == 3 {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[2], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[2],
                                    &timeout_ty,
                                    substitutions,
                                );
                                ("true", format!("({timeout}).nanos"))
                            } else {
                                ("false", "0".into())
                            };
                            let poll = async_poll_name(name, result);
                            format!(
                                "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"TCP read limit exceeds the 16 MiB safety limit\",{},{});disp_socket_io_state *_state=disp_socket_io_create(({stream})->state,DISP_SOCKET_READ,NULL,(size_t)_limit,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_socket_io_drop}};}})",
                                span.start.line,
                                span.start.column,
                                span.start.line,
                                span.start.column
                            )
                        }
                        "TcpStream.write_async" | "TcpStream.write_async_timeout" => {
                            let hir::Type::Future(result) = &destination_ty else {
                                unreachable!("TCP async write must return Future")
                            };
                            let bytes_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let bytes =
                                operand(program, function, &arguments[1], &bytes_ty, substitutions);
                            let bytes_c = native_types::c_type(&bytes_ty);
                            let (has_timeout, timeout) = if arguments.len() == 3 {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[2], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[2],
                                    &timeout_ty,
                                    substitutions,
                                );
                                ("true", format!("({timeout}).nanos"))
                            } else {
                                ("false", "0".into())
                            };
                            let poll = async_poll_name(name, result);
                            format!(
                                "({{{bytes_c} _bytes={bytes};disp_socket_io_state *_state=disp_socket_io_create(({stream})->state,DISP_SOCKET_WRITE,(const char*)_bytes.data,_bytes.len,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_socket_io_drop}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        "TcpStream.shutdown_read" | "TcpStream.shutdown_write" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let reading = if name == "TcpStream.shutdown_read" {
                                "true"
                            } else {
                                "false"
                            };
                            format!(
                                "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_tcp_stream_shutdown({stream},{reading},&_error)){{_r.tag=0;_r.payload.v0.f0=(disp_native_unit){{0}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("TlsStream.") => {
                    let (stream, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "TlsStream.close" => {
                            format!("(disp_tls_stream_close({stream}),(disp_native_unit){{0}})")
                        }
                        "TlsStream.read" => {
                            let limit_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let limit =
                                operand(program, function, &arguments[1], &limit_ty, substitutions);
                            let (limit_c, invalid) =
                                if matches!(limit_ty, hir::Type::Int { signed: true, .. }) {
                                    ("__int128", "_limit<0||_limit>16777216")
                                } else {
                                    ("unsigned __int128", "_limit>16777216")
                                };
                            let result_c = native_types::c_type(&destination_ty);
                            let hir::Type::Result(value, _) = &destination_ty else {
                                unreachable!("TLS read must return Result")
                            };
                            let list_c = native_types::c_type(value);
                            format!(
                                "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"TLS read limit exceeds the 16 MiB safety limit\",{},{});{result_c} _r={{0}};disp_native_string _bytes={{0}},_error={{0}};if(disp_tls_stream_read({stream},(size_t)_limit,&_bytes,&_error,{},{})){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                                span.start.line,
                                span.start.column,
                                span.start.line,
                                span.start.column
                            )
                        }
                        "TlsStream.write" => {
                            let bytes_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let bytes =
                                operand(program, function, &arguments[1], &bytes_ty, substitutions);
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};size_t _written=0;disp_native_string _error={{0}};if(disp_tls_stream_write({stream},(const char*)({bytes}).data,({bytes}).len,&_written,&_error,{},{})){{_r.tag=0;_r.payload.v0.f0=(uint64_t)_written;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                                span.start.line, span.start.column
                            )
                        }
                        "TlsStream.read_async" | "TlsStream.read_async_timeout" => {
                            let hir::Type::Future(result) = &destination_ty else {
                                unreachable!("TLS async read must return Future")
                            };
                            let limit_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let limit =
                                operand(program, function, &arguments[1], &limit_ty, substitutions);
                            let (limit_c, invalid) =
                                if matches!(limit_ty, hir::Type::Int { signed: true, .. }) {
                                    ("__int128", "_limit<0||_limit>16777216")
                                } else {
                                    ("unsigned __int128", "_limit>16777216")
                                };
                            let (has_timeout, timeout) = if arguments.len() == 3 {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[2], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[2],
                                    &timeout_ty,
                                    substitutions,
                                );
                                ("true", format!("({timeout}).nanos"))
                            } else {
                                ("false", "0".into())
                            };
                            let poll = async_poll_name(name, result);
                            format!(
                                "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"TLS read limit exceeds the 16 MiB safety limit\",{},{});disp_tls_io_state *_state=disp_tls_io_create(({stream})->state,DISP_TLS_READ,NULL,(size_t)_limit,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_tls_io_drop}};}})",
                                span.start.line,
                                span.start.column,
                                span.start.line,
                                span.start.column
                            )
                        }
                        "TlsStream.write_async" | "TlsStream.write_async_timeout" => {
                            let hir::Type::Future(result) = &destination_ty else {
                                unreachable!("TLS async write must return Future")
                            };
                            let bytes_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let bytes =
                                operand(program, function, &arguments[1], &bytes_ty, substitutions);
                            let bytes_c = native_types::c_type(&bytes_ty);
                            let (has_timeout, timeout) = if arguments.len() == 3 {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[2], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[2],
                                    &timeout_ty,
                                    substitutions,
                                );
                                ("true", format!("({timeout}).nanos"))
                            } else {
                                ("false", "0".into())
                            };
                            let poll = async_poll_name(name, result);
                            format!(
                                "({{{bytes_c} _bytes={bytes};disp_tls_io_state *_state=disp_tls_io_create(({stream})->state,DISP_TLS_WRITE,(const char*)_bytes.data,_bytes.len,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_tls_io_drop}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("TcpListener.") => {
                    let (listener, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "TcpListener.accept" | "TcpListener.accept_timeout" => {
                            let hir::Type::Future(result) = &destination_ty else {
                                unreachable!("TCP accept must return Future")
                            };
                            let (has_timeout, timeout) = if arguments.len() == 2 {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[1],
                                    &timeout_ty,
                                    substitutions,
                                );
                                ("true", format!("({timeout}).nanos"))
                            } else {
                                ("false", "0".into())
                            };
                            let poll = async_poll_name(name, result);
                            format!(
                                "({{disp_accept_state *_state=disp_accept_create({listener},{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_accept_drop}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        "TcpListener.local_port" => {
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};size_t _port=0;disp_native_string _error={{0}};if(disp_tcp_listener_local_port({listener},&_port,&_error)){{_r.tag=0;_r.payload.v0.f0=(uint64_t)_port;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "TcpListener.close" => {
                            format!("(disp_tcp_listener_close({listener}),(disp_native_unit){{0}})")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("UdpSocket.") => {
                    let (socket, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "UdpSocket.receive_from"
                        | "UdpSocket.receive_from_async"
                        | "UdpSocket.receive_from_async_timeout" => {
                            let limit_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let limit =
                                operand(program, function, &arguments[1], &limit_ty, substitutions);
                            let (limit_c, invalid) =
                                if matches!(limit_ty, hir::Type::Int { signed: true, .. }) {
                                    ("__int128", "_limit<0||_limit>65535")
                                } else {
                                    ("unsigned __int128", "_limit>65535")
                                };
                            if name == "UdpSocket.receive_from" {
                                let result_c = native_types::c_type(&destination_ty);
                                format!(
                                    "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"UDP receive limit exceeds 65535 bytes\",{},{});{result_c} _r={{0}};disp_native_udp_datagram _datagram={{0}};disp_native_string _error={{0}};if(disp_udp_socket_receive({socket},(size_t)_limit,&_datagram,&_error,{},{})){{_r.tag=0;_r.payload.v0.f0=_datagram;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                                    span.start.line,
                                    span.start.column,
                                    span.start.line,
                                    span.start.column
                                )
                            } else {
                                let hir::Type::Future(result) = &destination_ty else {
                                    unreachable!("UDP async receive must return Future")
                                };
                                let (has_timeout, timeout) = if arguments.len() == 3 {
                                    let timeout_ty =
                                        operand_ty(program, function, &arguments[2], substitutions);
                                    let timeout = operand(
                                        program,
                                        function,
                                        &arguments[2],
                                        &timeout_ty,
                                        substitutions,
                                    );
                                    ("true", format!("({timeout}).nanos"))
                                } else {
                                    ("false", "0".into())
                                };
                                let poll = async_poll_name(name, result);
                                format!(
                                    "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"UDP receive limit exceeds 65535 bytes\",{},{});disp_udp_io_state *_state=disp_udp_io_create(({socket})->state,DISP_UDP_RECEIVE,NULL,(size_t)_limit,NULL,{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_udp_io_drop}};}})",
                                    span.start.line,
                                    span.start.column,
                                    span.start.line,
                                    span.start.column
                                )
                            }
                        }
                        "UdpSocket.send_to"
                        | "UdpSocket.send_to_async"
                        | "UdpSocket.send_to_async_timeout" => {
                            let bytes_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let bytes =
                                operand(program, function, &arguments[1], &bytes_ty, substitutions);
                            let bytes_c = native_types::c_type(&bytes_ty);
                            let (address, _) =
                                system_argument(program, function, &arguments[2], substitutions);
                            if name == "UdpSocket.send_to" {
                                let result_c = native_types::c_type(&destination_ty);
                                format!(
                                    "({{{bytes_c} _bytes={bytes};{result_c} _r={{0}};size_t _sent=0;disp_native_string _error={{0}};if(disp_udp_socket_send({socket},(const char*)_bytes.data,_bytes.len,{address},&_sent,&_error)){{_r.tag=0;_r.payload.v0.f0=(uint64_t)_sent;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            } else {
                                let hir::Type::Future(result) = &destination_ty else {
                                    unreachable!("UDP async send must return Future")
                                };
                                let (has_timeout, timeout) = if arguments.len() == 4 {
                                    let timeout_ty =
                                        operand_ty(program, function, &arguments[3], substitutions);
                                    let timeout = operand(
                                        program,
                                        function,
                                        &arguments[3],
                                        &timeout_ty,
                                        substitutions,
                                    );
                                    ("true", format!("({timeout}).nanos"))
                                } else {
                                    ("false", "0".into())
                                };
                                let poll = async_poll_name(name, result);
                                format!(
                                    "({{{bytes_c} _bytes={bytes};disp_udp_io_state *_state=disp_udp_io_create(({socket})->state,DISP_UDP_SEND,(const char*)_bytes.data,_bytes.len,{address},{has_timeout},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_udp_io_drop}};}})",
                                    span.start.line, span.start.column
                                )
                            }
                        }
                        "UdpSocket.local_port" => {
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};size_t _port=0;disp_native_string _error={{0}};if(disp_udp_socket_local_port({socket},&_port,&_error)){{_r.tag=0;_r.payload.v0.f0=(uint64_t)_port;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "UdpSocket.close" => {
                            format!("(disp_udp_socket_close({socket}),(disp_native_unit){{0}})")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("UdpDatagram.") => {
                    let (datagram, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "UdpDatagram.bytes" => {
                            let list_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{list_c} _bytes={{.data=NULL,.len=({datagram})->len,.cap=({datagram})->len}};if(_bytes.len){{_bytes.data=(uint8_t*)disp_alloc(_bytes.len,1);memcpy(_bytes.data,({datagram})->data,_bytes.len);}}_bytes;}})"
                            )
                        }
                        "UdpDatagram.source" => {
                            format!("disp_socket_address_clone(&({datagram})->source)")
                        }
                        "UdpDatagram.len" => format!("({datagram})->len"),
                        "UdpDatagram.is_empty" => format!("({datagram})->len==0"),
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("HttpResponse.") => {
                    let (response, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "HttpResponse.status" => {
                            format!("disp_http_response_status({response})")
                        }
                        "HttpResponse.is_success" => {
                            format!("disp_http_response_is_success({response})")
                        }
                        "HttpResponse.body" => {
                            let list_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{list_c} _body={{0}};disp_native_string _bytes=disp_http_response_body({response});_body.data=(uint8_t*)_bytes.data;_body.len=_bytes.len;_body.cap=_bytes.cap;_body;}})"
                            )
                        }
                        "HttpResponse.text" => {
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};disp_native_string _text={{0}},_error={{0}};if(disp_http_response_text({response},&_text,&_error)){{_r.tag=0;_r.payload.v0.f0=_text;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "HttpResponse.json" => {
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};disp_native_json _json={{0}};disp_native_string _error={{0}};if(disp_http_response_json({response},&_json,&_error)){{_r.tag=0;_r.payload.v0.f0=_json;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "HttpResponse.header" => {
                            let option_c = native_types::c_type(&destination_ty);
                            let (header, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            format!(
                                "({{{option_c} _r={{0}};disp_native_string _value={{0}};if(disp_http_response_header({response},({header})->data,({header})->len,&_value,{},{})){{_r.tag=1;_r.payload.v1.f0=_value;}}_r;}})",
                                span.start.line, span.start.column
                            )
                        }
                        "HttpResponse.url" => format!("disp_http_response_url({response})"),
                        "HttpResponse.len" => format!("disp_http_response_len({response})"),
                        "HttpResponse.is_empty" => {
                            format!("disp_http_response_len({response})==0")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("HttpRequest.") => {
                    let (request, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "HttpRequest.header" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let (header, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            let (value, _) =
                                system_argument(program, function, &arguments[2], substitutions);
                            format!(
                                "({{{result_c} _r={{0}};disp_native_http_request _next={{0}};disp_native_string _error={{0}};if(disp_http_builder_header({request},({header})->data,({header})->len,({value})->data,({value})->len,&_next,&_error)){{_r.tag=0;_r.payload.v0.f0=_next;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "HttpRequest.text" | "HttpRequest.bytes" | "HttpRequest.json" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let (body, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            let text = name == "HttpRequest.text";
                            let json = name == "HttpRequest.json";
                            format!(
                                "({{{result_c} _r={{0}};disp_native_http_request _next={{0}};disp_native_string _error={{0}};if(disp_http_builder_body({request},({body})->data,({body})->len,{text},{json},&_next,&_error)){{_r.tag=0;_r.payload.v0.f0=_next;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "HttpRequest.send" | "HttpRequest.send_timeout" => {
                            let hir::Type::Future(result) = &destination_ty else {
                                unreachable!("HTTP send destination must be Future<T>")
                            };
                            let timeout = if arguments.len() == 2 {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[1],
                                    &timeout_ty,
                                    substitutions,
                                );
                                format!("({timeout}).nanos")
                            } else {
                                "30000000000ULL".into()
                            };
                            let poll = async_poll_name(name, result);
                            format!(
                                "({{disp_http_request_state *_state=disp_http_request_from_builder({request},{timeout},{},{});(disp_native_future){{.context=_state,.poll={poll},.drop=disp_http_request_drop}};}})",
                                span.start.line, span.start.column
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("IpAddress.") => {
                    let (ip, _) = system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "IpAddress.as_string" => format!("disp_ip_address_string({ip})"),
                        "IpAddress.is_ipv4" => format!("({ip})->family==4"),
                        "IpAddress.is_ipv6" => format!("({ip})->family==6"),
                        "IpAddress.is_loopback" => {
                            format!("disp_ip_address_loopback({ip})")
                        }
                        "IpAddress.is_unspecified" => {
                            format!("disp_ip_address_unspecified({ip})")
                        }
                        _ => unreachable!(),
                    }
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
                hir::CallTarget::Intrinsic(name) if name.starts_with("Url.") => {
                    let (url, _) = system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "Url.as_string" => format!("disp_url_as_string({url})"),
                        "Url.scheme" => format!("disp_url_scheme({url})"),
                        "Url.host" | "Url.query" => {
                            let option_c = native_types::c_type(&destination_ty);
                            let helper = if name == "Url.host" {
                                "disp_url_host"
                            } else {
                                "disp_url_query"
                            };
                            format!(
                                "({{{option_c} _r={{0}};disp_native_string _value={{0}};if({helper}({url},&_value)){{_r.tag=1;_r.payload.v1.f0=_value;}}_r;}})"
                            )
                        }
                        "Url.port" => {
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{option_c} _r={{0}};uint64_t _value=0;if(disp_url_port({url},&_value)){{_r.tag=1;_r.payload.v1.f0=_value;}}_r;}})"
                            )
                        }
                        "Url.path" => format!("disp_url_path({url})"),
                        "Url.is_secure" => format!("disp_url_is_secure({url})"),
                        "Url.join_path" | "Url.query_param" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let (first, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            let call = if name == "Url.join_path" {
                                format!(
                                    "disp_url_join_path({url},({first})->data,({first})->len,&_value,&_error)"
                                )
                            } else {
                                let (second, _) = system_argument(
                                    program,
                                    function,
                                    &arguments[2],
                                    substitutions,
                                );
                                format!(
                                    "disp_url_query_param({url},({first})->data,({first})->len,({second})->data,({second})->len,&_value,&_error)"
                                )
                            };
                            format!(
                                "({{{result_c} _r={{0}};disp_native_url _value={{0}};disp_native_string _error={{0}};if({call}){{_r.tag=0;_r.payload.v0.f0=_value;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("Json.") => {
                    let (json, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "Json.as_string" => format!("disp_json_as_string({json})"),
                        "Json.kind" => format!("disp_json_kind({json})"),
                        "Json.len" => format!("({json})->len"),
                        "Json.is_null" => format!("disp_json_is_kind({json},\"null\")"),
                        "Json.is_bool" => format!("disp_json_is_kind({json},\"bool\")"),
                        "Json.is_number" => format!("disp_json_is_kind({json},\"number\")"),
                        "Json.is_string" => format!("disp_json_is_kind({json},\"string\")"),
                        "Json.is_array" => format!("disp_json_is_kind({json},\"array\")"),
                        "Json.is_object" => format!("disp_json_is_kind({json},\"object\")"),
                        "Json.get" => {
                            let (key, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{option_c} _r={{0}};disp_native_json _value={{0}};if(disp_json_get({json},({key})->data,({key})->len,&_value)){{_r.tag=1;_r.payload.v1.f0=_value;}}_r;}})"
                            )
                        }
                        "Json.at" => {
                            let actual =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let index =
                                operand(program, function, &arguments[1], &actual, substitutions);
                            let negative = if matches!(actual, hir::Type::Int { signed: true, .. })
                            {
                                format!(
                                    "if(({index})<0)dv_panic(\"JSON index cannot be negative\",{},{});",
                                    span.start.line, span.start.column
                                )
                            } else {
                                String::new()
                            };
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{negative}{option_c} _r={{0}};disp_native_json _value={{0}};if(disp_json_at({json},(size_t)({index}),&_value)){{_r.tag=1;_r.payload.v1.f0=_value;}}_r;}})"
                            )
                        }
                        "Json.as_bool" | "Json.as_int" | "Json.as_uint" | "Json.as_f64"
                        | "Json.as_text" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let (value_c, helper) = match name.as_str() {
                                "Json.as_bool" => ("bool", "disp_json_as_bool"),
                                "Json.as_int" => ("int64_t", "disp_json_as_int"),
                                "Json.as_uint" => ("uint64_t", "disp_json_as_uint"),
                                "Json.as_f64" => ("double", "disp_json_as_f64"),
                                "Json.as_text" => ("disp_native_string", "disp_json_as_text"),
                                _ => unreachable!(),
                            };
                            format!(
                                "({{{result_c} _r={{0}};{value_c} _value={{0}};disp_native_string _error={{0}};if({helper}({json},&_value,&_error)){{_r.tag=0;_r.payload.v0.f0=_value;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
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
                                    "({{{result_c} _r={{0}};disp_native_string _bytes={{0}},_error={{0}};if(disp_file_read_bytes({path},&_bytes,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
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
                hir::CallTarget::Intrinsic(name) if name.starts_with("Environment.") => {
                    match name.as_str() {
                        "Environment.arguments" => {
                            let list_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{list_c} _r={{0}};if(disp_program_argc>0){{_r.data=(disp_native_string*)disp_alloc_zeroed((size_t)disp_program_argc,sizeof(disp_native_string),_Alignof(disp_native_string));_r.len=_r.cap=(size_t)disp_program_argc;for(size_t _i=0;_i<_r.len;_i++)_r.data[_i]=disp_owned_bytes(disp_program_argv[_i],strlen(disp_program_argv[_i]));}}_r;}})"
                            )
                        }
                        "Environment.get" => {
                            let (name, _) =
                                system_argument(program, function, &arguments[0], substitutions);
                            let option_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{option_c} _r={{0}};disp_native_string *_name={name};if(!_name->len||memchr(_name->data,0,_name->len)||memchr(_name->data,'=',_name->len))dv_panic(\"environment variable name cannot be empty or contain '=' or NUL\",{},{});disp_native_string _value={{0}};bool _found=false;if(!disp_environment_get(_name,&_value,&_found))dv_panic(\"environment variable value is not valid UTF-8\",{},{});if(_found){{_r.tag=1;_r.payload.v1.f0=_value;}}_r;}})",
                                span.start.line,
                                span.start.column,
                                span.start.line,
                                span.start.column
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.random_bytes" => {
                    let actual = operand_ty(program, function, &arguments[0], substitutions);
                    let length = operand(program, function, &arguments[0], &actual, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let hir::Type::Result(ok, _) = &destination_ty else {
                        unreachable!()
                    };
                    let list_c = native_types::c_type(ok);
                    let nonnegative = if matches!(actual, hir::Type::Int { signed: true, .. }) {
                        format!("({length})>=0&&")
                    } else {
                        String::new()
                    };
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _bytes={{0}},_error={{0}};bool _valid={nonnegative}(unsigned __int128)({length})<=(unsigned __int128)SIZE_MAX;if(!_valid){{const char *_message=\"secure-random byte length must be a non-negative platform-sized integer\";_error=disp_owned_bytes(_message,strlen(_message));}}if(_valid&&disp_crypto_random_bytes((size_t)({length}),&_bytes,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.random_secret" => {
                    let actual = operand_ty(program, function, &arguments[0], substitutions);
                    let length = operand(program, function, &arguments[0], &actual, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let nonnegative = if matches!(actual, hir::Type::Int { signed: true, .. }) {
                        format!("({length})>=0&&")
                    } else {
                        String::new()
                    };
                    format!(
                        "({{{result_c} _r={{0}};disp_native_secret _secret={{0}};disp_native_string _error={{0}};bool _valid={nonnegative}(unsigned __int128)({length})<=(unsigned __int128)SIZE_MAX;if(!_valid){{const char *_message=\"secure-random byte length must be a non-negative platform-sized integer\";_error=disp_owned_bytes(_message,strlen(_message));}}if(_valid&&disp_crypto_random_secret((size_t)({length}),&_secret,&_error)){{_r.tag=0;_r.payload.v0.f0=_secret;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.import_secret" => {
                    let bytes_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let bytes = operand(program, function, &arguments[0], &bytes_ty, substitutions);
                    let bytes_c = native_types::c_type(&bytes_ty);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};{bytes_c} _bytes={bytes};disp_native_secret _secret={{0}};disp_native_string _error={{0}};if(disp_crypto_import_secret(_bytes.data,_bytes.len,_bytes.cap,&_secret,&_error)){{_r.tag=0;_r.payload.v0.f0=_secret;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Crypto.sha256" | "Crypto.hmac_sha256" | "Crypto.hmac_sha256_verify"
                    ) =>
                {
                    let result_c = native_types::c_type(&destination_ty);
                    let message_index = usize::from(name != "Crypto.sha256");
                    let (message, _) = system_argument(
                        program,
                        function,
                        &arguments[message_index],
                        substitutions,
                    );
                    if name == "Crypto.hmac_sha256_verify" {
                        let (key, _) =
                            system_argument(program, function, &arguments[0], substitutions);
                        let (expected, _) =
                            system_argument(program, function, &arguments[2], substitutions);
                        format!(
                            "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _valid=false;if(disp_crypto_hmac_sha256_verify({key},({message})->data,({message})->len,({expected})->data,({expected})->len,&_valid,&_error)){{_r.tag=0;_r.payload.v0.f0=_valid;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    } else {
                        let hir::Type::Result(ok, _) = &destination_ty else {
                            unreachable!()
                        };
                        let list_c = native_types::c_type(ok);
                        let call = if name == "Crypto.sha256" {
                            format!(
                                "disp_crypto_sha256(({message})->data,({message})->len,&_digest,&_error)"
                            )
                        } else {
                            let (key, _) =
                                system_argument(program, function, &arguments[0], substitutions);
                            format!(
                                "disp_crypto_hmac_sha256({key},({message})->data,({message})->len,&_digest,&_error)"
                            )
                        };
                        format!(
                            "({{{result_c} _r={{0}};disp_native_string _digest={{0}},_error={{0}};if({call}){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_digest.data,.len=_digest.len,.cap=_digest.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.hkdf_sha256" => {
                    let (salt, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (input, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let (info, _) =
                        system_argument(program, function, &arguments[2], substitutions);
                    let length_ty = operand_ty(program, function, &arguments[3], substitutions);
                    let length =
                        operand(program, function, &arguments[3], &length_ty, substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let nonnegative = if matches!(length_ty, hir::Type::Int { signed: true, .. }) {
                        format!("({length})>=0&&")
                    } else {
                        String::new()
                    };
                    format!(
                        "({{{result_c} _r={{0}};disp_native_secret _output={{0}};disp_native_string _error={{0}};bool _valid={nonnegative}(unsigned __int128)({length})<=(unsigned __int128)SIZE_MAX;if(!_valid){{const char *_message=\"HKDF-SHA-256 output length must be a non-negative platform-sized integer\";_error=disp_owned_bytes(_message,strlen(_message));}}if(_valid&&disp_crypto_hkdf_sha256(({salt})->data,({salt})->len,{input},({info})->data,({info})->len,(size_t)({length}),&_output,&_error)){{_r.tag=0;_r.payload.v0.f0=_output;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Crypto.aes256_gcm_siv_seal" | "Crypto.aes256_gcm_siv_open"
                    ) =>
                {
                    let (key, _) = system_argument(program, function, &arguments[0], substitutions);
                    let (input, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let (associated_data, _) =
                        system_argument(program, function, &arguments[2], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    if name == "Crypto.aes256_gcm_siv_seal" {
                        format!(
                            "({{{result_c} _r={{0}};disp_native_string _envelope={{0}},_error={{0}};if(disp_crypto_aead_seal({key},{input},({associated_data})->data,({associated_data})->len,&_envelope,&_error)){{_r.tag=0;_r.payload.v0.f0=_envelope;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    } else {
                        format!(
                            "({{{result_c} _r={{0}};disp_native_secret _plaintext={{0}};disp_native_string _error={{0}};if(disp_crypto_aead_open({key},{input},({associated_data})->data,({associated_data})->len,&_plaintext,&_error)){{_r.tag=0;_r.payload.v0.f0=_plaintext;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Crypto.encode_aead_envelope" | "Crypto.decode_aead_envelope"
                    ) =>
                {
                    let (input, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let hir::Type::Result(ok, _) = &destination_ty else {
                        unreachable!()
                    };
                    if name == "Crypto.encode_aead_envelope" {
                        let list_c = native_types::c_type(ok);
                        format!(
                            "({{{result_c} _r={{0}};disp_native_string _encoded={{0}},_error={{0}};if(disp_crypto_aead_encode({input},&_encoded,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_encoded.data,.len=_encoded.len,.cap=_encoded.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    } else {
                        format!(
                            "({{{result_c} _r={{0}};disp_native_string _envelope={{0}},_error={{0}};if(disp_crypto_aead_decode(({input})->data,({input})->len,&_envelope,&_error)){{_r.tag=0;_r.payload.v0.f0=_envelope;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.ed25519_generate" => {
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_secret _key={{0}};disp_native_string _error={{0}};if(disp_crypto_ed25519_generate(&_key,&_error)){{_r.tag=0;_r.payload.v0.f0=_key;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Crypto.ed25519_public_key" | "Crypto.ed25519_sign"
                    ) =>
                {
                    let (key, _) = system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let hir::Type::Result(ok, _) = &destination_ty else {
                        unreachable!()
                    };
                    let list_c = native_types::c_type(ok);
                    let call = if name == "Crypto.ed25519_public_key" {
                        format!("disp_crypto_ed25519_public_key({key},&_bytes,&_error)")
                    } else {
                        let (message, _) =
                            system_argument(program, function, &arguments[1], substitutions);
                        format!(
                            "disp_crypto_ed25519_sign({key},({message})->data,({message})->len,&_bytes,&_error)"
                        )
                    };
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _bytes={{0}},_error={{0}};if({call}){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_bytes.data,.len=_bytes.len,.cap=_bytes.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.ed25519_verify" => {
                    let (public_key, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (message, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let (signature, _) =
                        system_argument(program, function, &arguments[2], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _valid=false;if(disp_crypto_ed25519_verify(({public_key})->data,({public_key})->len,({message})->data,({message})->len,({signature})->data,({signature})->len,&_valid,&_error)){{_r.tag=0;_r.payload.v0.f0=_valid;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.ed25519_key_id" => {
                    let (public_key, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let hir::Type::Result(ok, _) = &destination_ty else {
                        unreachable!()
                    };
                    let list_c = native_types::c_type(ok);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _key_id={{0}},_error={{0}};if(disp_crypto_ed25519_key_id(({public_key})->data,({public_key})->len,&_key_id,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_key_id.data,.len=_key_id.len,.cap=_key_id.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.ed25519_verify_keyed" => {
                    let (expected_key_id, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (public_key, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let (message, _) =
                        system_argument(program, function, &arguments[2], substitutions);
                    let (signature, _) =
                        system_argument(program, function, &arguments[3], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _valid=false;if(disp_crypto_ed25519_verify_keyed(({expected_key_id})->data,({expected_key_id})->len,({public_key})->data,({public_key})->len,({message})->data,({message})->len,({signature})->data,({signature})->len,&_valid,&_error)){{_r.tag=0;_r.payload.v0.f0=_valid;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.ed25519_verify_lifecycle" => {
                    let (expected_key_id, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (public_key, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let (message, _) =
                        system_argument(program, function, &arguments[2], substitutions);
                    let (signature, _) =
                        system_argument(program, function, &arguments[3], substitutions);
                    let policy = (4..8)
                        .map(|index| {
                            let ty =
                                operand_ty(program, function, &arguments[index], substitutions);
                            operand(program, function, &arguments[index], &ty, substitutions)
                        })
                        .collect::<Vec<_>>();
                    let valid_from = &policy[0];
                    let valid_until = &policy[1];
                    let revoked = &policy[2];
                    let evaluation_time = &policy[3];
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _valid=false;if(disp_crypto_ed25519_verify_lifecycle(({expected_key_id})->data,({expected_key_id})->len,({public_key})->data,({public_key})->len,({message})->data,({message})->len,({signature})->data,({signature})->len,(uint64_t)({valid_from}),(uint64_t)({valid_until}),({revoked}),(uint64_t)({evaluation_time}),&_valid,&_error)){{_r.tag=0;_r.payload.v0.f0=_valid;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Crypto.encode_ed25519_public_key"
                            | "Crypto.decode_ed25519_public_key"
                            | "Crypto.encode_ed25519_signature"
                            | "Crypto.decode_ed25519_signature"
                    ) =>
                {
                    let (input, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    let hir::Type::Result(ok, _) = &destination_ty else {
                        unreachable!()
                    };
                    let list_c = native_types::c_type(ok);
                    let public_key = name.ends_with("public_key");
                    let decode = name.contains("decode_");
                    let kind = if public_key { 2 } else { 3 };
                    let payload_length = if public_key { 32 } else { 64 };
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _output={{0}},_error={{0}};if(disp_crypto_ed25519_record(({input})->data,({input})->len,{kind},{payload_length},{decode},&_output,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=(uint8_t*)_output.data,.len=_output.len,.cap=_output.cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.argon2id_hash_password" => {
                    let (password, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _encoded={{0}},_error={{0}};if(disp_crypto_argon2id_hash({password},&_encoded,&_error)){{_r.tag=0;_r.payload.v0.f0=_encoded;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name == "Crypto.argon2id_verify_password" => {
                    let (password, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (encoded, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _valid=false;if(disp_crypto_argon2id_verify({password},({encoded})->data,({encoded})->len,&_valid,&_error)){{_r.tag=0;_r.payload.v0.f0=_valid;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("SecretBytes.") => {
                    let receiver_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let receiver = operand(
                        program,
                        function,
                        &arguments[0],
                        &receiver_ty,
                        substitutions,
                    );
                    match name.as_str() {
                        "SecretBytes.len" => format!("({receiver})->len"),
                        "SecretBytes.is_empty" => format!("(({receiver})->len==0)"),
                        "SecretBytes.constant_time_equals" => {
                            let (other, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            format!("disp_secret_constant_time_equals({receiver},{other})")
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name)
                    if matches!(
                        name.as_str(),
                        "Database.open" | "Database.memory" | "DataStore.open" | "DataStore.memory"
                    ) =>
                {
                    let result_c = native_types::c_type(&destination_ty);
                    if matches!(name.as_str(), "Database.open" | "DataStore.open") {
                        let (path, _) =
                            system_argument(program, function, &arguments[0], substitutions);
                        let constructor = if name == "DataStore.open" {
                            "disp_data_store_open"
                        } else {
                            "disp_database_open"
                        };
                        format!(
                            "({{{result_c} _r={{0}};disp_native_database _database={{0}};disp_native_string _error={{0}};if({constructor}(({path})->data,({path})->len,&_database,&_error)){{_r.tag=0;_r.payload.v0.f0=_database;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    } else {
                        let constructor = if name == "DataStore.memory" {
                            "disp_data_store_memory"
                        } else {
                            "disp_database_memory"
                        };
                        format!(
                            "({{{result_c} _r={{0}};disp_native_database _database={{0}};disp_native_string _error={{0}};if({constructor}(&_database,&_error)){{_r.tag=0;_r.payload.v0.f0=_database;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("Database.") => {
                    let (database, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "Database.execute" | "Database.query" => {
                            let (sql, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            let (parameters, _) =
                                system_argument(program, function, &arguments[2], substitutions);
                            let result_c = native_types::c_type(&destination_ty);
                            if name == "Database.execute" {
                                format!(
                                    "({{{result_c} _r={{0}};uint64_t _changes=0;disp_native_string _error={{0}};if(disp_database_execute(({database})->state,({sql})->data,({sql})->len,({parameters})->data,({parameters})->len,&_changes,&_error)){{_r.tag=0;_r.payload.v0.f0=_changes;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            } else {
                                let hir::Type::Result(ok, _) = &destination_ty else {
                                    unreachable!()
                                };
                                let list_c = native_types::c_type(ok);
                                format!(
                                    "({{{result_c} _r={{0}};disp_native_json *_rows=NULL;size_t _len=0,_cap=0;disp_native_string _error={{0}};if(disp_database_query(({database})->state,({sql})->data,({sql})->len,({parameters})->data,({parameters})->len,&_rows,&_len,&_cap,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=_rows,.len=_len,.cap=_cap}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                                )
                            }
                        }
                        "Database.begin" | "Database.commit" | "Database.rollback" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let (sql, active) = match name.as_str() {
                                "Database.begin" => ("BEGIN IMMEDIATE", false),
                                "Database.commit" => ("COMMIT", true),
                                _ => ("ROLLBACK", true),
                            };
                            format!(
                                "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_database_control(({database})->state,\"{sql}\",{active},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "Database.close" => {
                            let result_c = native_types::c_type(&destination_ty);
                            format!(
                                "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_database_close({database},&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "Database.changes" => {
                            format!("(uint64_t)sqlite3_changes(({database})->state->handle)")
                        }
                        "Database.last_insert_id" => format!(
                            "(int64_t)sqlite3_last_insert_rowid(({database})->state->handle)"
                        ),
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name == "Process.command" => {
                    let path_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let path = operand(program, function, &arguments[0], &path_ty, substitutions);
                    format!("(disp_native_process_command){{.program={path}}}")
                }
                hir::CallTarget::Intrinsic(name) if name == "Process.run" => {
                    let (path, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let (args, _) =
                        system_argument(program, function, &arguments[1], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    format!(
                        "({{disp_native_process_command _c={{.program=*{path},.args=({args})->data,.args_len=({args})->len,.args_cap=({args})->len}};{result_c} _r={{0}};disp_native_process_output _output={{0}};disp_native_string _error={{0}};disp_runtime_charge_process_start();disp_runtime_acquire_handle();bool _ok=disp_process_run_command(&_c,&_output,&_error);disp_runtime_release_handle();if(_ok){{_r.tag=0;_r.payload.v0.f0=_output;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                    )
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("ProcessCommand.") => {
                    let command_ty = operand_ty(program, function, &arguments[0], substitutions);
                    let command =
                        operand(program, function, &arguments[0], &command_ty, substitutions);
                    if matches!(name.as_str(), "ProcessCommand.run" | "ProcessCommand.start") {
                        let result_c = native_types::c_type(&destination_ty);
                        let command_c = native_types::c_type(&hir::Type::ProcessCommand);
                        if name == "ProcessCommand.run" {
                            format!(
                                "({{{command_c} _c={command};{result_c} _r={{0}};disp_native_process_output _output={{0}};disp_native_string _error={{0}};disp_runtime_charge_process_start();disp_runtime_acquire_handle();bool _ok=disp_process_run_command(&_c,&_output,&_error);disp_runtime_release_handle();if(_ok){{_r.tag=0;_r.payload.v0.f0=_output;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}disp_process_command_drop(&_c);_r;}})"
                            )
                        } else {
                            format!(
                                "({{{command_c} _c={command};{result_c} _r={{0}};disp_native_child_process _child={{0}};disp_native_string _error={{0}};disp_runtime_charge_process_start();disp_runtime_acquire_handle();if(disp_process_start_command(&_c,&_child,&_error)){{_child.state->handle_charged=true;_r.tag=0;_r.payload.v0.f0=_child;}}else{{disp_runtime_release_handle();_r.tag=1;_r.payload.v1.f0=_error;}}disp_process_command_drop(&_c);_r;}})"
                            )
                        }
                    } else {
                        let reserve_args = "size_t _need=_c.args_len+1;if(_need>DISP_PROCESS_MAX_ARGUMENTS)dv_panic(\"process argument count exceeds 4096\",0,0);if(_need>_c.args_cap){size_t _cap=_c.args_cap?_c.args_cap*2:4;if(_cap<_need)_cap=_need;_c.args=(disp_native_string*)disp_realloc(_c.args,_cap*sizeof(disp_native_string),_Alignof(disp_native_string));_c.args_cap=_cap;}";
                        let result = match name.as_str() {
                            "ProcessCommand.arg" => {
                                let actual =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                if matches!(actual, hir::Type::String) {
                                    let value = operand(
                                        program,
                                        function,
                                        &arguments[1],
                                        &actual,
                                        substitutions,
                                    );
                                    format!("{{{reserve_args}_c.args[_c.args_len++]={value};}}")
                                } else {
                                    let (value, _) = system_argument(
                                        program,
                                        function,
                                        &arguments[1],
                                        substitutions,
                                    );
                                    format!(
                                        "{{{reserve_args}_c.args[_c.args_len++]=disp_owned_bytes(({value})->data,({value})->len);}}"
                                    )
                                }
                            }
                            "ProcessCommand.arguments" => {
                                let values_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let values = operand(
                                    program,
                                    function,
                                    &arguments[1],
                                    &values_ty,
                                    substitutions,
                                );
                                "{disp_t_Vs _v=VALUE;size_t _need;if(__builtin_add_overflow(_c.args_len,_v.len,&_need)||_need>DISP_PROCESS_MAX_ARGUMENTS)dv_panic(\"process argument count exceeds 4096\",0,0);if(_need>_c.args_cap){size_t _cap=_c.args_cap?_c.args_cap:4;while(_cap<_need)_cap*=2;_c.args=(disp_native_string*)disp_realloc(_c.args,_cap*sizeof(disp_native_string),_Alignof(disp_native_string));_c.args_cap=_cap;}if(_v.len)memcpy(_c.args+_c.args_len,_v.data,_v.len*sizeof(disp_native_string));_c.args_len=_need;disp_dealloc(_v.data);}".replace("VALUE", &values)
                            }
                            "ProcessCommand.directory" => {
                                let path_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let path = operand(
                                    program,
                                    function,
                                    &arguments[1],
                                    &path_ty,
                                    substitutions,
                                );
                                format!(
                                    "{{if(_c.has_directory)disp_path_drop(&_c.directory);_c.directory={path};_c.has_directory=true;}}"
                                )
                            }
                            "ProcessCommand.environment" => {
                                let key_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let value_ty =
                                    operand_ty(program, function, &arguments[2], substitutions);
                                let key = if matches!(key_ty, hir::Type::String) {
                                    operand(
                                        program,
                                        function,
                                        &arguments[1],
                                        &key_ty,
                                        substitutions,
                                    )
                                } else {
                                    let (value, _) = system_argument(
                                        program,
                                        function,
                                        &arguments[1],
                                        substitutions,
                                    );
                                    format!("disp_owned_bytes(({value})->data,({value})->len)")
                                };
                                let value = if matches!(value_ty, hir::Type::String) {
                                    operand(
                                        program,
                                        function,
                                        &arguments[2],
                                        &value_ty,
                                        substitutions,
                                    )
                                } else {
                                    let (value, _) = system_argument(
                                        program,
                                        function,
                                        &arguments[2],
                                        substitutions,
                                    );
                                    format!("disp_owned_bytes(({value})->data,({value})->len)")
                                };
                                format!(
                                    "{{disp_native_string _key={key},_value={value};size_t _found=SIZE_MAX;for(size_t _i=0;_i<_c.environment_len;_i++)if(_c.environment_keys[_i].len==_key.len&&!memcmp(_c.environment_keys[_i].data,_key.data,_key.len)){{_found=_i;break;}}if(_found!=SIZE_MAX){{disp_string_drop(&_c.environment_values[_found]);_c.environment_values[_found]=_value;disp_string_drop(&_key);}}else{{if(_c.environment_len>=4096)dv_panic(\"process environment override count exceeds 4096\",0,0);size_t _need=_c.environment_len+1;if(_need>_c.environment_cap){{size_t _cap=_c.environment_cap?_c.environment_cap*2:4;_c.environment_keys=(disp_native_string*)disp_realloc(_c.environment_keys,_cap*sizeof(disp_native_string),_Alignof(disp_native_string));_c.environment_values=(disp_native_string*)disp_realloc(_c.environment_values,_cap*sizeof(disp_native_string),_Alignof(disp_native_string));_c.environment_cap=_cap;}}_c.environment_keys[_c.environment_len]=_key;_c.environment_values[_c.environment_len]=_value;_c.environment_len++;}}}}"
                                )
                            }
                            "ProcessCommand.clear_environment" => {
                                "{_c.clear_environment=true;}".into()
                            }
                            "ProcessCommand.input" => {
                                let input_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let input = operand(
                                    program,
                                    function,
                                    &arguments[1],
                                    &input_ty,
                                    substitutions,
                                );
                                "{disp_t_Vu8 _v=VALUE;if(_c.input_cap)disp_dealloc(_c.input);_c.input=_v.data;_c.input_len=_v.len;_c.input_cap=_v.cap;}".replace("VALUE", &input)
                            }
                            "ProcessCommand.input_text" => {
                                let actual =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                if matches!(actual, hir::Type::String) {
                                    let input = operand(
                                        program,
                                        function,
                                        &arguments[1],
                                        &actual,
                                        substitutions,
                                    );
                                    format!(
                                        "{{disp_native_string _v={input};if(_c.input_cap)disp_dealloc(_c.input);_c.input=(uint8_t*)_v.data;_c.input_len=_v.len;_c.input_cap=_v.cap;}}"
                                    )
                                } else {
                                    let (input, _) = system_argument(
                                        program,
                                        function,
                                        &arguments[1],
                                        substitutions,
                                    );
                                    format!(
                                        "{{if(_c.input_cap)disp_dealloc(_c.input);_c.input=NULL;_c.input_len=_c.input_cap=0;if(({input})->len){{_c.input=(uint8_t*)disp_alloc(({input})->len,1);memcpy(_c.input,({input})->data,({input})->len);_c.input_len=_c.input_cap=({input})->len;}}}}"
                                    )
                                }
                            }
                            "ProcessCommand.timeout" => {
                                let timeout_ty =
                                    operand_ty(program, function, &arguments[1], substitutions);
                                let timeout = operand(
                                    program,
                                    function,
                                    &arguments[1],
                                    &timeout_ty,
                                    substitutions,
                                );
                                format!(
                                    "{{_c.timeout_nanos=({timeout}).nanos;_c.has_timeout=true;}}"
                                )
                            }
                            _ => unreachable!(),
                        };
                        format!(
                            "({{{} _c={command};{result}_c;}})",
                            native_types::c_type(&hir::Type::ProcessCommand)
                        )
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("ChildProcess.") => {
                    let (child, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    let result_c = native_types::c_type(&destination_ty);
                    match name.as_str() {
                        "ChildProcess.write" => {
                            let bytes_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let bytes =
                                operand(program, function, &arguments[1], &bytes_ty, substitutions);
                            format!(
                                "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_child_write(({child})->state,(const uint8_t*)({bytes}).data,({bytes}).len,&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "ChildProcess.write_text" => {
                            let (text, _) =
                                system_argument(program, function, &arguments[1], substitutions);
                            format!(
                                "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_child_write(({child})->state,(const uint8_t*)({text})->data,({text})->len,&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                            )
                        }
                        "ChildProcess.close_input" => format!(
                            "({{{result_c} _r={{0}};disp_child_close_input(({child})->state);_r.tag=0;_r;}})"
                        ),
                        "ChildProcess.read_stdout" | "ChildProcess.read_stderr" => {
                            let limit_ty =
                                operand_ty(program, function, &arguments[1], substitutions);
                            let limit =
                                operand(program, function, &arguments[1], &limit_ty, substitutions);
                            let (limit_c, invalid) =
                                if matches!(limit_ty, hir::Type::Int { signed: true, .. }) {
                                    ("__int128", "_limit<0||_limit>16777216")
                                } else {
                                    ("unsigned __int128", "_limit>16777216")
                                };
                            let hir::Type::Result(value, _) = &destination_ty else {
                                unreachable!()
                            };
                            let list_c = native_types::c_type(value);
                            let stdout = name == "ChildProcess.read_stdout";
                            format!(
                                "({{{limit_c} _limit=({limit_c})({limit});if({invalid})dv_panic(\"child-process read limit exceeds 16 MiB\",{},{});{result_c} _r={{0}};uint8_t *_data=NULL;size_t _len=0;disp_native_string _error={{0}};if(disp_child_read(({child})->state,{stdout},(size_t)_limit,&_data,&_len,&_error)){{_r.tag=0;_r.payload.v0.f0=({list_c}){{.data=_data,.len=_len,.cap=_len}};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                                span.start.line, span.start.column
                            )
                        }
                        "ChildProcess.try_wait" => format!(
                            "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_child_update(({child})->state,false,&_error)){{_r.tag=0;if(({child})->state->complete){{_r.payload.v0.f0.tag=1;_r.payload.v0.f0.payload.v1.f0=({child})->state->status;}}}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        ),
                        "ChildProcess.kill" => format!(
                            "({{{result_c} _r={{0}};disp_native_string _error={{0}};if(disp_child_kill(({child})->state,&_error)){{_r.tag=0;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})"
                        ),
                        "ChildProcess.wait" => {
                            let child_ty =
                                operand_ty(program, function, &arguments[0], substitutions);
                            let child_value =
                                operand(program, function, &arguments[0], &child_ty, substitutions);
                            format!(
                                "({{disp_native_child_process _child={child_value};{result_c} _r={{0}};disp_native_process_output _output={{0}};disp_native_string _error={{0}};if(disp_child_wait_output(_child.state,&_output,&_error)){{_r.tag=0;_r.payload.v0.f0=_output;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}disp_child_drop(&_child);_r;}})"
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                hir::CallTarget::Intrinsic(name) if name.starts_with("ProcessOutput.") => {
                    let (output, _) =
                        system_argument(program, function, &arguments[0], substitutions);
                    match name.as_str() {
                        "ProcessOutput.status" => format!("({output})->status"),
                        "ProcessOutput.success" => format!("(({output})->status==0)"),
                        "ProcessOutput.stdout" | "ProcessOutput.stderr" => {
                            let list_c = native_types::c_type(&destination_ty);
                            let field = if name.ends_with("stdout") {
                                "stdout"
                            } else {
                                "stderr"
                            };
                            format!(
                                "({{{list_c} _r={{0}};size_t _len=({output})->{field}_len;if(_len){{_r.data=(uint8_t*)disp_alloc(_len,1);memcpy(_r.data,({output})->{field}_data,_len);_r.len=_r.cap=_len;}}_r;}})"
                            )
                        }
                        "ProcessOutput.stdout_text" | "ProcessOutput.stderr_text" => {
                            let result_c = native_types::c_type(&destination_ty);
                            let field = if name.contains("stdout") {
                                "stdout"
                            } else {
                                "stderr"
                            };
                            format!(
                                "({{{result_c} _r={{0}};const char *_data=(const char*)({output})->{field}_data;size_t _len=({output})->{field}_len;if(disp_utf8_valid(_data,_len)){{_r.tag=0;_r.payload.v0.f0=disp_owned_bytes(_data,_len);}}else{{_r.tag=1;_r.payload.v1.f0=disp_owned_bytes(\"process output is not valid UTF-8\",strlen(\"process output is not valid UTF-8\"));}}_r;}})"
                            )
                        }
                        _ => unreachable!(),
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
                        "Time.ticks" => "((uint32_t)(disp_time_now_nanos()/10000000ULL))".into(),
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
                        "Memory.as_ptr" | "Memory.as_mut_ptr" => format!(
                            "(disp_native_memory_pointer){{.address=({receiver})->data,.base=({receiver})->data,.byte_len=({receiver})->len,.element_size=sizeof(uint8_t),.element_align=_Alignof(uint8_t)}}"
                        ),
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
            if matches!(future_ty, hir::Type::Task(_)) {
                if async_poll {
                    writeln!(
                        output,
                        "if(!disp_task_poll(&({future}),&({destination}),{},{})){{context->pc={block_index};return false;}}goto bb{};",
                        span.start.line, span.start.column, next.0
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "disp_task_wait(&({future}),&({destination}),{},{});goto bb{};",
                        span.start.line, span.start.column, next.0
                    )
                    .unwrap();
                }
            } else if async_poll {
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
        mir::Terminator::Return => output.push_str("disp_runtime_leave_call();return l0;\n"),
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
            if matches!(expected, hir::Type::CFunction(_, _)) {
                return format!(
                    "({}){}",
                    native_types::c_type(expected),
                    mono::mangle(program, &target)
                );
            }
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
            if *operator == ast::UnaryOperator::Negate
                && let mir::Operand::Constant(mir::Constant::Unsigned(magnitude, _)) = value
                && matches!(expected, hir::Type::Int { signed: true, .. })
            {
                let high = (*magnitude >> 64) as u64;
                let low = *magnitude as u64;
                return format!(
                    "({{unsigned __int128 _m=((unsigned __int128){high}ULL<<64)|{low}ULL;({})(_m?(-((__int128)(_m-1))-1):0);}})",
                    native_types::c_type(expected)
                );
            }
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

fn data_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn data_storage_type(ty: &hir::Type) -> &'static str {
    let ty = if let hir::Type::Option(inner) = ty {
        inner.as_ref()
    } else {
        ty
    };
    match ty {
        hir::Type::Float { .. } => "REAL",
        hir::Type::String | hir::Type::Char => "TEXT",
        _ => "INTEGER",
    }
}

fn data_create_sql(schema: &hir::Struct) -> String {
    let fields = schema
        .fields
        .iter()
        .map(|field| {
            let optional = matches!(field.ty, hir::Type::Option(_));
            let mut value = format!(
                "{} {}",
                data_identifier(&field.name),
                data_storage_type(&field.ty)
            );
            if !optional {
                value.push_str(" NOT NULL");
            }
            let inner = if let hir::Type::Option(inner) = &field.ty {
                inner.as_ref()
            } else {
                &field.ty
            };
            if matches!(inner, hir::Type::Bool) {
                let name = data_identifier(&field.name);
                value.push_str(&format!(" CHECK ({name} IN (0,1))"));
            }
            if field.primary {
                value.push_str(" PRIMARY KEY");
            } else if field.unique {
                value.push_str(" UNIQUE");
            }
            value
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({fields})",
        data_identifier(&schema.name)
    )
}

fn data_select_sql(schema: &hir::Struct) -> String {
    schema
        .fields
        .iter()
        .map(|field| data_identifier(&field.name))
        .collect::<Vec<_>>()
        .join(",")
}

fn data_constant_sql(constant: &hir::Constant) -> String {
    match constant {
        hir::Constant::Signed(value, _) => value.to_string(),
        hir::Constant::Unsigned(value, _) => value.to_string(),
        hir::Constant::Float(value, _) => format!("{value:?}"),
        hir::Constant::Bool(value) => if *value { "1" } else { "0" }.into(),
        hir::Constant::Char(value) => format!("'{}'", value.to_string().replace('\'', "''")),
        hir::Constant::String(value) => format!("'{}'", value.replace('\'', "''")),
        hir::Constant::Unit => "NULL".into(),
    }
}

fn data_expr_sql(expression: &hir::DataExpr, schema: &hir::Struct) -> String {
    match &expression.kind {
        hir::DataExprKind::Field(index) => data_identifier(&schema.fields[*index].name),
        hir::DataExprKind::Parameter(_) => "?".into(),
        hir::DataExprKind::Constant(value) => data_constant_sql(value),
        hir::DataExprKind::Unary(operator, operand) => {
            let operand = data_expr_sql(operand, schema);
            match operator {
                ast::UnaryOperator::Not => format!("(NOT {operand})"),
                ast::UnaryOperator::Negate => format!("(-{operand})"),
            }
        }
        hir::DataExprKind::Binary(operator, left, right) => {
            let left = data_expr_sql(left, schema);
            let right = data_expr_sql(right, schema);
            let operator = match operator {
                ast::BinaryOperator::Add => "+",
                ast::BinaryOperator::Subtract => "-",
                ast::BinaryOperator::Multiply => "*",
                ast::BinaryOperator::Divide => "/",
                ast::BinaryOperator::Remainder => "%",
                ast::BinaryOperator::Equal => "=",
                ast::BinaryOperator::NotEqual => "<>",
                ast::BinaryOperator::Less => "<",
                ast::BinaryOperator::LessEqual => "<=",
                ast::BinaryOperator::Greater => ">",
                ast::BinaryOperator::GreaterEqual => ">=",
                ast::BinaryOperator::And => "AND",
                ast::BinaryOperator::Or => "OR",
            };
            format!("({left} {operator} {right})")
        }
    }
}

fn data_parameters(expression: &hir::DataExpr, output: &mut Vec<usize>) {
    match &expression.kind {
        hir::DataExprKind::Parameter(index) => output.push(*index),
        hir::DataExprKind::Unary(_, operand) => data_parameters(operand, output),
        hir::DataExprKind::Binary(_, left, right) => {
            data_parameters(left, output);
            data_parameters(right, output);
        }
        hir::DataExprKind::Field(_) | hir::DataExprKind::Constant(_) => {}
    }
}

enum DataIndexLookup {
    Field {
        field: usize,
        parameter: usize,
    },
    Composite {
        constraint: usize,
        parameters: Vec<usize>,
    },
}

fn data_equality_parameters(expression: &hir::DataExpr, output: &mut HashMap<usize, usize>) {
    let hir::DataExprKind::Binary(operator, left, right) = &expression.kind else {
        return;
    };
    if matches!(operator, ast::BinaryOperator::And) {
        data_equality_parameters(left, output);
        data_equality_parameters(right, output);
    } else if matches!(operator, ast::BinaryOperator::Equal) {
        match (&left.kind, &right.kind) {
            (hir::DataExprKind::Field(field), hir::DataExprKind::Parameter(parameter))
            | (hir::DataExprKind::Parameter(parameter), hir::DataExprKind::Field(field)) => {
                output.entry(*field).or_insert(*parameter);
            }
            _ => {}
        }
    }
}

fn data_index_lookup(
    expression: &hir::DataExpr,
    schema: &hir::Struct,
    parameters: &[usize],
) -> Option<DataIndexLookup> {
    let mut equalities = HashMap::new();
    data_equality_parameters(expression, &mut equalities);
    let parameter_slot = |parameter: &usize| {
        parameters
            .iter()
            .position(|candidate| candidate == parameter)
    };
    for (constraint, declaration) in schema.data_constraints.iter().enumerate() {
        let slots = declaration
            .fields
            .iter()
            .map(|field| equalities.get(field).and_then(parameter_slot))
            .collect::<Option<Vec<_>>>();
        if let Some(parameters) = slots {
            return Some(DataIndexLookup::Composite {
                constraint,
                parameters,
            });
        }
    }
    schema.fields.iter().find_map(|declaration| {
        let parameter = equalities.get(&declaration.index)?;
        (declaration.primary || declaration.unique || declaration.indexed).then(|| {
            parameter_slot(parameter).map(|parameter| DataIndexLookup::Field {
                field: declaration.index,
                parameter,
            })
        })?
    })
}

fn data_constant_native(constant: &hir::Constant) -> String {
    match constant {
        hir::Constant::Signed(value, width) => {
            let bits = *value as u128;
            format!(
                "dv_i((__int128)(((unsigned __int128){}ULL<<64)|{}ULL),{})",
                (bits >> 64) as u64,
                bits as u64,
                width.unwrap_or(64)
            )
        }
        hir::Constant::Unsigned(value, width) => format!(
            "dv_u(((unsigned __int128){}ULL<<64)|{}ULL,{})",
            (*value >> 64) as u64,
            *value as u64,
            width.unwrap_or(64)
        ),
        hir::Constant::Float(value, width) => format!("dv_f({value:?},{width})"),
        hir::Constant::Bool(value) => format!("dv_bool({value})"),
        hir::Constant::Char(value) => format!("dv_char({})", *value as u32),
        hir::Constant::String(value) => {
            format!("dv_string(\"{}\",{})", escape(value), value.len())
        }
        hir::Constant::Unit => "dv_unit()".into(),
    }
}

fn data_expr_native_raw(
    program: &mir::Program,
    function: &mir::Function,
    expression: &hir::DataExpr,
    row: &str,
    arguments: &[mir::Operand],
    substitutions: &HashMap<String, hir::Type>,
) -> Option<String> {
    match expression.kind {
        hir::DataExprKind::Field(index) => Some(format!("(({row})->f{index})")),
        hir::DataExprKind::Parameter(index) => {
            let (value, _) = system_argument(program, function, &arguments[index], substitutions);
            Some(format!("(*({value}))"))
        }
        _ => None,
    }
}

fn data_native_equal(ty: &hir::Type, left: &str, right: &str) -> String {
    match ty {
        hir::Type::Option(inner) => {
            let payload = data_native_equal(
                inner,
                &format!("({left}).payload.v1.f0"),
                &format!("({right}).payload.v1.f0"),
            );
            format!("((({left}).tag==({right}).tag)&&((({left}).tag==0)||({payload})))")
        }
        hir::Type::String | hir::Type::Str => native_key_equal(ty, left, right),
        _ => format!("(({left})==({right}))"),
    }
}

fn data_expr_native(
    program: &mir::Program,
    function: &mir::Function,
    expression: &hir::DataExpr,
    row: &str,
    arguments: &[mir::Operand],
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    match &expression.kind {
        hir::DataExprKind::Field(_) | hir::DataExprKind::Parameter(_) => {
            let raw =
                data_expr_native_raw(program, function, expression, row, arguments, substitutions)
                    .expect("field and parameter data expressions are native values");
            to_dv(&raw, &expression.ty)
        }
        hir::DataExprKind::Constant(value) => data_constant_native(value),
        hir::DataExprKind::Unary(operator, operand) => {
            let operand =
                data_expr_native(program, function, operand, row, arguments, substitutions);
            format!(
                "dv_unary({},{operand},{},{})",
                unary(*operator),
                expression.span.start.line,
                expression.span.start.column
            )
        }
        hir::DataExprKind::Binary(operator, left, right)
            if matches!(operator, ast::BinaryOperator::And | ast::BinaryOperator::Or) =>
        {
            let left = data_expr_native(program, function, left, row, arguments, substitutions);
            let right = data_expr_native(program, function, right, row, arguments, substitutions);
            let operator = if *operator == ast::BinaryOperator::And {
                "&&"
            } else {
                "||"
            };
            format!("dv_bool(dv_truth({left}){operator}dv_truth({right}))")
        }
        hir::DataExprKind::Binary(operator, left, right)
            if matches!(
                operator,
                ast::BinaryOperator::Equal | ast::BinaryOperator::NotEqual
            ) && matches!(left.ty, hir::Type::Option(_)) =>
        {
            let left_raw =
                data_expr_native_raw(program, function, left, row, arguments, substitutions)
                    .expect("Option data comparisons use fields or parameters");
            let right_raw =
                data_expr_native_raw(program, function, right, row, arguments, substitutions)
                    .expect("Option data comparisons use fields or parameters");
            let equal = data_native_equal(&left.ty, &left_raw, &right_raw);
            if *operator == ast::BinaryOperator::Equal {
                format!("dv_bool({equal})")
            } else {
                format!("dv_bool(!({equal}))")
            }
        }
        hir::DataExprKind::Binary(operator, left, right) => {
            let left = data_expr_native(program, function, left, row, arguments, substitutions);
            let right = data_expr_native(program, function, right, row, arguments, substitutions);
            format!(
                "dv_binary({},{left},{right},{},{})",
                binary(*operator),
                expression.span.start.line,
                expression.span.start.column
            )
        }
    }
}

fn data_decode_field(field: &hir::Field, row: &str, target: &str) -> String {
    let key = escape(&field.name);
    let get = format!(
        "disp_native_json _field_{}={{0}};if(!disp_json_get({row},\"{key}\",{},&_field_{})){{const char *_message=\"stored row is missing a DISP Data field\";_error=disp_owned_bytes(_message,strlen(_message));_ok=false;}}",
        field.index,
        field.name.len(),
        field.index
    );
    let decode = match &field.ty {
        hir::Type::Bool => format!(
            "if(_ok){{int64_t _boolean=0;bool _decoded_boolean=false;if(!strcmp(disp_json_kind_name(&_field_{}),\"bool\")){{if(!disp_json_as_bool(&_field_{},&_decoded_boolean,&_error))_ok=false;else _boolean=_decoded_boolean?1:0;}}else if(!disp_json_as_int(&_field_{},&_boolean,&_error)||(_boolean!=0&&_boolean!=1)){{if(!_error.len){{const char *_message=\"stored bool is outside 0 or 1\";_error=disp_owned_bytes(_message,strlen(_message));}}_ok=false;}}if(_ok){target}=(_boolean!=0);}}",
            field.index, field.index, field.index
        ),
        hir::Type::Option(inner) if matches!(inner.as_ref(), hir::Type::Bool) => format!(
            "if(_ok){{if(!strcmp(disp_json_kind_name(&_field_{}),\"null\")){{{target}.tag=0;}}else{{int64_t _boolean=0;bool _decoded_boolean=false;if(!strcmp(disp_json_kind_name(&_field_{}),\"bool\")){{if(!disp_json_as_bool(&_field_{},&_decoded_boolean,&_error))_ok=false;else _boolean=_decoded_boolean?1:0;}}else if(!disp_json_as_int(&_field_{},&_boolean,&_error)||(_boolean!=0&&_boolean!=1)){{if(!_error.len){{const char *_message=\"stored bool is outside 0 or 1\";_error=disp_owned_bytes(_message,strlen(_message));}}_ok=false;}}if(_ok){{{target}.tag=1;{target}.payload.v1.f0=(_boolean!=0);}}}}}}",
            field.index, field.index, field.index, field.index
        ),
        ty => format!(
            "if(_ok&&!{}(&_field_{},&({target}),&_error))_ok=false;",
            json_decoder_name(ty),
            field.index
        ),
    };
    format!(
        "{{{get}{decode}disp_json_drop(&_field_{});if(_ok)_field_done={};}}",
        field.index,
        field.index + 1
    )
}

fn data_migration_json(constant: &hir::Constant) -> String {
    fn quoted(value: &str) -> String {
        let mut output = String::from("\"");
        for character in value.chars() {
            match character {
                '\"' => output.push_str("\\\""),
                '\\' => output.push_str("\\\\"),
                '\u{08}' => output.push_str("\\b"),
                '\u{0c}' => output.push_str("\\f"),
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                character if character < '\u{20}' => {
                    write!(output, "\\u{:04x}", character as u32).unwrap();
                }
                character => output.push(character),
            }
        }
        output.push('\"');
        output
    }

    match constant {
        hir::Constant::Signed(value, _) => value.to_string(),
        hir::Constant::Unsigned(value, _) => value.to_string(),
        hir::Constant::Float(value, _) => format!("{value:?}"),
        hir::Constant::Bool(value) => value.to_string(),
        hir::Constant::Char(value) => quoted(&value.to_string()),
        hir::Constant::String(value) => quoted(value),
        hir::Constant::Unit => unreachable!("migration defaults cannot be unit"),
    }
}

fn data_schema_guard(schema: &hir::Struct, database: &str) -> String {
    let create = data_create_sql(schema);
    let inspect = format!("PRAGMA table_info({})", data_identifier(&schema.name));
    let names = schema
        .fields
        .iter()
        .map(|field| format!("\"{}\"", escape(&field.name)))
        .collect::<Vec<_>>()
        .join(",");
    let types = schema
        .fields
        .iter()
        .map(|field| format!("\"{}\"", data_storage_type(&field.ty)))
        .collect::<Vec<_>>()
        .join(",");
    let required = schema
        .fields
        .iter()
        .map(|field| (!matches!(field.ty, hir::Type::Option(_))).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let primary = schema
        .fields
        .iter()
        .map(|field| field.primary.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let unique = schema
        .fields
        .iter()
        .map(|field| field.unique.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let indexed = schema
        .fields
        .iter()
        .map(|field| field.indexed.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let migration_from = schema
        .fields
        .iter()
        .map(|field| {
            field
                .migration_from
                .as_ref()
                .map_or_else(|| "NULL".into(), |name| format!("\"{}\"", escape(name)))
        })
        .collect::<Vec<_>>()
        .join(",");
    let migration_defaults = schema
        .fields
        .iter()
        .map(|field| {
            field.migration_default.as_ref().map_or_else(
                || "NULL".into(),
                |value| format!("\"{}\"", escape(&data_migration_json(value))),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let migration_default_lens = schema
        .fields
        .iter()
        .map(|field| {
            field
                .migration_default
                .as_ref()
                .map_or(0, |value| data_migration_json(value).len())
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(",");
    let constraint_names = schema
        .data_constraints
        .iter()
        .map(|constraint| format!("\"{}\"", escape(&constraint.name)))
        .chain(schema.data_constraints.is_empty().then_some("NULL".into()))
        .collect::<Vec<_>>()
        .join(",");
    let constraint_unique = schema
        .data_constraints
        .iter()
        .map(|constraint| constraint.unique.to_string())
        .chain(schema.data_constraints.is_empty().then_some("false".into()))
        .collect::<Vec<_>>()
        .join(",");
    let mut offset = 0;
    let mut constraint_offsets = vec!["0".into()];
    let mut constraint_fields = Vec::new();
    for constraint in &schema.data_constraints {
        offset += constraint.fields.len();
        constraint_offsets.push(offset.to_string());
        constraint_fields.extend(constraint.fields.iter().map(ToString::to_string));
    }
    if constraint_fields.is_empty() {
        constraint_fields.push("0".into());
    }
    let constraint_offsets = constraint_offsets.join(",");
    let constraint_fields = constraint_fields.join(",");
    format!(
        "disp_data_ensure_schema(({database})->state,\"{}\",{},\"{}\",{},\"{}\",{},(const char*[]){{{names}}},(const char*[]){{{types}}},(bool[]){{{required}}},(bool[]){{{primary}}},(bool[]){{{unique}}},(bool[]){{{indexed}}},(const char*[]){{{migration_from}}},(const char*[]){{{migration_defaults}}},(size_t[]){{{migration_default_lens}}},{},(const char*[]){{{constraint_names}}},(bool[]){{{constraint_unique}}},(size_t[]){{{constraint_offsets}}},(size_t[]){{{constraint_fields}}},{},&_error)",
        escape(&schema.name),
        schema.name.len(),
        escape(&create),
        create.len(),
        escape(&inspect),
        inspect.len(),
        schema.fields.len(),
        schema.data_constraints.len()
    )
}

fn data_call(
    program: &mir::Program,
    function: &mir::Function,
    plan: hir::DataPlanId,
    arguments: &[mir::Operand],
    destination: &hir::Type,
    substitutions: &HashMap<String, hir::Type>,
) -> String {
    let plan = &program.data_plans[plan.0];
    let schema = &program.structs[plan.schema.0];
    let schema_ty = hir::Type::Struct(plan.schema, vec![]);
    let result_c = native_types::c_type(destination);
    let (database, _) = system_argument(program, function, &arguments[0], substitutions);
    let guard = data_schema_guard(schema, &database);
    match &plan.operation {
        hir::DataOperation::Add { replace } => {
            let (value, _) = system_argument(program, function, &arguments[1], substitutions);
            let count = schema.fields.len();
            let encoders = schema
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    format!(
                        "if(_ok){{if(!{}(&({value}->f{index}),&_params[{index}],&_error))_ok=false;else _encoded++;}}",
                        json_encoder_name(&field.ty)
                    )
                })
                .collect::<String>();
            let names = data_select_sql(schema);
            let keys = schema
                .fields
                .iter()
                .map(|field| {
                    format!(
                        "(disp_native_string){{(char*)\"{}\",{},0}}",
                        escape(&field.name),
                        field.name.len()
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            let placeholders = std::iter::repeat_n("?", count)
                .collect::<Vec<_>>()
                .join(",");
            let mut sql = format!(
                "INSERT INTO {} ({names}) VALUES ({placeholders})",
                data_identifier(&schema.name)
            );
            if *replace {
                let primary = schema
                    .fields
                    .iter()
                    .find(|field| field.primary)
                    .expect("validated data schema has a primary field");
                let primary_name = data_identifier(&primary.name);
                let updates = schema
                    .fields
                    .iter()
                    .filter(|field| !field.primary)
                    .map(|field| {
                        let name = data_identifier(&field.name);
                        format!("{name}=excluded.{name}")
                    })
                    .collect::<Vec<_>>();
                if updates.is_empty() {
                    sql.push_str(&format!(" ON CONFLICT({primary_name}) DO NOTHING"));
                } else {
                    sql.push_str(&format!(
                        " ON CONFLICT({primary_name}) DO UPDATE SET {}",
                        updates.join(",")
                    ));
                }
            }
            format!(
                "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _ok={guard};disp_native_json _params[{}]={{{{0}}}};size_t _encoded=0;{encoders}uint64_t _changes=0;if(_ok&&({database})->state->native){{disp_native_json _row={{0}};_ok=disp_json_from_object((disp_native_string[]){{{keys}}},_params,{count},&_row,&_error);if(_ok)_ok=disp_data_native_write(({database})->state,\"{}\",{},&_row,{},&_changes,&_error);disp_json_drop(&_row);}}else if(_ok)_ok=disp_database_execute(({database})->state,\"{}\",{},_params,{count},&_changes,&_error);for(size_t _i=0;_i<_encoded;_i++)disp_json_drop(&_params[_i]);if(_ok){{_r.tag=0;_r.payload.v0.f0=_changes;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                count.max(1),
                escape(&schema.name),
                schema.name.len(),
                replace,
                escape(&sql),
                sql.len()
            )
        }
        hir::DataOperation::Find {
            predicate,
            order,
            limit_argument,
        } => {
            let mut sql = format!(
                "SELECT {} FROM {}",
                data_select_sql(schema),
                data_identifier(&schema.name)
            );
            let mut indices = Vec::new();
            if let Some(predicate) = predicate {
                sql.push_str(" WHERE ");
                sql.push_str(&data_expr_sql(predicate, schema));
                data_parameters(predicate, &mut indices);
            }
            if let Some((order, descending)) = order {
                sql.push_str(" ORDER BY ");
                sql.push_str(&data_expr_sql(order, schema));
                sql.push_str(if *descending { " DESC" } else { " ASC" });
                data_parameters(order, &mut indices);
            }
            if let Some(index) = limit_argument {
                sql.push_str(" LIMIT ?");
                indices.push(*index);
            }
            let encoders = indices
                .iter()
                .enumerate()
                .map(|(parameter, argument)| {
                    let (value, ty) =
                        system_argument(program, function, &arguments[*argument], substitutions);
                    format!(
                        "if(_ok){{if(!{}({value},&_params[{parameter}],&_error))_ok=false;else _encoded++;}}",
                        json_encoder_name(&ty)
                    )
                })
                .collect::<String>();
            let limit_check = limit_argument.map_or_else(String::new, |index| {
                let (value, ty) =
                    system_argument(program, function, &arguments[index], substitutions);
                let invalid = match ty {
                    hir::Type::Int { signed: true, .. } => {
                        format!("((__int128)(*{value})<0||(unsigned __int128)(*{value})>100000)")
                    }
                    _ => format!("((unsigned __int128)(*{value})>100000)"),
                };
                format!("if(_ok&&{invalid}){{const char *_message=\"DISP Data limit must be between 0 and 100000\";_error=disp_owned_bytes(_message,strlen(_message));_ok=false;}}")
            });
            let parameter_count = indices.len();
            let list_ty = hir::Type::List(Box::new(schema_ty.clone()));
            let list_c = native_types::c_type(&list_ty);
            let element_c = native_types::c_type(&schema_ty);
            let native_predicate = predicate.as_ref().map_or_else(
                || "dv_bool(true)".into(),
                |predicate| {
                    data_expr_native(
                        program,
                        function,
                        predicate,
                        "_row",
                        arguments,
                        substitutions,
                    )
                },
            );
            let native_order = order.as_ref().map_or_else(String::new, |(order, descending)| {
                let left = data_expr_native(
                    program,
                    function,
                    order,
                    "_left",
                    arguments,
                    substitutions,
                );
                let right = data_expr_native(
                    program,
                    function,
                    order,
                    "_right",
                    arguments,
                    substitutions,
                );
                let comparison = if *descending {
                    ast::BinaryOperator::Greater
                } else {
                    ast::BinaryOperator::Less
                };
                format!(
                    "if(_native){{for(size_t _i=1;_i<_values.len;_i++){{{element_c} _moving=_values.data[_i];size_t _at=_i;while(_at){{{element_c} *_left=&_moving,*_right=&_values.data[_at-1];if(!dv_truth(dv_binary({},{left},{right},{},{})))break;_values.data[_at]=_values.data[_at-1];_at--;}}_values.data[_at]=_moving;}}}}",
                    binary(comparison),
                    order.span.start.line,
                    order.span.start.column
                )
            });
            let native_limit = limit_argument.map_or_else(
                || "size_t _native_limit=DISP_DATABASE_ROW_LIMIT;".into(),
                |index| {
                    let (value, _) =
                        system_argument(program, function, &arguments[index], substitutions);
                    format!("size_t _native_limit=(size_t)(*{value});")
                },
            );
            let decoders = schema
                .fields
                .iter()
                .map(|field| {
                    data_decode_field(
                        field,
                        "&_rows[_i]",
                        &format!("_values.data[_target].f{}", field.index),
                    )
                })
                .collect::<String>();
            let partial_drops = schema
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let drop = drop_value(
                        program,
                        &format!("_values.data[_target].f{}", field.index),
                        &field.ty,
                    );
                    format!("if(_field_done>{index}){{{drop}}}")
                })
                .collect::<String>();
            let drop_element = drop_value(program, "_values.data[_i]", &schema_ty);
            let drop_target = drop_value(program, "_values.data[_target]", &schema_ty);
            let native_rows = predicate
                .as_ref()
                .and_then(|predicate| data_index_lookup(predicate, schema, &indices))
                .map_or_else(
                    || {
                        format!(
                            "disp_data_native_snapshot(({database})->state,\"{}\",{},&_rows,&_rows_len,&_rows_cap,&_error)",
                            escape(&schema.name),
                            schema.name.len()
                        )
                    },
                    |lookup| match lookup {
                        DataIndexLookup::Field { field, parameter } => format!(
                            "disp_data_native_lookup(({database})->state,\"{}\",{},{field},&_params[{parameter}],&_rows,&_rows_len,&_rows_cap,&_error)",
                            escape(&schema.name),
                            schema.name.len()
                        ),
                        DataIndexLookup::Composite {
                            constraint,
                            parameters,
                        } => {
                            let keys = parameters
                                .iter()
                                .map(|parameter| format!("_params[{parameter}]"))
                                .collect::<Vec<_>>()
                                .join(",");
                            format!(
                                "disp_data_native_composite_lookup(({database})->state,\"{}\",{},{constraint},(disp_native_json[]){{{keys}}},{},&_rows,&_rows_len,&_rows_cap,&_error)",
                                escape(&schema.name),
                                schema.name.len(),
                                parameters.len()
                            )
                        }
                    },
                );
            format!(
                "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _ok={guard};bool _native=_ok&&({database})->state->native;disp_native_json _params[{}]={{{{0}}}};size_t _encoded=0;{limit_check}{native_limit}{encoders}disp_native_json *_rows=NULL;size_t _rows_len=0,_rows_cap=0;if(_ok){{if(_native)_ok={native_rows};else _ok=disp_database_query(({database})->state,\"{}\",{},_params,{parameter_count},&_rows,&_rows_len,&_rows_cap,&_error);}}{list_c} _values={{0}};if(_ok&&_rows_len){{_values.data=({element_c}*)disp_alloc_zeroed(_rows_len,sizeof({element_c}),_Alignof({element_c}));_values.cap=_rows_len;}}if(_ok){{for(size_t _i=0;_i<_rows_len;_i++){{size_t _target=_values.len,_field_done=0;{decoders}if(!_ok){{{partial_drops}break;}}{element_c} *_row=&_values.data[_target];if(_native&&!dv_truth({native_predicate})){{{drop_target}continue;}}_values.len++;}}}}{native_order}if(_ok&&_native&&_values.len>_native_limit){{for(size_t _i=_native_limit;_i<_values.len;_i++){{{drop_element}}}_values.len=_native_limit;}}disp_database_rows_drop(_rows,_rows_len);for(size_t _i=0;_i<_encoded;_i++)disp_json_drop(&_params[_i]);if(_ok){{_r.tag=0;_r.payload.v0.f0=_values;}}else{{for(size_t _i=0;_i<_values.len;_i++){{{drop_element}}}disp_dealloc(_values.data);_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                parameter_count.max(1),
                escape(&sql),
                sql.len()
            )
        }
        hir::DataOperation::Aggregate {
            kind,
            value,
            predicate,
        } => {
            let mut sql = format!(
                "SELECT {} FROM {}",
                data_select_sql(schema),
                data_identifier(&schema.name)
            );
            let mut indices = Vec::new();
            if let Some(predicate) = predicate {
                sql.push_str(" WHERE ");
                sql.push_str(&data_expr_sql(predicate, schema));
                data_parameters(predicate, &mut indices);
            }
            let encoders = indices
                .iter()
                .enumerate()
                .map(|(parameter, argument)| {
                    let (value, ty) =
                        system_argument(program, function, &arguments[*argument], substitutions);
                    format!(
                        "if(_ok){{if(!{}({value},&_params[{parameter}],&_error))_ok=false;else _encoded++;}}",
                        json_encoder_name(&ty)
                    )
                })
                .collect::<String>();
            let element_c = native_types::c_type(&schema_ty);
            let native_predicate = predicate.as_ref().map_or_else(
                || "dv_bool(true)".into(),
                |predicate| {
                    data_expr_native(
                        program,
                        function,
                        predicate,
                        "_row",
                        arguments,
                        substitutions,
                    )
                },
            );
            let native_value = value.as_ref().map(|value| {
                data_expr_native(program, function, value, "_row", arguments, substitutions)
            });
            let decoders = schema
                .fields
                .iter()
                .map(|field| {
                    data_decode_field(field, "&_rows[_i]", &format!("_row_value.f{}", field.index))
                })
                .collect::<String>();
            let partial_drops = schema
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let drop =
                        drop_value(program, &format!("_row_value.f{}", field.index), &field.ty);
                    format!("if(_field_done>{index}){{{drop}}}")
                })
                .collect::<String>();
            let drop_row = drop_value(program, "_row_value", &schema_ty);
            let native_rows = predicate
                .as_ref()
                .and_then(|predicate| data_index_lookup(predicate, schema, &indices))
                .map_or_else(
                    || {
                        if predicate.is_none()
                            && matches!(kind, ast::DataQueryKind::Count | ast::DataQueryKind::Exists)
                        {
                            format!(
                                "disp_data_native_length(({database})->state,\"{}\",{},&_count,&_error)",
                                escape(&schema.name),
                                schema.name.len()
                            )
                        } else {
                            format!(
                                "disp_data_native_snapshot(({database})->state,\"{}\",{},&_rows,&_rows_len,&_rows_cap,&_error)",
                                escape(&schema.name),
                                schema.name.len()
                            )
                        }
                    },
                    |lookup| match lookup {
                        DataIndexLookup::Field { field, parameter } => format!(
                            "disp_data_native_lookup(({database})->state,\"{}\",{},{field},&_params[{parameter}],&_rows,&_rows_len,&_rows_cap,&_error)",
                            escape(&schema.name),
                            schema.name.len()
                        ),
                        DataIndexLookup::Composite {
                            constraint,
                            parameters,
                        } => {
                            let keys = parameters
                                .iter()
                                .map(|parameter| format!("_params[{parameter}]"))
                                .collect::<Vec<_>>()
                                .join(",");
                            format!(
                                "disp_data_native_composite_lookup(({database})->state,\"{}\",{},{constraint},(disp_native_json[]){{{keys}}},{},&_rows,&_rows_len,&_rows_cap,&_error)",
                                escape(&schema.name),
                                schema.name.len(),
                                parameters.len()
                            )
                        }
                    },
                );
            let value_step = match kind {
                ast::DataQueryKind::Count => "_count++;".into(),
                ast::DataQueryKind::Exists => "_count++;if(_count)break;".into(),
                ast::DataQueryKind::Sum => format!(
                    "DV _next={};if(!_has)_aggregate=_next;else _aggregate=dv_binary(0,_aggregate,_next,{},{});_has=true;_count++;",
                    native_value.as_ref().expect("sum has a value"),
                    value.as_ref().unwrap().span.start.line,
                    value.as_ref().unwrap().span.start.column
                ),
                ast::DataQueryKind::Average => format!(
                    "DV _next={};_average+=_next.tag==DV_FLOAT?_next.as.fp:_next.tag==DV_SIGNED?(double)_next.as.si:(double)_next.as.ui;_has=true;_count++;",
                    native_value.as_ref().expect("average has a value")
                ),
                ast::DataQueryKind::Min | ast::DataQueryKind::Max => {
                    let comparison = if *kind == ast::DataQueryKind::Min {
                        8
                    } else {
                        10
                    };
                    format!(
                        "DV _next={};if(!_has||dv_truth(dv_binary({comparison},_next,_aggregate,{},{})))_aggregate=_next;_has=true;_count++;",
                        native_value.as_ref().expect("min/max has a value"),
                        value.as_ref().unwrap().span.start.line,
                        value.as_ref().unwrap().span.start.column
                    )
                }
                ast::DataQueryKind::Rows => unreachable!("row query uses Find"),
            };
            let hir::Type::Result(output_ty, _) = destination else {
                unreachable!("data aggregate returns Result")
            };
            let output_c = native_types::c_type(output_ty);
            let (success_setup, success) = match kind {
                ast::DataQueryKind::Count => (String::new(), "_count".into()),
                ast::DataQueryKind::Exists => (String::new(), "(_count!=0)".into()),
                ast::DataQueryKind::Sum => {
                    let aggregate_ty = &value.as_ref().expect("sum has a value").ty;
                    let zero = match aggregate_ty {
                        hir::Type::Float { width } => format!("dv_f(0.0,{width})"),
                        hir::Type::Int {
                            signed: true,
                            width,
                        } => {
                            format!("dv_i(0,{})", width.unwrap_or(64))
                        }
                        hir::Type::Int {
                            signed: false,
                            width,
                        } => {
                            format!("dv_u(0,{})", width.unwrap_or(64))
                        }
                        _ => unreachable!("type checker restricts aggregates to numeric values"),
                    };
                    (
                        format!("if(!_has)_aggregate={zero};"),
                        from_dv("_aggregate", aggregate_ty),
                    )
                }
                ast::DataQueryKind::Average => (
                    format!(
                        "{output_c} _aggregate_result={{0}};if(_has){{_aggregate_result.tag=1;_aggregate_result.payload.v1.f0=_average/(double)_count;}}"
                    ),
                    "_aggregate_result".into(),
                ),
                ast::DataQueryKind::Min | ast::DataQueryKind::Max => {
                    let aggregate_ty = &value.as_ref().expect("min/max has a value").ty;
                    (
                        format!(
                            "{output_c} _aggregate_result={{0}};if(_has){{_aggregate_result.tag=1;_aggregate_result.payload.v1.f0={};}}",
                            from_dv("_aggregate", aggregate_ty)
                        ),
                        "_aggregate_result".into(),
                    )
                }
                ast::DataQueryKind::Rows => unreachable!("row query uses Find"),
            };
            let parameter_count = indices.len();
            let direct_native = predicate.is_none()
                && matches!(kind, ast::DataQueryKind::Count | ast::DataQueryKind::Exists);
            format!(
                "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _ok={guard};bool _native=_ok&&({database})->state->native;disp_native_json _params[{}]={{{{0}}}};size_t _encoded=0;{encoders}disp_native_json *_rows=NULL;size_t _rows_len=0,_rows_cap=0;uint64_t _count=0;bool _has=false;double _average=0.0;DV _aggregate=dv_i(0,64);if(_ok){{if(_native)_ok={native_rows};else _ok=disp_database_query(({database})->state,\"{}\",{},_params,{parameter_count},&_rows,&_rows_len,&_rows_cap,&_error);}}if(_ok&&!(_native&&{direct_native})){{for(size_t _i=0;_i<_rows_len;_i++){{{element_c} _row_value={{0}};size_t _field_done=0;{decoders}if(!_ok){{{partial_drops}break;}}{element_c} *_row=&_row_value;if(!_native||dv_truth({native_predicate})){{{value_step}}}{drop_row}}}}}disp_database_rows_drop(_rows,_rows_len);for(size_t _i=0;_i<_encoded;_i++)disp_json_drop(&_params[_i]);{success_setup}if(_ok){{_r.tag=0;_r.payload.v0.f0={success};}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                parameter_count.max(1),
                escape(&sql),
                sql.len()
            )
        }
        hir::DataOperation::Remove { predicate } => {
            let mut indices = Vec::new();
            data_parameters(predicate, &mut indices);
            let encoders = indices
                .iter()
                .enumerate()
                .map(|(parameter, argument)| {
                    let (value, ty) =
                        system_argument(program, function, &arguments[*argument], substitutions);
                    format!(
                        "if(_ok){{if(!{}({value},&_params[{parameter}],&_error))_ok=false;else _encoded++;}}",
                        json_encoder_name(&ty)
                    )
                })
                .collect::<String>();
            let sql = format!(
                "DELETE FROM {} WHERE {}",
                data_identifier(&schema.name),
                data_expr_sql(predicate, schema)
            );
            let count = indices.len();
            let element_c = native_types::c_type(&schema_ty);
            let native_predicate = data_expr_native(
                program,
                function,
                predicate,
                "_row",
                arguments,
                substitutions,
            );
            let decoders = schema
                .fields
                .iter()
                .map(|field| {
                    data_decode_field(field, "&_rows[_i]", &format!("_value.f{}", field.index))
                })
                .collect::<String>();
            let partial_drops = schema
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let drop = drop_value(program, &format!("_value.f{}", field.index), &field.ty);
                    format!("if(_field_done>{index}){{{drop}}}")
                })
                .collect::<String>();
            let drop_value = drop_value(program, "_value", &schema_ty);
            format!(
                "({{{result_c} _r={{0}};disp_native_string _error={{0}};bool _ok={guard};bool _native=_ok&&({database})->state->native;disp_native_json _params[{}]={{{{0}}}};size_t _encoded=0;{encoders}uint64_t _changes=0;if(_ok&&_native){{disp_native_json *_rows=NULL;size_t _rows_len=0,_rows_cap=0;_ok=disp_data_native_snapshot(({database})->state,\"{}\",{},&_rows,&_rows_len,&_rows_cap,&_error);bool *_remove=_rows_len?(bool*)disp_alloc_zeroed(_rows_len,sizeof(bool),_Alignof(bool)):NULL;if(_ok){{for(size_t _i=0;_i<_rows_len;_i++){{{element_c} _value={{0}};size_t _field_done=0;{decoders}if(!_ok){{{partial_drops}break;}}{element_c} *_row=&_value;_remove[_i]=dv_truth({native_predicate});{drop_value}}}}}if(_ok)_ok=disp_data_native_delete(({database})->state,\"{}\",{},_remove,_rows_len,&_changes,&_error);disp_dealloc(_remove);disp_database_rows_drop(_rows,_rows_len);}}else if(_ok)_ok=disp_database_execute(({database})->state,\"{}\",{},_params,{count},&_changes,&_error);for(size_t _i=0;_i<_encoded;_i++)disp_json_drop(&_params[_i]);if(_ok){{_r.tag=0;_r.payload.v0.f0=_changes;}}else{{_r.tag=1;_r.payload.v1.f0=_error;}}_r;}})",
                count.max(1),
                escape(&schema.name),
                schema.name.len(),
                escape(&schema.name),
                schema.name.len(),
                escape(&sql),
                sql.len()
            )
        }
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
        hir::Type::SecretBytes => "dv_string(\"<SecretBytes:redacted>\",22)".into(),
        hir::Type::AeadEnvelope => "dv_string(\"<AeadEnvelope>\",14)".into(),
        hir::Type::Ed25519SigningKey => "dv_string(\"<Ed25519SigningKey:redacted>\",28)".into(),
        hir::Type::Path => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::Url => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::Json => format!("dv_string(({value}).data,({value}).len)"),
        hir::Type::IpAddress => format!("dv_ip({value})"),
        hir::Type::SocketAddress => "dv_string(\"<SocketAddress>\",15)".into(),
        hir::Type::TcpStream => "dv_string(\"<TcpStream>\",11)".into(),
        hir::Type::TlsStream => "dv_string(\"<TlsStream>\",11)".into(),
        hir::Type::HttpRequest => "dv_string(\"<HttpRequest>\",13)".into(),
        hir::Type::HttpResponse => "dv_string(\"<HttpResponse>\",14)".into(),
        hir::Type::TcpListener => "dv_string(\"<TcpListener>\",13)".into(),
        hir::Type::UdpSocket => "dv_string(\"<UdpSocket>\",11)".into(),
        hir::Type::UdpDatagram => "dv_string(\"<UdpDatagram>\",13)".into(),
        hir::Type::Instant | hir::Type::Duration => {
            format!("dv_u((unsigned __int128)({value}).nanos,64)")
        }
        hir::Type::ProcessOutput => "dv_string(\"<ProcessOutput>\",15)".into(),
        hir::Type::ProcessCommand => "dv_string(\"<ProcessCommand>\",16)".into(),
        hir::Type::ChildProcess => "dv_string(\"<ChildProcess>\",14)".into(),
        hir::Type::Database => "dv_string(\"<Database>\",10)".into(),
        hir::Type::DataStore => "dv_string(\"<DataStore>\",11)".into(),
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
        hir::Type::SecretBytes => format!("disp_secret_drop(&({value}));"),
        hir::Type::AeadEnvelope => format!("disp_string_drop(&({value}));"),
        hir::Type::Ed25519SigningKey => format!("disp_secret_drop(&({value}));"),
        hir::Type::Path => format!("disp_path_drop(&({value}));"),
        hir::Type::Url => format!("disp_url_drop(&({value}));"),
        hir::Type::Json => format!("disp_json_drop(&({value}));"),
        hir::Type::Generic(_) => format!("disp_string_drop(&({value}));"),
        hir::Type::ProcessOutput => format!(
            "{{disp_dealloc(({value}).stdout_data);disp_dealloc(({value}).stderr_data);({value})=(disp_native_process_output){{0}};}}"
        ),
        hir::Type::ProcessCommand => format!("disp_process_command_drop(&({value}));"),
        hir::Type::ChildProcess => format!("disp_child_drop(&({value}));"),
        hir::Type::Database | hir::Type::DataStore => {
            format!("disp_database_drop(&({value}));")
        }
        hir::Type::SocketAddress => format!("disp_socket_address_drop(&({value}));"),
        hir::Type::TcpStream => format!("disp_tcp_stream_drop(&({value}));"),
        hir::Type::TlsStream => format!("disp_tls_stream_drop(&({value}));"),
        hir::Type::HttpRequest => format!("disp_http_builder_drop(&({value}));"),
        hir::Type::HttpResponse => format!("disp_http_response_drop(&({value}));"),
        hir::Type::TcpListener => format!("disp_tcp_listener_drop(&({value}));"),
        hir::Type::UdpSocket => format!("disp_udp_socket_drop(&({value}));"),
        hir::Type::UdpDatagram => format!("disp_udp_datagram_drop(&({value}));"),
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
        hir::Type::Channel(value_ty) => {
            let value_c = native_types::c_type(value_ty);
            let index = format!("_drop_i{depth}");
            let queued = format!(
                "(({value_c}*)({value}).state->data)[(({value}).state->head+{index})%({value}).state->capacity]"
            );
            let queued_drop = drop_value_depth(program, &queued, value_ty, depth + 1);
            format!(
                "{{if(({value}).state&&disp_channel_release(({value}).state)){{for(size_t {index}=0;{index}<({value}).state->len;{index}++){{{queued_drop}}}disp_dealloc(({value}).state->data);disp_dealloc(({value}).state);}}({value}).state=NULL;}}"
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
        hir::Type::CRegistration => {
            format!("disp_c_registration_close(&({value}));")
        }
        hir::Type::Future(_) => format!(
            "{{if(({value}).drop)({value}).drop(({value}).context);({value})=(disp_native_future){{0}};}}"
        ),
        hir::Type::Task(_) => format!("disp_task_drop(&({value}));"),
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
