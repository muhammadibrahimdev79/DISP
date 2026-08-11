use super::{
    layout::{self, Layout, LayoutEngine},
    mono,
    target::Target,
};
use crate::{diagnostics::Diagnostic, hir, mir};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassMode {
    Ignore,
    Direct,
    Indirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionAbi {
    pub arguments: Vec<PassMode>,
    pub result: PassMode,
}

#[derive(Debug, Clone, Default)]
pub struct AbiProgram {
    pub functions: BTreeMap<mono::FunctionInstance, FunctionAbi>,
}

pub fn classify(_ty: &hir::Type, layout: &Layout, _target: Target) -> PassMode {
    if layout.size == 0 {
        return PassMode::Ignore;
    }
    if matches!(layout.size, 1 | 2 | 4 | 8) {
        PassMode::Direct
    } else {
        PassMode::Indirect
    }
}

pub fn lower(
    hir: &hir::Program,
    mir: &mir::Program,
    mono_program: &mono::MonoProgram,
    target: Target,
) -> Result<AbiProgram, Diagnostic> {
    let mut engine = LayoutEngine::new(target, hir);
    let mut functions = BTreeMap::new();
    for instance in &mono_program.instances {
        let function = &mir.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        let mut arguments = Vec::with_capacity(function.argument_count);
        for local in function.locals.iter().skip(1).take(function.argument_count) {
            let ty = layout::substitute(&local.ty, &substitutions);
            arguments.push(classify(&ty, &engine.layout(&ty)?, target));
        }
        let result_ty =
            layout::substitute(&function.locals[function.return_local.0].ty, &substitutions);
        let result = classify(&result_ty, &engine.layout(&result_ty)?, target);
        functions.insert(instance.clone(), FunctionAbi { arguments, result });
    }
    Ok(AbiProgram { functions })
}
