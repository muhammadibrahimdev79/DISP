use crate::ast::{
    AssignmentOperator, BinaryOperator, BindingKind, Block, Capability, CapabilityUse, Expr,
    Expression, Function, Pattern, Program, Statement, TypeName, TypeQualifier, UnaryOperator,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    IntLiteral(u128),
    NegativeIntLiteral(i128),
    UInt,
    Signed(u16),
    Unsigned(u16),
    Float,
    Float32,
    FloatLiteral,
    String,
    Str,
    CString,
    CStr,
    Memory,
    SecretBytes,
    AeadEnvelope,
    Ed25519SigningKey,
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
    ProcessCommand,
    ChildProcess,
    ProcessOutput,
    Database,
    DataStore,
    Array(Box<Type>, usize),
    Slice(Box<Type>),
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Thread(Box<Type>),
    Future(Box<Type>),
    Task(Box<Type>),
    Mutex(Box<Type>),
    MutexGuard(Box<Type>),
    Channel(Box<Type>),
    AtomicInt,
    Char,
    Bool,
    ConversionError,
    IoError,
    NetworkError,
    HttpError,
    DataError,
    CryptoError,
    Unit,
    Struct(TypeId, Vec<Type>),
    Enum(TypeId, Vec<Type>),
    Generic(String),
    Associated(String),
    Reference(Box<Type>, bool),
    RawPointer(Box<Type>, bool),
    MemoryPointer(Box<Type>, bool),
    Option(Box<Type>),
    Result(Box<Type>, Box<Type>),
    Function(Vec<Type>, Box<Type>),
    CFunction(Vec<Type>, Box<Type>),
    CRegistration,
    Infer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(usize);

#[derive(Debug, Clone)]
struct StructInfo {
    name: String,
    data: bool,
    c_abi: bool,
    generics: Vec<String>,
    constraints: Vec<Vec<String>>,
    fields: HashMap<String, Type>,
    primary: Option<Type>,
}

#[derive(Debug, Clone)]
struct VariantInfo {
    owner: TypeId,
    payload: Vec<Type>,
}

#[derive(Debug, Clone)]
struct EnumInfo {
    name: String,
    generics: Vec<String>,
    constraints: Vec<Vec<String>>,
    variants: HashMap<String, VariantInfo>,
}

#[derive(Debug, Clone)]
struct Variable {
    ty: Type,
    constant: bool,
}

#[derive(Debug, Clone)]
struct Signature {
    asynchronous: bool,
    generics: Vec<String>,
    constraints: HashMap<String, Vec<String>>,
    parameters: Vec<Type>,
    result: Type,
    capabilities: Option<HashSet<Capability>>,
}

#[derive(Debug, Clone)]
struct TraitInfo {
    generics: Vec<String>,
    constraints: Vec<Vec<String>>,
    methods: HashMap<String, Signature>,
    associated_types: HashSet<String>,
}

#[derive(Debug, Clone)]
struct ImplInfo {
    trait_name: String,
    trait_arguments: Vec<Type>,
    target: Type,
    constraints: HashMap<String, Vec<String>>,
    associated_types: HashMap<String, Type>,
}

pub struct TypeChecker {
    functions: HashMap<String, Signature>,
    types: HashMap<String, Type>,
    structs: HashMap<TypeId, StructInfo>,
    enums: HashMap<TypeId, EnumInfo>,
    variants: HashMap<String, Vec<VariantInfo>>,
    scopes: Vec<HashMap<String, Variable>>,
    expected_return: Type,
    generic_types: HashMap<String, Vec<String>>,
    traits: HashMap<String, TraitInfo>,
    implementations: Vec<ImplInfo>,
    external_functions: HashSet<String>,
    exported_functions: HashSet<String>,
    unsafe_depth: usize,
    unsafe_contracts: Vec<Option<HashSet<Capability>>>,
    async_depth: usize,
    data_context: Option<TypeId>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
            types: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            variants: HashMap::new(),
            scopes: Vec::new(),
            expected_return: Type::Unit,
            generic_types: HashMap::new(),
            traits: HashMap::new(),
            implementations: Vec::new(),
            external_functions: HashSet::new(),
            exported_functions: HashSet::new(),
            unsafe_depth: 0,
            unsafe_contracts: Vec::new(),
            async_depth: 0,
            data_context: None,
        }
    }

    pub fn check(&mut self, program: &Program) -> Result<(), Diagnostic> {
        self.functions.clear();
        self.types.clear();
        self.structs.clear();
        self.enums.clear();
        self.variants.clear();
        self.traits.clear();
        self.implementations.clear();
        self.external_functions.clear();
        self.exported_functions.clear();
        self.unsafe_depth = 0;
        self.unsafe_contracts.clear();
        self.traits.insert(
            "Copy".into(),
            TraitInfo {
                generics: vec![],
                constraints: vec![],
                methods: HashMap::new(),
                associated_types: HashSet::new(),
            },
        );

        let mut next_type_id = 0usize;
        for declaration in &program.structs {
            let id = TypeId(next_type_id);
            next_type_id += 1;
            self.types
                .insert(declaration.name.clone(), Type::Struct(id, vec![]));
            self.structs.insert(
                id,
                StructInfo {
                    name: declaration.name.clone(),
                    data: declaration.data,
                    c_abi: declaration.c_abi,
                    generics: declaration
                        .generics
                        .iter()
                        .map(|p| p.name.clone())
                        .collect(),
                    constraints: declaration
                        .generics
                        .iter()
                        .map(|parameter| {
                            parameter
                                .constraints
                                .iter()
                                .map(|constraint| constraint.name.clone())
                                .collect()
                        })
                        .collect(),
                    fields: HashMap::new(),
                    primary: None,
                },
            );
        }
        for declaration in &program.enums {
            let id = TypeId(next_type_id);
            next_type_id += 1;
            self.types
                .insert(declaration.name.clone(), Type::Enum(id, vec![]));
            self.enums.insert(
                id,
                EnumInfo {
                    name: declaration.name.clone(),
                    generics: declaration
                        .generics
                        .iter()
                        .map(|p| p.name.clone())
                        .collect(),
                    constraints: declaration
                        .generics
                        .iter()
                        .map(|parameter| {
                            parameter
                                .constraints
                                .iter()
                                .map(|constraint| constraint.name.clone())
                                .collect()
                        })
                        .collect(),
                    variants: HashMap::new(),
                },
            );
        }
        for declaration in &program.structs {
            let Type::Struct(id, _) = self.types[&declaration.name] else {
                unreachable!();
            };
            self.set_generics(&declaration.generics);
            if declaration.data && !declaration.generics.is_empty() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "data schemas cannot be generic",
                    declaration.name_span,
                ));
            }
            if declaration.data && declaration.fields.is_empty() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "a data schema must declare at least one field",
                    declaration.name_span,
                ));
            }
            let mut fields = HashMap::new();
            let mut primary = None;
            for field in &declaration.fields {
                let ty = self.resolve_type(&field.ty)?;
                if declaration.data {
                    self.ensure_data_field_type(&ty, field.ty.span)?;
                    if field.primary {
                        if primary.is_some() {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "a data schema must have exactly one primary field",
                                field.name_span,
                            ));
                        }
                        if matches!(ty, Type::Option(_)) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "a primary data field cannot be optional",
                                field.ty.span,
                            ));
                        }
                        if !matches!(ty, Type::Int | Type::Signed(_) | Type::String) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "a primary data field must be a signed integer or String",
                                field.ty.span,
                            ));
                        }
                        primary = Some(ty.clone());
                    }
                    if field.unique && matches!(ty, Type::Option(_)) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "a unique data field cannot be optional",
                            field.ty.span,
                        )
                        .with_help(
                            "use a required field so uniqueness has one unambiguous value domain",
                        ));
                    }
                }
                if type_contains_task(&ty) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "Task<T> cannot be stored in a struct",
                        field.ty.span,
                    )
                    .with_help("keep spawned tasks in the async scope that owns them and await or cancel them there"));
                }
                fields.insert(field.name.clone(), ty);
            }
            if declaration.data && primary.is_none() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "a data schema must mark exactly one field as `primary`",
                    declaration.name_span,
                ));
            }
            let info = self.structs.get_mut(&id).unwrap();
            info.fields = fields;
            info.primary = primary;
        }
        for declaration in &program.structs {
            if declaration.c_abi {
                self.validate_c_abi_struct(declaration)?;
            }
        }
        for declaration in &program.enums {
            let Type::Enum(id, _) = self.types[&declaration.name] else {
                unreachable!();
            };
            self.set_generics(&declaration.generics);
            let mut variants = HashMap::new();
            for variant in &declaration.variants {
                let info = VariantInfo {
                    owner: id,
                    payload: variant
                        .payload
                        .iter()
                        .map(|ty| {
                            let resolved = self.resolve_type(ty)?;
                            if type_contains_task(&resolved) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Task<T> cannot be stored in an enum",
                                    ty.span,
                                )
                                .with_help("keep spawned tasks in the async scope that owns them and await or cancel them there"));
                            }
                            Ok(resolved)
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };
                self.variants
                    .entry(variant.name.clone())
                    .or_default()
                    .push(info.clone());
                variants.insert(variant.name.clone(), info);
            }
            self.enums.get_mut(&id).unwrap().variants = variants;
        }

        for declaration in &program.traits {
            if self.traits.contains_key(&declaration.name)
                || self.types.contains_key(&declaration.name)
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("duplicate trait `{}`", declaration.name),
                    declaration.name_span,
                ));
            }
            let associated_types = declaration
                .associated_types
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<HashSet<_>>();
            if associated_types.len() != declaration.associated_types.len() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "trait `{}` declares an associated type more than once",
                        declaration.name
                    ),
                    declaration.span,
                ));
            }
            let mut methods = HashMap::new();
            for method in &declaration.methods {
                let mut method_generics = declaration.generics.clone();
                method_generics.extend(method.generics.clone());
                self.set_generics(&method_generics);
                self.generic_types.insert("Self".into(), vec![]);
                let signature = Signature {
                    asynchronous: method.asynchronous,
                    generics: method.generics.iter().map(|p| p.name.clone()).collect(),
                    constraints: method
                        .generics
                        .iter()
                        .map(|p| {
                            (
                                p.name.clone(),
                                p.constraints.iter().map(|c| c.name.clone()).collect(),
                            )
                        })
                        .collect(),
                    parameters: method
                        .parameters
                        .iter()
                        .map(|p| self.resolve_type(&p.ty))
                        .collect::<Result<_, _>>()?,
                    result: method
                        .return_type
                        .as_ref()
                        .map(|ty| self.resolve_type(ty))
                        .transpose()?
                        .unwrap_or(Type::Unit),
                    capabilities: capability_set(&method.capabilities),
                };
                for ty in signature.parameters.iter().chain([&signature.result]) {
                    validate_associated_references(ty, &associated_types, method.span)?;
                }
                if methods.insert(method.name.clone(), signature).is_some() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("duplicate trait method `{}`", method.name),
                        method.name_span,
                    ));
                }
            }
            self.traits.insert(
                declaration.name.clone(),
                TraitInfo {
                    generics: declaration
                        .generics
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                    constraints: declaration
                        .generics
                        .iter()
                        .map(|parameter| {
                            parameter
                                .constraints
                                .iter()
                                .map(|constraint| constraint.name.clone())
                                .collect()
                        })
                        .collect(),
                    methods,
                    associated_types,
                },
            );
        }

        for implementation in &program.implementations {
            self.set_generics(&implementation.generics);
            let target = self.resolve_type(&implementation.target)?;
            let trait_name = implementation.trait_name.as_ref().ok_or_else(|| Diagnostic::new(DiagnosticKind::Type, "inherent implementations are not available until constructor-method semantics are finalized", implementation.span))?.name.clone();
            let trait_arguments = implementation
                .trait_name
                .as_ref()
                .unwrap()
                .arguments
                .iter()
                .map(|argument| self.resolve_type(argument))
                .collect::<Result<Vec<_>, _>>()?;
            for generic in &implementation.generics {
                if !type_contains_generic_name(&target, &generic.name)
                    && !trait_arguments
                        .iter()
                        .any(|argument| type_contains_generic_name(argument, &generic.name))
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "implementation generic `{}` is not constrained by the target or trait arguments",
                            generic.name
                        ),
                        generic.name_span,
                    ));
                }
            }
            let trait_info = self.traits.get(&trait_name).cloned().ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("unknown trait `{trait_name}`"),
                    implementation.trait_name.as_ref().unwrap().span,
                )
            })?;
            if self.implementations.iter().any(|existing| {
                existing.trait_name == trait_name && types_overlap(&existing.target, &target)
            }) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "conflicting implementation of `{trait_name}` for {}",
                        self.format_type(&target)
                    ),
                    implementation.span,
                ));
            }
            let associated: HashSet<_> = implementation
                .associated_types
                .iter()
                .map(|(name, _, _)| name.clone())
                .collect();
            if associated.len() != implementation.associated_types.len() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "implementation of `{trait_name}` defines an associated type more than once"
                    ),
                    implementation.span,
                ));
            }
            if associated != trait_info.associated_types {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "implementation of `{trait_name}` must define exactly its associated types"
                    ),
                    implementation.span,
                ));
            }
            for (_, ty, _) in &implementation.associated_types {
                self.resolve_type(ty)?;
            }
            let associated_types = implementation
                .associated_types
                .iter()
                .map(|(name, ty, _)| Ok((name.clone(), self.resolve_type(ty)?)))
                .collect::<Result<HashMap<_, _>, Diagnostic>>()?;
            let methods: HashMap<_, _> = implementation
                .methods
                .iter()
                .map(|method| (method.name.clone(), method.clone()))
                .collect();
            if methods.len() != implementation.methods.len()
                || methods.keys().cloned().collect::<HashSet<_>>()
                    != trait_info.methods.keys().cloned().collect()
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "implementation of `{trait_name}` must define exactly its declared methods"
                    ),
                    implementation.span,
                ));
            }
            self.implementations.push(ImplInfo {
                trait_name,
                trait_arguments,
                target,
                constraints: implementation
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
                    .collect(),
                associated_types,
            });
        }

        for function in &program.functions {
            self.set_generics(&function.generics);
            let parameters = function
                .parameters
                .iter()
                .map(|parameter| self.resolve_type(&parameter.ty))
                .collect::<Result<Vec<_>, _>>()?;
            let result = function
                .return_type
                .as_ref()
                .map(|ty| self.resolve_type(ty))
                .transpose()?
                .unwrap_or(Type::Unit);
            if function.external.is_some() {
                self.validate_external_signature(function, &parameters, &result)?;
                self.external_functions.insert(function.name.clone());
            }
            if function.exported {
                self.validate_export_signature(function, &parameters, &result)?;
                self.exported_functions.insert(function.name.clone());
            }
            if function.asynchronous
                && (parameters.iter().any(type_crosses_thread_by_borrow)
                    || type_crosses_thread_by_borrow(&result))
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "async function `{}` cannot capture a borrowed or raw-pointer type",
                        function.name
                    ),
                    function.span,
                )
                .with_help("pass owned data into async functions and return owned results"));
            }
            self.functions.insert(
                function.name.clone(),
                Signature {
                    asynchronous: function.asynchronous,
                    generics: function.generics.iter().map(|p| p.name.clone()).collect(),
                    constraints: function
                        .generics
                        .iter()
                        .map(|p| {
                            (
                                p.name.clone(),
                                p.constraints.iter().map(|c| c.name.clone()).collect(),
                            )
                        })
                        .collect(),
                    parameters,
                    result,
                    capabilities: capability_set(&function.capabilities),
                },
            );
        }

        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Type,
                    "missing `main` function",
                    Span::point(1, 1),
                )
            })?;
        let main_parameters_valid = main.parameters.is_empty()
            || (main.parameters.len() == 1
                && matches!(
                    self.functions["main"].parameters.as_slice(),
                    [Type::List(element)] if matches!(**element, Type::String)
                ));
        if !main_parameters_valid || main.return_type.is_some() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "`main` must have signature `fn main()` or `fn main(args: List<String>)`",
                main.name_span,
            ));
        }

        for implementation in program.implementations.clone() {
            self.set_generics(&implementation.generics);
            let target = self.resolve_type(&implementation.target)?;
            let trait_name = implementation.trait_name.as_ref().unwrap().name.clone();
            let trait_info = self.traits[&trait_name].clone();
            let trait_arguments = implementation
                .trait_name
                .as_ref()
                .unwrap()
                .arguments
                .iter()
                .map(|argument| self.resolve_type(argument))
                .collect::<Result<Vec<_>, _>>()?;
            self.require_constraints(
                &trait_info.constraints,
                &trait_arguments,
                implementation.trait_name.as_ref().unwrap().span,
            )?;
            let associated_types = implementation
                .associated_types
                .iter()
                .map(|(name, ty, _)| Ok((name.clone(), self.resolve_type(ty)?)))
                .collect::<Result<HashMap<_, _>, Diagnostic>>()?;
            for method in &implementation.methods {
                let mut method_generics = implementation.generics.clone();
                method_generics.extend(method.generics.clone());
                self.set_generics(&method_generics);
                let expected = trait_info.methods[&method.name].clone();
                let actual = Signature {
                    asynchronous: method.asynchronous,
                    generics: method.generics.iter().map(|p| p.name.clone()).collect(),
                    constraints: method
                        .generics
                        .iter()
                        .map(|p| {
                            (
                                p.name.clone(),
                                p.constraints.iter().map(|c| c.name.clone()).collect(),
                            )
                        })
                        .collect(),
                    parameters: method
                        .parameters
                        .iter()
                        .map(|p| self.resolve_type_with_self(&p.ty, &target))
                        .collect::<Result<_, _>>()?,
                    result: method
                        .return_type
                        .as_ref()
                        .map(|ty| self.resolve_type_with_self(ty, &target))
                        .transpose()?
                        .unwrap_or(Type::Unit),
                    capabilities: capability_set(&method.capabilities),
                };
                let mut substitutions = HashMap::new();
                substitutions.insert("Self".into(), target.clone());
                for (name, argument) in trait_info
                    .generics
                    .iter()
                    .zip(implementation.trait_name.as_ref().unwrap().arguments.iter())
                {
                    substitutions.insert(name.clone(), self.resolve_type(argument)?);
                }
                for (expected_name, actual_name) in expected.generics.iter().zip(&actual.generics) {
                    substitutions.insert(expected_name.clone(), Type::Generic(actual_name.clone()));
                }
                if expected.asynchronous != actual.asynchronous
                    || expected.generics.len() != actual.generics.len()
                    || positional_constraints(&expected) != positional_constraints(&actual)
                    || expected.capabilities != actual.capabilities
                    || expected
                        .parameters
                        .iter()
                        .map(|ty| {
                            substitute_associated(
                                &substitute(ty, &substitutions),
                                &associated_types,
                            )
                        })
                        .collect::<Vec<_>>()
                        != actual.parameters
                    || substitute_associated(
                        &substitute(&expected.result, &substitutions),
                        &associated_types,
                    ) != actual.result
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "method `{}` does not match trait `{trait_name}`",
                            method.name
                        ),
                        method.span,
                    ));
                }
                self.check_callable(method, actual)?;
            }
        }

        for function in &program.functions {
            if function.external.is_none() {
                self.check_function(function)?;
            }
        }
        Ok(())
    }

    fn validate_external_signature(
        &self,
        function: &Function,
        parameters: &[Type],
        result: &Type,
    ) -> Result<(), Diagnostic> {
        if function.name == "main" {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "`main` must be implemented in DISP and cannot be external",
                function.name_span,
            ));
        }
        if !function.generics.is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "external C functions cannot be generic",
                function.span,
            ));
        }
        if !is_c_identifier(&function.external.as_ref().unwrap().link_name) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "an external C symbol must use a safe, non-reserved ASCII C identifier",
                function.name_span,
            )
            .with_help(
                "C keywords and DISP runtime-reserved symbol prefixes cannot be imported directly",
            ));
        }
        if let Some(library) = function
            .external
            .as_ref()
            .and_then(|external| external.library.as_deref())
            && (library.is_empty()
                || !library.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                }))
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "external library names may only contain ASCII letters, digits, `_`, and `-`",
                function.span,
            ));
        }
        for (parameter, ty) in function.parameters.iter().zip(parameters) {
            if !self.ffi_parameter_type(ty) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "{} is not safe to pass through the defined C ABI",
                        self.format_type(ty)
                    ),
                    parameter.ty.span,
                )
                .with_help(
                    "use fixed-width numbers, CSize/CSSize, CStr, CFunction, or an explicit raw pointer",
                ));
            }
        }
        if !self.ffi_result_type(result) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "{} is not safe to return through the defined C ABI",
                    self.format_type(result)
                ),
                function
                    .return_type
                    .as_ref()
                    .map_or(function.name_span, |ty| ty.span),
            )
            .with_help(
                "return a scalar or explicit raw pointer; borrowed CStr results are rejected",
            ));
        }
        Ok(())
    }

    fn validate_export_signature(
        &self,
        function: &Function,
        parameters: &[Type],
        result: &Type,
    ) -> Result<(), Diagnostic> {
        if function.name == "main" {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "`main` cannot be exported through the C library ABI",
                function.name_span,
            ));
        }
        if function.asynchronous {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "exported C functions cannot be async",
                function.span,
            ));
        }
        if !function.generics.is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "exported C functions cannot be generic",
                function.span,
            ));
        }
        let export_capabilities_valid = function.capabilities.as_ref().is_some_and(|uses| {
            uses.is_empty()
                || matches!(uses.as_slice(), [capability] if capability.capability == Capability::Foreign)
        });
        if !export_capabilities_valid {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "exported C functions require an explicit `uses Pure` or `uses Foreign` contract",
                function.span,
            )
            .with_help(
                "use `Foreign` only when the export invokes a typed CFunction callback; keep all other ambient authority outside the in-process C ABI boundary",
            ));
        }
        if !is_c_identifier(&function.name) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "an exported C symbol must use a safe, non-reserved ASCII C identifier",
                function.name_span,
            ));
        }
        for (parameter, ty) in function.parameters.iter().zip(parameters) {
            if !self.ffi_parameter_type(ty) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "{} is not safe to pass through the exported C ABI",
                        self.format_type(ty)
                    ),
                    parameter.ty.span,
                )
                .with_help("use C ABI scalars, CStr, CFunction, or explicit raw pointers"));
            }
        }
        if !self.ffi_result_type(result) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "{} is not safe to return through the exported C ABI",
                    self.format_type(result)
                ),
                function
                    .return_type
                    .as_ref()
                    .map_or(function.name_span, |ty| ty.span),
            )
            .with_help("return Unit, a C ABI scalar, or an explicit raw pointer"));
        }
        Ok(())
    }

    fn validate_c_abi_struct(
        &self,
        declaration: &crate::ast::StructDeclaration,
    ) -> Result<(), Diagnostic> {
        if declaration.data {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "a data schema cannot define a C ABI record",
                declaration.name_span,
            ));
        }
        if !declaration.generics.is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "an exported C struct cannot be generic",
                declaration.name_span,
            ));
        }
        if declaration.fields.is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "an exported C struct must declare at least one field",
                declaration.name_span,
            )
            .with_help("ISO C has no portable zero-sized struct representation"));
        }
        if !is_c_identifier(&declaration.name) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "an exported C struct must use a safe, non-reserved ASCII C identifier",
                declaration.name_span,
            ));
        }
        let Type::Struct(id, _) = self.types[&declaration.name] else {
            unreachable!();
        };
        let info = &self.structs[&id];
        for field in &declaration.fields {
            if !is_c_identifier(&field.name) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "an exported C struct field must use a safe, non-reserved ASCII C identifier",
                    field.name_span,
                ));
            }
            let ty = &info.fields[&field.name];
            if !self.c_abi_field_type(ty) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!(
                        "{} has no stable value representation inside an exported C struct",
                        self.format_type(ty)
                    ),
                    field.ty.span,
                )
                .with_help("use C ABI scalars, explicit raw pointers to stable ABI types, or another exported C struct"));
            }
        }
        Ok(())
    }

    fn c_abi_field_type(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Bool
                | Type::Int
                | Type::UInt
                | Type::Signed(_)
                | Type::Unsigned(_)
                | Type::Float
                | Type::Float32
        ) || matches!(
            ty,
            Type::RawPointer(inner, _) if self.c_abi_pointer_target(inner)
        ) || matches!(
            ty,
            Type::Struct(id, arguments)
                if arguments.is_empty() && self.structs.get(id).is_some_and(|info| info.c_abi)
        )
    }

    fn c_abi_pointer_target(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Unit
                | Type::Bool
                | Type::Int
                | Type::UInt
                | Type::Signed(_)
                | Type::Unsigned(_)
                | Type::Float
                | Type::Float32
                | Type::CStr
        ) || matches!(
            ty,
            Type::RawPointer(inner, _) if self.c_abi_pointer_target(inner)
        ) || matches!(
            ty,
            Type::Struct(id, arguments)
                if arguments.is_empty() && self.structs.get(id).is_some_and(|info| info.c_abi)
        )
    }

    fn ffi_parameter_type(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Bool
                | Type::Int
                | Type::UInt
                | Type::Signed(_)
                | Type::Unsigned(_)
                | Type::Float
                | Type::Float32
                | Type::CStr
                | Type::RawPointer(_, _)
        ) || matches!(
            ty,
            Type::CFunction(parameters, result)
                if parameters.iter().all(|parameter| self.ffi_parameter_type(parameter))
                    && self.ffi_result_type(result)
        ) || matches!(
            ty,
            Type::Struct(id, arguments)
                if arguments.is_empty() && self.structs.get(id).is_some_and(|info| info.c_abi)
        )
    }

    fn ffi_result_type(&self, ty: &Type) -> bool {
        *ty == Type::Unit || (self.ffi_parameter_type(ty) && *ty != Type::CStr)
    }

    fn set_generics(&mut self, parameters: &[crate::ast::GenericParameter]) {
        self.generic_types = parameters
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
    }

    fn check_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        self.set_generics(&function.generics);
        let signature = self
            .functions
            .get(&function.name)
            .cloned()
            .expect("function signatures are collected before body checking");
        self.check_callable(function, signature)
    }

    fn check_callable(
        &mut self,
        function: &Function,
        signature: Signature,
    ) -> Result<(), Diagnostic> {
        if signature.parameters.iter().any(type_contains_task)
            || type_contains_task(&signature.result)
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "Task<T> cannot escape the structured scope of `{}`",
                    function.name
                ),
                function.span,
            )
            .with_help("spawn, await, or let the task cancel within the same async function"));
        }
        if function.asynchronous
            && (signature
                .parameters
                .iter()
                .any(type_crosses_thread_by_borrow)
                || type_crosses_thread_by_borrow(&signature.result))
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "async function `{}` cannot capture a borrowed or raw-pointer type",
                    function.name
                ),
                function.span,
            )
            .with_help("pass owned data into async functions and return owned results"));
        }
        self.expected_return = signature.result.clone();
        self.scopes.clear();
        debug_assert_eq!(self.unsafe_depth, 0);
        debug_assert!(self.unsafe_contracts.is_empty());
        self.begin_scope();
        for (parameter, ty) in function.parameters.iter().zip(signature.parameters) {
            self.scopes.last_mut().unwrap().insert(
                parameter.name.clone(),
                Variable {
                    ty,
                    constant: false,
                },
            );
        }
        self.async_depth += usize::from(function.asynchronous);
        let checked = self.check_block_contents(&function.body);
        self.async_depth -= usize::from(function.asynchronous);
        let always_returns = checked?;
        if self.expected_return != Type::Unit && !always_returns {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "function `{}` may finish without returning {:?}",
                    function.name, self.expected_return
                ),
                function.span,
            )
            .with_help("return a value on every control-flow path"));
        }
        self.end_scope();
        Ok(())
    }

    fn resolve_type_with_self(&self, ty: &TypeName, self_type: &Type) -> Result<Type, Diagnostic> {
        if ty.name == "Self" && ty.arguments.is_empty() {
            return Ok(match ty.qualifier {
                TypeQualifier::Owned => self_type.clone(),
                TypeQualifier::SharedReference => {
                    Type::Reference(Box::new(self_type.clone()), false)
                }
                TypeQualifier::MutableReference => {
                    Type::Reference(Box::new(self_type.clone()), true)
                }
                TypeQualifier::RawConstPointer => {
                    Type::RawPointer(Box::new(self_type.clone()), false)
                }
                TypeQualifier::RawMutPointer => Type::RawPointer(Box::new(self_type.clone()), true),
            });
        }
        self.resolve_type(ty)
    }

    fn check_block(&mut self, block: &Block) -> Result<bool, Diagnostic> {
        self.begin_scope();
        let result = self.check_block_contents(block);
        self.end_scope();
        result
    }

    fn check_block_contents(&mut self, block: &Block) -> Result<bool, Diagnostic> {
        let mut always_returns = false;
        for statement in &block.statements {
            if !always_returns {
                always_returns = self.check_statement(&statement.node, statement.span)?;
            }
        }
        Ok(always_returns)
    }

    fn check_statement(&mut self, statement: &Statement, span: Span) -> Result<bool, Diagnostic> {
        match statement {
            Statement::Binding {
                kind,
                name,
                annotation,
                value,
                ..
            } => {
                if value.is_none() && *kind == BindingKind::Const {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("constant `{name}` requires an initializer"),
                        span,
                    ));
                }
                let inferred = value
                    .as_ref()
                    .map(|value| self.check_expression(value))
                    .transpose()?;
                let ty = if let Some(annotation) = annotation {
                    let declared = self.resolve_type(annotation)?;
                    if let (Some(inferred), Some(value)) = (&inferred, value) {
                        self.require_same(&declared, inferred, value.span, "binding initializer")?;
                    }
                    declared
                } else {
                    let inferred =
                        inferred.expect("parser requires an initializer without an annotation");
                    if contains_infer(&inferred) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("cannot infer the complete type of binding `{name}`"),
                            value.as_ref().unwrap().span,
                        )
                        .with_help("add an explicit Option<T> or Result<T, E> annotation"));
                    }
                    let materialized = materialize_literal(inferred);
                    if matches!(
                        materialized,
                        Type::IntLiteral(_) | Type::NegativeIntLiteral(_)
                    ) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "integer literal does not fit the default `int` type",
                            value.as_ref().unwrap().span,
                        )
                        .with_help("add an explicit i128 or u128 annotation"));
                    }
                    materialized
                };
                if ty == Type::Str {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "a `str` view must be held through `&str`",
                        value.as_ref().map_or(span, |value| value.span),
                    )
                    .with_help("borrow the view, for example `let view: &str = &text[0..end]`"));
                }
                if *kind == BindingKind::Const
                    && !self.is_constant_expression(value.as_ref().unwrap())
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("constant `{name}` requires a compile-time expression"),
                        value.as_ref().unwrap().span,
                    ));
                }
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty,
                        constant: *kind == BindingKind::Const,
                    },
                );
                Ok(false)
            }
            Statement::Assignment {
                name,
                operator,
                value,
                name_span,
            } => {
                let Some(target) = self.lookup_variable(name) else {
                    if *operator != AssignmentOperator::Assign {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("unknown variable `{name}`"),
                            *name_span,
                        ));
                    }
                    let inferred = self.check_expression(value)?;
                    if contains_infer(&inferred) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("cannot infer the complete type of binding `{name}`"),
                            value.span,
                        ));
                    }
                    let ty = materialize_literal(inferred);
                    if ty == Type::Str {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "a `str` view must be held through `&str`",
                            value.span,
                        ));
                    }
                    self.scopes.last_mut().unwrap().insert(
                        name.clone(),
                        Variable {
                            ty,
                            constant: false,
                        },
                    );
                    return Ok(false);
                };
                let value_type = self.check_expression(value)?;
                self.require_same(&target.ty, &value_type, value.span, "assignment")?;
                if *operator != AssignmentOperator::Assign && !is_numeric(&target.ty) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "compound arithmetic assignment requires a number, found {:?}",
                            target.ty
                        ),
                        span,
                    ));
                }
                Ok(false)
            }
            Statement::PlaceAssignment {
                target,
                operator,
                value,
            } => {
                let target_type = self.check_expression(target)?;
                let value_type = self.check_expression(value)?;
                if *operator == AssignmentOperator::Assign {
                    self.require_same(&target_type, &value_type, value.span, "assignment")?;
                } else {
                    self.check_binary(
                        match operator {
                            AssignmentOperator::Add => BinaryOperator::Add,
                            AssignmentOperator::Subtract => BinaryOperator::Subtract,
                            AssignmentOperator::Multiply => BinaryOperator::Multiply,
                            AssignmentOperator::Divide => BinaryOperator::Divide,
                            AssignmentOperator::Assign => unreachable!(),
                        },
                        target_type,
                        value_type,
                        span,
                    )?;
                }
                Ok(false)
            }
            Statement::Expression(expression) => {
                self.check_expression(expression)?;
                Ok(false)
            }
            Statement::Return(value) => {
                let actual = value
                    .as_ref()
                    .map(|expression| self.check_expression(expression))
                    .transpose()?
                    .unwrap_or(Type::Unit);
                self.require_same(&self.expected_return, &actual, span, "return value")?;
                Ok(true)
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_type = self.check_expression(condition)?;
                self.require_same(&Type::Bool, &condition_type, condition.span, "if condition")?;
                let then_returns = self.check_block(then_branch)?;
                let else_returns = else_branch
                    .as_ref()
                    .map(|branch| self.check_block(branch))
                    .transpose()?
                    .unwrap_or(false);
                Ok(then_returns && else_returns)
            }
            Statement::While { condition, body } => {
                let condition_type = self.check_expression(condition)?;
                self.require_same(
                    &Type::Bool,
                    &condition_type,
                    condition.span,
                    "while condition",
                )?;
                self.check_block(body)?;
                Ok(false)
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                let start_type = self.check_expression(start)?;
                let end_type = self.check_expression(end)?;
                self.require_same(&Type::Int, &start_type, start.span, "range start")?;
                self.require_same(&Type::Int, &end_type, end.span, "range end")?;
                self.begin_scope();
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty: Type::Int,
                        constant: false,
                    },
                );
                let result = self.check_block_contents(body);
                self.end_scope();
                result?;
                Ok(false)
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                let iterable_ty = self.check_expression(iterable)?;
                let element = match iterable_ty {
                    Type::Array(element, _)
                    | Type::Slice(element)
                    | Type::List(element)
                    | Type::Set(element) => *element,
                    Type::Reference(inner, _) => match *inner {
                        Type::Array(element, _)
                        | Type::Slice(element)
                        | Type::List(element)
                        | Type::Set(element) => *element,
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`for item in value` requires an array, slice, or List",
                                iterable.span,
                            ));
                        }
                    },
                    _ => {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`for item in value` requires an array, slice, or List",
                            iterable.span,
                        ));
                    }
                };
                let item = if self.type_is_copy(&element) {
                    element
                } else {
                    Type::Reference(Box::new(element), false)
                };
                self.begin_scope();
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty: item,
                        constant: false,
                    },
                );
                let result = self.check_block_contents(body);
                self.end_scope();
                result?;
                Ok(false)
            }
            Statement::Loop(body) => {
                self.check_block(body)?;
                Ok(!contains_break_for_current_loop(body))
            }
            Statement::Unsafe { capabilities, body } => {
                self.unsafe_depth += 1;
                self.unsafe_contracts
                    .push(capabilities.as_ref().map(|items| {
                        items
                            .iter()
                            .map(|item| item.capability)
                            .collect::<HashSet<_>>()
                    }));
                let result = self.check_block(body);
                self.unsafe_contracts.pop();
                self.unsafe_depth -= 1;
                result
            }
            Statement::Break | Statement::Continue => Ok(false),
        }
    }

    fn check_expression(&mut self, expression: &Expr) -> Result<Type, Diagnostic> {
        match &expression.node {
            Expression::Integer(value) => Ok(Type::IntLiteral(*value)),
            Expression::Float(_) => Ok(Type::FloatLiteral),
            Expression::String(_) => Ok(Type::String),
            Expression::Array(values) => {
                let Some(first) = values.first() else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "cannot infer the element type of an empty array",
                        expression.span,
                    ));
                };
                let element = match self.check_expression(first)? {
                    Type::IntLiteral(value) if value <= i64::MAX as u128 => Type::Int,
                    Type::FloatLiteral => Type::Float,
                    other => other,
                };
                for value in &values[1..] {
                    let actual = self.check_expression(value)?;
                    self.require_same(&element, &actual, value.span, "array element")?;
                }
                Ok(Type::Array(Box::new(element), values.len()))
            }
            Expression::DataWrite {
                value,
                store,
                replace: _,
            } => {
                let value_ty = materialize_literal(self.check_expression(value)?);
                let Type::Struct(id, arguments) = value_ty else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`data add` and `data save` require a data value",
                        value.span,
                    ));
                };
                let info = &self.structs[&id];
                if !info.data || !arguments.is_empty() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("`{}` is not a concrete data schema", info.name),
                        value.span,
                    ));
                }
                let store_ty = self.check_expression(store)?;
                self.require_same(&Type::DataStore, &store_ty, store.span, "DISP Data store")?;
                Ok(Type::Result(
                    Box::new(Type::UInt),
                    Box::new(Type::DataError),
                ))
            }
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    let ty = self.check_expression(path)?;
                    if !matches!(ty, Type::Path) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`data open` requires a Path",
                            path.span,
                        ));
                    }
                }
                Ok(Type::Result(
                    Box::new(Type::DataStore),
                    Box::new(Type::DataError),
                ))
            }
            Expression::DataQuery {
                schema,
                schema_span,
                store,
                predicate,
                order,
                limit,
            } => {
                let id = self.require_data_schema(schema, *schema_span)?;
                let store_ty = self.check_expression(store)?;
                self.require_same(&Type::DataStore, &store_ty, store.span, "DISP Data store")?;
                let previous = self.data_context.replace(id);
                let checked = (|| {
                    if let Some(predicate) = predicate {
                        let ty = self.check_expression(predicate)?;
                        self.require_same(&Type::Bool, &ty, predicate.span, "DISP Data condition")?;
                    }
                    if let Some(order) = order {
                        let ty = materialize_literal(self.check_expression(&order.key)?);
                        if !is_data_order_type(&ty) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("cannot order data by {}", self.format_type(&ty)),
                                order.key.span,
                            ));
                        }
                    }
                    Ok(())
                })();
                self.data_context = previous;
                checked?;
                if let Some(limit) = limit {
                    let ty = self.check_expression(limit)?;
                    if !is_integer(&ty) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "DISP Data limit must be an integer",
                            limit.span,
                        ));
                    }
                }
                Ok(Type::Result(
                    Box::new(Type::List(Box::new(Type::Struct(id, vec![])))),
                    Box::new(Type::DataError),
                ))
            }
            Expression::DataRemove {
                schema,
                schema_span,
                store,
                predicate,
            } => {
                let id = self.require_data_schema(schema, *schema_span)?;
                let store_ty = self.check_expression(store)?;
                self.require_same(&Type::DataStore, &store_ty, store.span, "DISP Data store")?;
                let previous = self.data_context.replace(id);
                let predicate_ty = self.check_expression(predicate);
                self.data_context = previous;
                let predicate_ty = predicate_ty?;
                self.require_same(
                    &Type::Bool,
                    &predicate_ty,
                    predicate.span,
                    "DISP Data condition",
                )?;
                Ok(Type::Result(
                    Box::new(Type::UInt),
                    Box::new(Type::DataError),
                ))
            }
            Expression::Closure {
                parameters,
                return_type,
                body,
                ..
            } => {
                let parameter_types = parameters
                    .iter()
                    .map(|parameter| self.resolve_type(&parameter.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                let declared_result = return_type
                    .as_ref()
                    .map(|result| self.resolve_type(result))
                    .transpose()?;
                let previous_return = self.expected_return.clone();
                self.expected_return = declared_result.clone().unwrap_or(Type::Infer);
                self.begin_scope();
                for (parameter, ty) in parameters.iter().zip(&parameter_types) {
                    self.scopes.last_mut().unwrap().insert(
                        parameter.name.clone(),
                        Variable {
                            ty: ty.clone(),
                            constant: false,
                        },
                    );
                }
                let checked = match body {
                    crate::ast::ClosureBody::Expression(value) => {
                        let actual = materialize_literal(self.check_expression(value)?);
                        if let Some(expected) = &declared_result {
                            self.require_same(expected, &actual, value.span, "closure result")?;
                            expected.clone()
                        } else {
                            actual
                        }
                    }
                    crate::ast::ClosureBody::Block(block) => {
                        let expected = declared_result
                            .clone()
                            .expect("parser requires result type");
                        let always_returns = self.check_block_contents(block)?;
                        if expected != Type::Unit && !always_returns {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "closure may finish without returning {}",
                                    self.format_type(&expected)
                                ),
                                block.span,
                            ));
                        }
                        expected
                    }
                };
                self.end_scope();
                self.expected_return = previous_return;
                Ok(Type::Function(parameter_types, Box::new(checked)))
            }
            Expression::Index { object, index } => {
                let object_ty = self.check_expression(object)?;
                let index_ty = self.check_expression(index)?;
                if !matches!(
                    index_ty,
                    Type::Int
                        | Type::UInt
                        | Type::Signed(_)
                        | Type::Unsigned(_)
                        | Type::IntLiteral(_)
                ) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "array index must be an integer",
                        index.span,
                    ));
                }
                match object_ty {
                    Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                        Ok(*element)
                    }
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("cannot index {}", self.format_type(&other)),
                        object.span,
                    )),
                }
            }
            Expression::Subslice { object, start, end } => {
                let object_ty = self.check_expression(object)?;
                for bound in [start, end] {
                    let ty = self.check_expression(bound)?;
                    if !matches!(
                        ty,
                        Type::Int
                            | Type::UInt
                            | Type::Signed(_)
                            | Type::Unsigned(_)
                            | Type::IntLiteral(_)
                    ) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "subslice bound must be an integer",
                            bound.span,
                        ));
                    }
                }
                match object_ty {
                    Type::Array(element, _) | Type::Slice(element) | Type::List(element) => {
                        Ok(Type::Slice(element))
                    }
                    Type::String | Type::Str => Ok(Type::Str),
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("cannot subslice {}", self.format_type(&other)),
                        object.span,
                    )),
                }
            }
            Expression::Character(_) => Ok(Type::Char),
            Expression::Bool(_) => Ok(Type::Bool),
            Expression::Spawn(task) => {
                let Expression::Call { callee, arguments } = &task.node else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`spawn` requires a direct function call",
                        task.span,
                    ));
                };
                let Expression::Identifier(name) = &callee.node else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`spawn` currently requires a named DISP function",
                        callee.span,
                    ));
                };
                let Some(signature) = self.functions.get(name).cloned() else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`spawn` target must be a DISP function",
                        callee.span,
                    ));
                };
                if signature.asynchronous {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`spawn` requires a synchronous function",
                        callee.span,
                    )
                    .with_help("call the async function and `await` its Future instead"));
                }
                if self.external_functions.contains(name) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "external C calls cannot be spawned directly",
                        callee.span,
                    )
                    .with_help("wrap the unsafe C call in a checked DISP function, then spawn that function"));
                }
                if signature
                    .parameters
                    .iter()
                    .any(type_crosses_thread_by_borrow)
                {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "spawned functions cannot accept references, borrowed views, or raw pointers across a thread boundary",
                        callee.span,
                    )
                    .with_help("pass owned data to the spawned function"));
                }
                let result = self.check_expression(task)?;
                for argument in arguments {
                    let ty = self.check_expression(argument)?;
                    if !self.type_is_send(&ty, &mut HashSet::new()) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "{} cannot be transferred to another thread",
                                self.format_type(&ty)
                            ),
                            argument.span,
                        )
                        .with_help("move owned values into the thread; references and raw pointers cannot cross a thread boundary"));
                    }
                }
                if !self.type_is_send(&result, &mut HashSet::new()) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "thread result {} cannot cross a thread boundary",
                            self.format_type(&result)
                        ),
                        task.span,
                    ));
                }
                Ok(Type::Thread(Box::new(result)))
            }
            Expression::Await(future) => {
                if self.async_depth == 0 {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`await` is only allowed inside an `async fn`",
                        expression.span,
                    )
                    .with_help("make the enclosing function `async`, or wait at an explicit synchronous boundary"));
                }
                match self.check_expression(future)? {
                    Type::Future(output) | Type::Task(output) => Ok(*output),
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "`await` requires Future<T>, found {}",
                            self.format_type(&other)
                        ),
                        future.span,
                    )),
                }
            }
            Expression::StructConstruct { name, fields, .. } => {
                let Some(Type::Struct(id, _)) = self.types.get(name).cloned() else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("`{name}` is not a struct type"),
                        expression.span,
                    ));
                };
                let info = self.structs[&id].clone();
                let mut provided = HashMap::new();
                let mut substitutions = HashMap::new();
                for field in fields {
                    if provided
                        .insert(field.name.clone(), field.name_span)
                        .is_some()
                    {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("field `{}` is provided more than once", field.name),
                            field.name_span,
                        ));
                    }
                    let expected = info.fields.get(&field.name).ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("struct `{}` has no field `{}`", info.name, field.name),
                            field.name_span,
                        )
                    })?;
                    let actual = self.check_expression(&field.value)?;
                    infer_substitutions(expected, &actual, &mut substitutions, field.value.span)?;
                    self.require_same(
                        &substitute(expected, &substitutions),
                        &actual,
                        field.value.span,
                        "struct field",
                    )?;
                }
                let mut missing_fields = info
                    .fields
                    .keys()
                    .filter(|field| !provided.contains_key(*field))
                    .cloned()
                    .collect::<Vec<_>>();
                missing_fields.sort();
                if let Some(missing) = missing_fields.first() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "missing field `{missing}` when constructing `{}`",
                            info.name
                        ),
                        expression.span,
                    ));
                }
                let arguments = info
                    .generics
                    .iter()
                    .map(|name| {
                        substitutions.get(name).cloned().ok_or_else(|| {
                            Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "cannot infer generic argument `{name}` for `{}`",
                                    info.name
                                ),
                                expression.span,
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.require_constraints(&info.constraints, &arguments, expression.span)?;
                Ok(Type::Struct(id, arguments))
            }
            Expression::Identifier(name) => {
                if let Some(variable) = self.lookup_variable(name) {
                    return Ok(variable.ty);
                }
                if let Some(schema) = self.data_context
                    && let Some(ty) = self.structs[&schema].fields.get(name)
                {
                    return Ok(ty.clone());
                }
                if let Some(signature) = self.functions.get(name) {
                    if !signature.generics.is_empty() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "generic function `{name}` needs concrete type arguments before it can become a value"
                            ),
                            expression.span,
                        )
                        .with_help(
                            "call the generic function directly, or wrap a concrete call in a closure",
                        ));
                    }
                    if self.external_functions.contains(name) {
                        return Ok(Type::CFunction(
                            signature.parameters.clone(),
                            Box::new(signature.result.clone()),
                        ));
                    }
                    return Ok(Type::Function(
                        signature.parameters.clone(),
                        Box::new(if signature.asynchronous {
                            Type::Future(Box::new(signature.result.clone()))
                        } else {
                            signature.result.clone()
                        }),
                    ));
                }
                if name == "print" {
                    return Ok(Type::Function(vec![], Box::new(Type::Unit)));
                }
                match name.as_str() {
                    "None" => return Ok(Type::Option(Box::new(Type::Infer))),
                    "Some" => {
                        return Ok(Type::Function(
                            vec![Type::Infer],
                            Box::new(Type::Option(Box::new(Type::Infer))),
                        ));
                    }
                    "Ok" => {
                        return Ok(Type::Function(
                            vec![Type::Infer],
                            Box::new(Type::Result(Box::new(Type::Infer), Box::new(Type::Infer))),
                        ));
                    }
                    "Err" => {
                        return Ok(Type::Function(
                            vec![Type::Infer],
                            Box::new(Type::Result(Box::new(Type::Infer), Box::new(Type::Infer))),
                        ));
                    }
                    _ => {}
                }
                if let Some(candidates) = self.variants.get(name) {
                    if candidates.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("ambiguous enum variant `{name}`"),
                            expression.span,
                        ));
                    }
                    return Ok(self.variant_constructor_type(&candidates[0]));
                }
                Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("unknown name `{name}`"),
                    expression.span,
                ))
            }
            Expression::FieldAccess {
                object,
                field,
                field_span,
            } => {
                if let Expression::Identifier(type_name) = &object.node
                    && let Some(Type::Enum(id, _)) = self.types.get(type_name)
                {
                    let info = &self.enums[id];
                    let variant = info.variants.get(field).ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("enum `{}` has no variant `{field}`", info.name),
                            *field_span,
                        )
                    })?;
                    return Ok(self.variant_constructor_type(variant));
                }
                let object_type = match self.check_expression(object)? {
                    Type::Reference(inner, _) => *inner,
                    other => other,
                };
                let Type::Struct(id, arguments) = object_type else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("field access requires a struct, found {object_type:?}"),
                        object.span,
                    ));
                };
                let substitutions = self.structs[&id]
                    .generics
                    .iter()
                    .cloned()
                    .zip(arguments)
                    .collect();
                self.structs[&id]
                    .fields
                    .get(field)
                    .map(|ty| substitute(ty, &substitutions))
                    .ok_or_else(|| {
                        Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("struct `{}` has no field `{field}`", self.structs[&id].name),
                            *field_span,
                        )
                    })
            }
            Expression::Unary { operator, operand } => {
                let ty = self.check_expression(operand)?;
                match operator {
                    UnaryOperator::Negate if is_numeric(&ty) => Ok(match ty {
                        Type::IntLiteral(value) if value <= i128::MAX as u128 => {
                            Type::NegativeIntLiteral(-(value as i128))
                        }
                        Type::IntLiteral(value) if value == (1_u128 << 127) => {
                            Type::NegativeIntLiteral(i128::MIN)
                        }
                        Type::IntLiteral(_) => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "negative integer literal is outside i128 range",
                                expression.span,
                            ));
                        }
                        other => other,
                    }),
                    UnaryOperator::Not if ty == Type::Bool => Ok(Type::Bool),
                    UnaryOperator::Negate => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("unary `-` requires a number, found {ty:?}"),
                        expression.span,
                    )),
                    UnaryOperator::Not => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("unary `!` requires Bool, found {ty:?}"),
                        expression.span,
                    )),
                }
            }
            Expression::Move(operand) => self.check_expression(operand),
            Expression::Borrow { mutable, target } => {
                let target = self.check_expression(target)?;
                if *mutable && target == Type::Str {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "`str` is an immutable UTF-8 view",
                        expression.span,
                    ));
                }
                Ok(Type::Reference(Box::new(target), *mutable))
            }
            Expression::Dereference(target) => match self.check_expression(target)? {
                Type::Reference(inner, _) => Ok(*inner),
                Type::MutexGuard(inner) => Ok(*inner),
                Type::MemoryPointer(_, _) => Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "checked Memory pointers must be accessed with `.read()` or `.write()`",
                    expression.span,
                )
                .with_help("use checked pointer methods so bounds and alignment are validated")),
                Type::RawPointer(inner, _) => {
                    self.require_unsafe_capability(
                        Capability::RawMemory,
                        "raw pointer dereference",
                        expression.span,
                        "prove the pointer is live, aligned, and points to an initialized value",
                    )?;
                    Ok(*inner)
                }
                other => Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("cannot dereference {}", self.format_type(&other)),
                    expression.span,
                )),
            },
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left_type = self.check_expression(left)?;
                let right_type = self.check_expression(right)?;
                self.check_binary(*operator, left_type, right_type, expression.span)
            }
            Expression::Call { callee, arguments } => {
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Async")
                {
                    if field == "yield" && arguments.is_empty() {
                        return Ok(Type::Future(Box::new(Type::Unit)));
                    }
                    if field == "spawn" && arguments.len() == 1 {
                        if self.async_depth == 0 {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`Async.spawn` is only allowed inside an `async fn`",
                                expression.span,
                            ));
                        }
                        return match self.check_expression(&arguments[0])? {
                            Type::Future(output) => Ok(Type::Task(output)),
                            other => Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "`Async.spawn` requires Future<T>, found {}",
                                    self.format_type(&other)
                                ),
                                arguments[0].span,
                            )),
                        };
                    }
                    if field == "sleep" && arguments.len() == 1 {
                        let duration = self.check_expression(&arguments[0])?;
                        self.require_same(
                            &Type::Duration,
                            &duration,
                            arguments[0].span,
                            "async sleep duration",
                        )?;
                        return Ok(Type::Future(Box::new(Type::Unit)));
                    }
                    if field == "connect" && arguments.len() == 1 {
                        let address = self.check_expression(&arguments[0])?;
                        self.require_same(
                            &Type::SocketAddress,
                            &address,
                            arguments[0].span,
                            "TCP connect address",
                        )?;
                        return Ok(Type::Future(Box::new(Type::Result(
                            Box::new(Type::TcpStream),
                            Box::new(Type::NetworkError),
                        ))));
                    }
                    if field == "connect_timeout" && arguments.len() == 2 {
                        let address = self.check_expression(&arguments[0])?;
                        self.require_same(
                            &Type::SocketAddress,
                            &address,
                            arguments[0].span,
                            "TCP connect address",
                        )?;
                        let timeout = self.check_expression(&arguments[1])?;
                        self.require_same(
                            &Type::Duration,
                            &timeout,
                            arguments[1].span,
                            "TCP connect timeout",
                        )?;
                        return Ok(Type::Future(Box::new(Type::Result(
                            Box::new(Type::TcpStream),
                            Box::new(Type::NetworkError),
                        ))));
                    }
                    if matches!(field.as_str(), "resolve" | "resolve_timeout")
                        && arguments.len() == if field == "resolve" { 1 } else { 2 }
                    {
                        let host = self.check_expression(&arguments[0])?;
                        if !matches!(host, Type::String | Type::Str) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "DNS host must be String or str",
                                arguments[0].span,
                            ));
                        }
                        if field == "resolve_timeout" {
                            let timeout = self.check_expression(&arguments[1])?;
                            self.require_same(
                                &Type::Duration,
                                &timeout,
                                arguments[1].span,
                                "DNS resolution timeout",
                            )?;
                        }
                        return Ok(Type::Future(Box::new(Type::Result(
                            Box::new(Type::List(Box::new(Type::IpAddress))),
                            Box::new(Type::NetworkError),
                        ))));
                    }
                    if matches!(field.as_str(), "read_text" | "read_bytes") && arguments.len() == 1
                    {
                        self.require_path(&arguments[0])?;
                        let value = if field == "read_text" {
                            Type::String
                        } else {
                            Type::List(Box::new(Type::Unsigned(8)))
                        };
                        return Ok(Type::Future(Box::new(Type::Result(
                            Box::new(value),
                            Box::new(Type::IoError),
                        ))));
                    }
                    if matches!(field.as_str(), "write_text" | "write_bytes")
                        && arguments.len() == 2
                    {
                        self.require_path(&arguments[0])?;
                        let value = self.check_expression(&arguments[1])?;
                        let valid = if field == "write_text" {
                            matches!(value, Type::String)
                        } else {
                            matches!(value, Type::List(ref element) if matches!(**element, Type::Unsigned(8)))
                        };
                        if !valid {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                if field == "write_text" {
                                    "async file text must be an owned String"
                                } else {
                                    "async file bytes must be an owned List<u8>"
                                },
                                arguments[1].span,
                            )
                            .with_help(
                                "the future owns its input until completion or cancellation",
                            ));
                        }
                        return Ok(Type::Future(Box::new(Type::Result(
                            Box::new(Type::Unit),
                            Box::new(Type::IoError),
                        ))));
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "no async operation `Async.{field}` with {} arguments",
                            arguments.len()
                        ),
                        expression.span,
                    ));
                }
                if matches!(callee.node, Expression::FieldAccess { .. })
                    && let Ok(Type::Function(parameters, result)) = self.check_expression(callee)
                {
                    if parameters.len() != arguments.len() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "function expects {} arguments, found {}",
                                parameters.len(),
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let mut substitutions = HashMap::new();
                    for (parameter, argument) in parameters.iter().zip(arguments) {
                        let actual = self.check_expression(argument)?;
                        infer_substitutions(parameter, &actual, &mut substitutions, argument.span)?;
                        self.require_same(
                            &substitute(parameter, &substitutions),
                            &actual,
                            argument.span,
                            "function argument",
                        )?;
                    }
                    let result = substitute(&result, &substitutions);
                    self.validate_instantiated_type(&result, expression.span)?;
                    return Ok(result);
                }
                if let Expression::Identifier(name) = &callee.node
                    && self.external_functions.contains(name)
                {
                    self.require_unsafe_capability(
                        Capability::Foreign,
                        &format!("external call `{name}`"),
                        expression.span,
                        "validate the foreign function's contract, then place only the call inside `unsafe uses Foreign { ... }`",
                    )?;
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "String") {
                    if !arguments.is_empty() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`String` expects no arguments",
                            expression.span,
                        ));
                    }
                    return Ok(Type::String);
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Path") {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`Path` expects one String or str",
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !matches!(source, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "Path source must be String or str",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Path);
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Url") {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`Url` expects one String or str",
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !matches!(source, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "URL source must be String or str",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(Type::Url),
                        Box::new(Type::NetworkError),
                    ));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Json") {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`Json` expects one String or str",
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !matches!(source, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "JSON source must be String or str",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(Type::Json),
                        Box::new(Type::ConversionError),
                    ));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "SocketAddress") {
                    if arguments.len() != 2 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "`SocketAddress` expects a host and port",
                            expression.span,
                        ));
                    }
                    let host = self.check_expression(&arguments[0])?;
                    if !matches!(host, Type::String | Type::Str | Type::IpAddress) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "socket host must be String, str, or IpAddress",
                            arguments[0].span,
                        ));
                    }
                    let port = self.check_expression(&arguments[1])?;
                    if !is_integer(&port) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "socket port must be an integer from 0 through 65535",
                            arguments[1].span,
                        ));
                    }
                    return Ok(Type::SocketAddress);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "IpAddress")
                    && field == "parse"
                    && arguments.len() == 1
                {
                    let source = self.check_expression(&arguments[0])?;
                    if !matches!(source, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "IP address source must be String or str",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(Type::IpAddress),
                        Box::new(Type::NetworkError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Json")
                {
                    let conversion = || Type::ConversionError;
                    match field.as_str() {
                        "null" if arguments.is_empty() => return Ok(Type::Json),
                        "bool" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            self.require_same(
                                &Type::Bool,
                                &actual,
                                arguments[0].span,
                                "Json.bool value",
                            )?;
                            return Ok(Type::Json);
                        }
                        "int" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(
                                actual,
                                Type::Int
                                    | Type::Signed(_)
                                    | Type::IntLiteral(_)
                                    | Type::NegativeIntLiteral(_)
                            ) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Json.int expects a signed integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Json);
                        }
                        "uint" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(
                                actual,
                                Type::UInt | Type::Unsigned(_) | Type::IntLiteral(_)
                            ) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Json.uint expects an unsigned integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Json);
                        }
                        "float" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(actual, Type::Float | Type::Float32 | Type::FloatLiteral) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Json.float expects a floating-point value",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(Box::new(Type::Json), Box::new(conversion())));
                        }
                        "string" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(actual, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Json.string expects String or str",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(Box::new(Type::Json), Box::new(conversion())));
                        }
                        "array" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            self.require_same(
                                &Type::List(Box::new(Type::Json)),
                                &actual,
                                arguments[0].span,
                                "Json.array values",
                            )?;
                            return Ok(Type::Result(Box::new(Type::Json), Box::new(conversion())));
                        }
                        "object" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            self.require_same(
                                &Type::Map(Box::new(Type::String), Box::new(Type::Json)),
                                &actual,
                                arguments[0].span,
                                "Json.object entries",
                            )?;
                            return Ok(Type::Result(Box::new(Type::Json), Box::new(conversion())));
                        }
                        "from" if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            self.ensure_json_codec_type(&actual, arguments[0].span, false)?;
                            return Ok(Type::Result(Box::new(Type::Json), Box::new(conversion())));
                        }
                        "null" | "bool" | "int" | "uint" | "float" | "string" | "array"
                        | "object" | "from" => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`Json.{field}` received the wrong number of arguments"),
                                expression.span,
                            ));
                        }
                        _ => {}
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && field == "from_json"
                    && let Expression::Identifier(owner) = &object.node
                    && let Some(target) = self.types.get(owner).cloned()
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{owner}.from_json` expects one Json value"),
                            expression.span,
                        ));
                    }
                    let target = match target {
                        Type::Struct(id, _) if self.structs[&id].generics.is_empty() => {
                            Type::Struct(id, vec![])
                        }
                        Type::Enum(id, _) if self.enums[&id].generics.is_empty() => {
                            Type::Enum(id, vec![])
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "generic nominal JSON decoding requires a concrete wrapper type",
                                object.span,
                            )
                            .with_help("decode through a non-generic struct or enum whose fields use concrete generic arguments"));
                        }
                    };
                    let source = self.check_expression(&arguments[0])?;
                    self.require_same(&Type::Json, &source, arguments[0].span, "JSON source")?;
                    self.ensure_json_codec_type(&target, object.span, true)?;
                    return Ok(Type::Result(
                        Box::new(target),
                        Box::new(Type::ConversionError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Dns")
                    && field == "resolve"
                    && arguments.len() == 1
                {
                    let host = self.check_expression(&arguments[0])?;
                    if !matches!(host, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "DNS host must be String or str",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(Type::List(Box::new(Type::IpAddress))),
                        Box::new(Type::NetworkError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Tls")
                    && matches!(field.as_str(), "connect" | "connect_timeout")
                {
                    let expected = if field == "connect" { 2 } else { 3 };
                    if arguments.len() != expected {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`Tls.{field}` expects {expected} arguments"),
                            expression.span,
                        ));
                    }
                    let stream = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &Type::TcpStream,
                        &stream,
                        arguments[0].span,
                        "TLS source stream",
                    )?;
                    let server_name = self.check_expression(&arguments[1])?;
                    if !matches!(server_name, Type::String | Type::Str) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "TLS server name must be String or str",
                            arguments[1].span,
                        ));
                    }
                    if field == "connect_timeout" {
                        let timeout = self.check_expression(&arguments[2])?;
                        self.require_same(
                            &Type::Duration,
                            &timeout,
                            arguments[2].span,
                            "TLS handshake timeout",
                        )?;
                    }
                    return Ok(Type::Future(Box::new(Type::Result(
                        Box::new(Type::TlsStream),
                        Box::new(Type::NetworkError),
                    ))));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Http")
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
                    let expected = match field.as_str() {
                        "get" | "delete" => 1,
                        "get_timeout" | "delete_timeout" | "post" | "post_json" | "put"
                        | "patch" | "request" => 2,
                        _ => 3,
                    };
                    if arguments.len() != expected {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`Http.{field}` expects {expected} arguments"),
                            expression.span,
                        ));
                    }
                    let url_index = usize::from(field == "request");
                    if field == "request" {
                        let method = self.check_expression(&arguments[0])?;
                        if !matches!(method, Type::String | Type::Str) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "HTTP method must be String or str",
                                arguments[0].span,
                            ));
                        }
                        if let Expression::String(method) = &arguments[0].node
                            && !http_method_token(method)
                        {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "HTTP method is invalid or forbidden by the safe client",
                                arguments[0].span,
                            ));
                        }
                    }
                    let url = self.check_expression(&arguments[url_index])?;
                    if !matches!(url, Type::String | Type::Str | Type::Url) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "HTTP URL must be Url, String, or str",
                            arguments[url_index].span,
                        ));
                    }
                    if field == "request" {
                        return Ok(Type::Result(
                            Box::new(Type::HttpRequest),
                            Box::new(Type::HttpError),
                        ));
                    }
                    let has_body = matches!(
                        field.as_str(),
                        "post"
                            | "post_timeout"
                            | "post_json"
                            | "post_json_timeout"
                            | "put"
                            | "put_timeout"
                            | "patch"
                            | "patch_timeout"
                    );
                    if has_body {
                        let body = self.check_expression(&arguments[1])?;
                        if field.starts_with("post_json") && !matches!(body, Type::Json) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "HTTP JSON body must be Json",
                                arguments[1].span,
                            ));
                        }
                        if !field.starts_with("post_json") && !http_body_type(&body) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "HTTP body must be String, str, List<u8>, or a u8 slice",
                                arguments[1].span,
                            ));
                        }
                    }
                    if field.ends_with("_timeout") {
                        let timeout_index = if has_body { 2 } else { 1 };
                        let timeout = self.check_expression(&arguments[timeout_index])?;
                        self.require_same(
                            &Type::Duration,
                            &timeout,
                            arguments[timeout_index].span,
                            "HTTP request timeout",
                        )?;
                    }
                    return Ok(Type::Future(Box::new(Type::Result(
                        Box::new(Type::HttpResponse),
                        Box::new(Type::HttpError),
                    ))));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "UdpSocket")
                    && field == "bind"
                    && arguments.len() == 1
                {
                    let address = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &Type::SocketAddress,
                        &address,
                        arguments[0].span,
                        "UDP bind address",
                    )?;
                    return Ok(Type::Result(
                        Box::new(Type::UdpSocket),
                        Box::new(Type::NetworkError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "TcpListener")
                    && field == "bind"
                    && arguments.len() == 1
                {
                    let address = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &Type::SocketAddress,
                        &address,
                        arguments[0].span,
                        "TCP listener bind address",
                    )?;
                    return Ok(Type::Result(
                        Box::new(Type::TcpListener),
                        Box::new(Type::NetworkError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Database")
                {
                    match field.as_str() {
                        "open" if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(
                                Box::new(Type::Database),
                                Box::new(Type::DataError),
                            ));
                        }
                        "memory" if arguments.is_empty() => {
                            return Ok(Type::Result(
                                Box::new(Type::Database),
                                Box::new(Type::DataError),
                            ));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no Database constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Port")
                {
                    self.require_explicit_unsafe_capability(
                        Capability::DeviceIo,
                        &format!("hardware port operation `Port.{field}`"),
                        expression.span,
                        "isolate the operation inside `unsafe uses DeviceIo { ... }` after validating the device and port contract",
                    )?;
                    match (field.as_str(), arguments.as_slice()) {
                        ("read_u8", [port]) => {
                            let actual = self.check_expression(port)?;
                            self.require_same(
                                &Type::Unsigned(16),
                                &actual,
                                port.span,
                                "hardware port number",
                            )?;
                            return Ok(Type::Unsigned(8));
                        }
                        ("write_u8", [port, value]) => {
                            let actual = self.check_expression(port)?;
                            self.require_same(
                                &Type::Unsigned(16),
                                &actual,
                                port.span,
                                "hardware port number",
                            )?;
                            let actual = self.check_expression(value)?;
                            self.require_same(
                                &Type::Unsigned(8),
                                &actual,
                                value.span,
                                "hardware port byte",
                            )?;
                            return Ok(Type::Unit);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no protected hardware operation `Port.{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Mmio")
                {
                    self.require_explicit_unsafe_capability(
                        Capability::DeviceIo,
                        &format!("memory-mapped device operation `Mmio.{field}`"),
                        expression.span,
                        "isolate the operation inside `unsafe uses DeviceIo { ... }` after validating the discovered device-register contract",
                    )?;
                    let width = field
                        .strip_prefix("read_")
                        .or_else(|| field.strip_prefix("write_"));
                    let value_type = match width {
                        Some("u8") => Type::Unsigned(8),
                        Some("u16") => Type::Unsigned(16),
                        Some("u32") => Type::Unsigned(32),
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no bounded memory-mapped operation `Mmio.{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    };
                    let write = field.starts_with("write_");
                    let expected_arity = if write { 2 } else { 1 };
                    if arguments.len() != expected_arity {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "no bounded memory-mapped operation `Mmio.{field}` with {} arguments",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let offset = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &Type::Unsigned(16),
                        &offset,
                        arguments[0].span,
                        "memory-mapped device offset",
                    )?;
                    if write {
                        let actual = self.check_expression(&arguments[1])?;
                        self.require_same(
                            &value_type,
                            &actual,
                            arguments[1].span,
                            "memory-mapped device value",
                        )?;
                        return Ok(Type::Unit);
                    }
                    return Ok(value_type);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(owner) = &object.node
                    && matches!(
                        owner.as_str(),
                        "File"
                            | "Directory"
                            | "Time"
                            | "Duration"
                            | "Path"
                            | "Environment"
                            | "Process"
                            | "Crypto"
                    )
                {
                    let io_error = Type::IoError;
                    let result_unit =
                        || Type::Result(Box::new(Type::Unit), Box::new(io_error.clone()));
                    match (owner.as_str(), field.as_str()) {
                        ("Path", "new") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(actual, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Path source must be String or str",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Path);
                        }
                        ("File", "read_text") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(Box::new(Type::String), Box::new(io_error)));
                        }
                        ("File", "read_bytes") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(io_error),
                            ));
                        }
                        ("File", "write_text" | "append_text") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            let text = self.check_expression(&arguments[1])?;
                            if !matches!(text, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "file text must be String or str",
                                    arguments[1].span,
                                ));
                            }
                            return Ok(result_unit());
                        }
                        ("File", "write_bytes" | "append_bytes") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            let bytes = self.check_expression(&arguments[1])?;
                            if !matches!(bytes,Type::List(ref element)|Type::Slice(ref element) if matches!(**element,Type::Unsigned(8)))
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "file bytes must be List<u8> or a u8 slice",
                                    arguments[1].span,
                                ));
                            }
                            return Ok(result_unit());
                        }
                        ("File", "copy" | "move") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            self.require_path(&arguments[1])?;
                            return Ok(result_unit());
                        }
                        ("File", "remove") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(result_unit());
                        }
                        ("File", "exists") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Bool);
                        }
                        ("Directory", "create" | "create_all" | "remove")
                            if arguments.len() == 1 =>
                        {
                            self.require_path(&arguments[0])?;
                            return Ok(result_unit());
                        }
                        ("Directory", "exists") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Bool);
                        }
                        ("Directory", "read") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Path))),
                                Box::new(io_error),
                            ));
                        }
                        ("File", "size" | "modified_seconds") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::Result(Box::new(Type::UInt), Box::new(io_error)));
                        }
                        ("Time", "now") if arguments.is_empty() => return Ok(Type::Instant),
                        ("Time", "unix_seconds") if arguments.is_empty() => return Ok(Type::UInt),
                        ("Time", "ticks") if arguments.is_empty() => {
                            return Ok(Type::Unsigned(32));
                        }
                        ("Time", "sleep") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            self.require_same(
                                &Type::Duration,
                                &actual,
                                arguments[0].span,
                                "sleep duration",
                            )?;
                            return Ok(Type::Unit);
                        }
                        ("Duration", "from_nanos" | "from_millis" | "from_seconds")
                            if arguments.len() == 1 =>
                        {
                            let actual = self.check_expression(&arguments[0])?;
                            if !is_integer(&actual) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "duration value must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Duration);
                        }
                        ("Environment", "arguments") if arguments.is_empty() => {
                            return Ok(Type::List(Box::new(Type::String)));
                        }
                        ("Environment", "get") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !matches!(actual, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "environment variable name must be String or str",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Option(Box::new(Type::String)));
                        }
                        ("Process", "command") if arguments.len() == 1 => {
                            self.require_path(&arguments[0])?;
                            return Ok(Type::ProcessCommand);
                        }
                        ("Process", "run") if arguments.len() == 2 => {
                            self.require_path(&arguments[0])?;
                            let actual = self.check_expression(&arguments[1])?;
                            if !matches!(actual, Type::List(ref element) if matches!(**element, Type::String))
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "process arguments must be List<String>",
                                    arguments[1].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::ProcessOutput),
                                Box::new(Type::IoError),
                            ));
                        }
                        ("Crypto", "random_bytes") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !is_integer(&actual) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "secure-random byte length must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "random_secret") if arguments.len() == 1 => {
                            let actual = self.check_expression(&arguments[0])?;
                            if !is_integer(&actual) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "secure-random secret length must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::SecretBytes),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "import_secret") if arguments.len() == 1 => {
                            let bytes = self.check_expression(&arguments[0])?;
                            if !matches!(bytes, Type::List(ref element) if matches!(**element, Type::Unsigned(8)))
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "secret import must consume List<u8>",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::SecretBytes),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "sha256") if arguments.len() == 1 => {
                            let message = self.check_expression(&arguments[0])?;
                            if !matches!(message, Type::List(ref element) if matches!(**element, Type::Unsigned(8)))
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "SHA-256 message must be List<u8>",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "hmac_sha256" | "hmac_sha256_verify")
                            if arguments.len() == if field == "hmac_sha256" { 2 } else { 3 } =>
                        {
                            let key = self.check_expression(&arguments[0])?;
                            if key != Type::SecretBytes {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "HMAC-SHA-256 key must be SecretBytes",
                                    arguments[0].span,
                                ));
                            }
                            for (index, label) in [(1, "message"), (2, "expected authenticator")] {
                                if index >= arguments.len() {
                                    continue;
                                }
                                let bytes = self.check_expression(&arguments[index])?;
                                if !matches!(bytes, Type::List(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        format!("HMAC-SHA-256 {label} must be List<u8>"),
                                        arguments[index].span,
                                    ));
                                }
                            }
                            return Ok(Type::Result(
                                Box::new(if field == "hmac_sha256" {
                                    Type::List(Box::new(Type::Unsigned(8)))
                                } else {
                                    Type::Bool
                                }),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "hkdf_sha256") if arguments.len() == 4 => {
                            for (index, label) in [(0, "salt"), (2, "info")] {
                                let bytes = self.check_expression(&arguments[index])?;
                                if !matches!(bytes, Type::List(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        format!("HKDF-SHA-256 {label} must be List<u8>"),
                                        arguments[index].span,
                                    ));
                                }
                            }
                            let input = self.check_expression(&arguments[1])?;
                            if input != Type::SecretBytes {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "HKDF-SHA-256 input key material must be SecretBytes",
                                    arguments[1].span,
                                ));
                            }
                            let length = self.check_expression(&arguments[3])?;
                            if !is_integer(&length) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "HKDF-SHA-256 output length must be an integer",
                                    arguments[3].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::SecretBytes),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "aes256_gcm_siv_seal") if arguments.len() == 3 => {
                            for (index, label, expected) in [
                                (0, "key", Type::SecretBytes),
                                (1, "plaintext", Type::SecretBytes),
                            ] {
                                let actual = self.check_expression(&arguments[index])?;
                                if actual != expected {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        format!("AES-256-GCM-SIV {label} must be SecretBytes"),
                                        arguments[index].span,
                                    ));
                                }
                            }
                            self.require_byte_list(
                                &arguments[2],
                                "AES-256-GCM-SIV associated data must be List<u8>",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::AeadEnvelope),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "aes256_gcm_siv_open") if arguments.len() == 3 => {
                            let key = self.check_expression(&arguments[0])?;
                            if key != Type::SecretBytes {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "AES-256-GCM-SIV key must be SecretBytes",
                                    arguments[0].span,
                                ));
                            }
                            let envelope = self.check_expression(&arguments[1])?;
                            if envelope != Type::AeadEnvelope {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "AES-256-GCM-SIV envelope must be AeadEnvelope",
                                    arguments[1].span,
                                ));
                            }
                            self.require_byte_list(
                                &arguments[2],
                                "AES-256-GCM-SIV associated data must be List<u8>",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::SecretBytes),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "encode_aead_envelope") if arguments.len() == 1 => {
                            let envelope = self.check_expression(&arguments[0])?;
                            if envelope != Type::AeadEnvelope {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "AEAD encoding requires AeadEnvelope",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "decode_aead_envelope") if arguments.len() == 1 => {
                            self.require_byte_list(
                                &arguments[0],
                                "AEAD decoding requires List<u8>",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::AeadEnvelope),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_generate") if arguments.is_empty() => {
                            return Ok(Type::Result(
                                Box::new(Type::Ed25519SigningKey),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_public_key") if arguments.len() == 1 => {
                            let key = self.check_expression(&arguments[0])?;
                            if key != Type::Ed25519SigningKey {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Ed25519 signing key must be Ed25519SigningKey",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_sign") if arguments.len() == 2 => {
                            let key = self.check_expression(&arguments[0])?;
                            if key != Type::Ed25519SigningKey {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Ed25519 signing key must be Ed25519SigningKey",
                                    arguments[0].span,
                                ));
                            }
                            self.require_byte_list(
                                &arguments[1],
                                "Ed25519 message must be List<u8>",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_verify") if arguments.len() == 3 => {
                            for (index, label) in
                                [(0, "public key"), (1, "message"), (2, "signature")]
                            {
                                self.require_byte_list(
                                    &arguments[index],
                                    &format!("Ed25519 {label} must be List<u8>"),
                                )?;
                            }
                            return Ok(Type::Result(
                                Box::new(Type::Bool),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_key_id") if arguments.len() == 1 => {
                            self.require_byte_list(
                                &arguments[0],
                                "Ed25519 public key must be List<u8>",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_verify_keyed") if arguments.len() == 4 => {
                            for (index, label) in [
                                (0, "expected key identifier"),
                                (1, "public key"),
                                (2, "message"),
                                (3, "signature"),
                            ] {
                                self.require_byte_list(
                                    &arguments[index],
                                    &format!("Ed25519 {label} must be List<u8>"),
                                )?;
                            }
                            return Ok(Type::Result(
                                Box::new(Type::Bool),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "ed25519_verify_lifecycle") if arguments.len() == 8 => {
                            for (index, label) in [
                                (0, "expected key identifier"),
                                (1, "public key"),
                                (2, "message"),
                                (3, "signature"),
                            ] {
                                self.require_byte_list(
                                    &arguments[index],
                                    &format!("Ed25519 {label} must be List<u8>"),
                                )?;
                            }
                            for (index, label) in [
                                (4, "valid-from"),
                                (5, "valid-until"),
                                (7, "evaluation time"),
                            ] {
                                let actual = self.check_expression(&arguments[index])?;
                                self.require_same(
                                    &Type::UInt,
                                    &actual,
                                    arguments[index].span,
                                    &format!("Ed25519 {label}"),
                                )?;
                            }
                            let revoked = self.check_expression(&arguments[6])?;
                            self.require_same(
                                &Type::Bool,
                                &revoked,
                                arguments[6].span,
                                "Ed25519 revoked state",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::Bool),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        (
                            "Crypto",
                            "encode_ed25519_public_key"
                            | "decode_ed25519_public_key"
                            | "encode_ed25519_signature"
                            | "decode_ed25519_signature",
                        ) if arguments.len() == 1 => {
                            self.require_byte_list(
                                &arguments[0],
                                "Ed25519 record conversion requires List<u8>",
                            )?;
                            return Ok(Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "argon2id_hash_password") if arguments.len() == 1 => {
                            let password = self.check_expression(&arguments[0])?;
                            if password != Type::SecretBytes {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Argon2id password must be SecretBytes",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::String),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        ("Crypto", "argon2id_verify_password") if arguments.len() == 2 => {
                            let password = self.check_expression(&arguments[0])?;
                            if password != Type::SecretBytes {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Argon2id password must be SecretBytes",
                                    arguments[0].span,
                                ));
                            }
                            let encoded = self.check_expression(&arguments[1])?;
                            if !matches!(encoded, Type::String | Type::Str) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Argon2id encoded hash must be String or str",
                                    arguments[1].span,
                                ));
                            }
                            return Ok(Type::Result(
                                Box::new(Type::Bool),
                                Box::new(Type::CryptoError),
                            ));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no system operation `{owner}.{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "String")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => return Ok(Type::String),
                        "with_capacity" if arguments.len() == 1 => {
                            self.check_expression(&arguments[0])?;
                            return Ok(Type::String);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no String constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "CString")
                {
                    if field != "new" || arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "no CString constructor `{field}` with {} arguments",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !matches!(source, Type::String | Type::Str | Type::CStr) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "CString.new expects String, str, or CStr",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(Type::CString),
                        Box::new(Type::String),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "CRegistration")
                {
                    if field == "register_async" {
                        if arguments.len() != 4 {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no CRegistration constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                        match &arguments[0].node {
                            Expression::Closure {
                                move_captures: true,
                                ..
                            } => {}
                            Expression::Identifier(name)
                                if self.functions.contains_key(name)
                                    && !self.external_functions.contains(name) => {}
                            Expression::Closure { .. } => {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "asynchronous C callbacks must use a `move` closure",
                                    arguments[0].span,
                                )
                                .with_help("move captures into the registration so borrowed storage cannot outlive its owner"));
                            }
                            _ => {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "CRegistration.register_async expects a direct named DISP function or `move` closure handler",
                                    arguments[0].span,
                                ));
                            }
                        }
                        self.require_explicit_unsafe_capability(
                            Capability::Foreign,
                            "captured asynchronous foreign callback registration",
                            expression.span,
                            "register only with a provider that retains the callback/context pair until quiesce returns and never calls it afterward",
                        )?;
                        if let Expression::Closure {
                            move_captures: true,
                            parameters,
                            body,
                            ..
                        } = &arguments[0].node
                        {
                            for (name, capture) in
                                crate::ast::closure_capture_uses(parameters, body)
                            {
                                let Some(variable) = self.lookup_variable(&name) else {
                                    continue;
                                };
                                if !self.type_is_send(&variable.ty, &mut HashSet::new()) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        format!(
                                            "captured {} cannot be transferred to an asynchronous C callback",
                                            self.format_type(&variable.ty)
                                        ),
                                        capture.span,
                                    )
                                    .with_help("move only owned Send-compatible values into the callback; secrets, references, pointers, function values, guards, and registrations cannot cross this boundary"));
                                }
                            }
                        }
                        let handler = self.check_expression(&arguments[0])?;
                        let Type::Function(parameters, result) = handler else {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "C callback handler must be an ordinary synchronous DISP function",
                                arguments[0].span,
                            ));
                        };
                        if parameters
                            .iter()
                            .any(|parameter| !self.ffi_parameter_type(parameter))
                            || !self.ffi_result_type(&result)
                        {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "captured C callback handler uses a type without a stable C ABI",
                                arguments[0].span,
                            )
                            .with_help("use C ABI scalars, CStr, or explicit raw pointers"));
                        }
                        let context = Type::RawPointer(Box::new(Type::Unit), true);
                        let mut trampoline_parameters = vec![context.clone()];
                        trampoline_parameters.extend(parameters);
                        if !matches!(*result, Type::Unit) {
                            trampoline_parameters
                                .push(Type::RawPointer(Box::new((*result).clone()), true));
                        }
                        let trampoline =
                            Type::CFunction(trampoline_parameters, Box::new(Type::Signed(32)));
                        let register = Type::CFunction(
                            vec![trampoline, context.clone()],
                            Box::new(context.clone()),
                        );
                        let actual_register = self.check_expression(&arguments[1])?;
                        self.require_same(
                            &register,
                            &actual_register,
                            arguments[1].span,
                            "callback provider register function",
                        )?;
                        let cleanup = Type::CFunction(vec![context.clone()], Box::new(Type::Unit));
                        for (index, description) in [(2, "quiesce"), (3, "release")] {
                            let actual = self.check_expression(&arguments[index])?;
                            self.require_same(
                                &cleanup,
                                &actual,
                                arguments[index].span,
                                &format!("callback provider {description} function"),
                            )?;
                        }
                        return Ok(Type::CRegistration);
                    }
                    let asynchronous = field == "adopt_async";
                    if !((field == "adopt" && arguments.len() == 2)
                        || (asynchronous && arguments.len() == 3))
                    {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "no CRegistration constructor `{field}` with {} arguments",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    self.require_explicit_unsafe_capability(
                        Capability::Foreign,
                        if asynchronous {
                            "asynchronous foreign callback registration adoption"
                        } else {
                            "foreign callback registration adoption"
                        },
                        expression.span,
                        if asynchronous {
                            "adopt an asynchronous registration only with exact quiesce and release callbacks whose provider contract waits for all in-flight calls"
                        } else {
                            "adopt a registration only after the provider has returned a live context and exact release callback"
                        },
                    )?;
                    let context = Type::RawPointer(Box::new(Type::Unit), true);
                    let release = Type::CFunction(vec![context.clone()], Box::new(Type::Unit));
                    let actual_context = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &context,
                        &actual_context,
                        arguments[0].span,
                        "registration context",
                    )?;
                    if asynchronous {
                        let actual_quiesce = self.check_expression(&arguments[1])?;
                        self.require_same(
                            &release,
                            &actual_quiesce,
                            arguments[1].span,
                            "registration quiesce callback",
                        )?;
                    }
                    let release_index = if asynchronous { 2 } else { 1 };
                    let actual_release = self.check_expression(&arguments[release_index])?;
                    self.require_same(
                        &release,
                        &actual_release,
                        arguments[release_index].span,
                        "registration release callback",
                    )?;
                    return Ok(Type::CRegistration);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "CExport")
                {
                    if field != "callback" || arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "no CExport operation `{field}` with {} arguments",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let Expression::Identifier(name) = &arguments[0].node else {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "CExport.callback expects the name of an exported C function",
                            arguments[0].span,
                        ));
                    };
                    if !self.exported_functions.contains(name) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{name}` is not an `export C fn`"),
                            arguments[0].span,
                        )
                        .with_help(
                            "mark a synchronous, non-generic ABI-safe function `export C`",
                        ));
                    }
                    let signature = &self.functions[name];
                    let mut parameters = signature.parameters.clone();
                    if !matches!(signature.result, Type::Unit) {
                        parameters.push(Type::RawPointer(Box::new(signature.result.clone()), true));
                    }
                    return Ok(Type::CFunction(parameters, Box::new(Type::Signed(32))));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Memory")
                {
                    if field != "allocate" || arguments.len() != 2 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "no Memory constructor `{field}` with {} arguments",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    for (index, argument) in arguments.iter().enumerate() {
                        let actual = self.check_expression(argument)?;
                        if !is_integer(&actual) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                if index == 0 {
                                    "Memory size must be an integer"
                                } else {
                                    "Memory alignment must be an integer"
                                },
                                argument.span,
                            ));
                        }
                    }
                    return Ok(Type::Result(Box::new(Type::Memory), Box::new(Type::String)));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "List")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => {
                            return Ok(Type::List(Box::new(Type::Infer)));
                        }
                        "of" if !arguments.is_empty() => {
                            let first = self.check_expression(&arguments[0])?;
                            let element = materialize_literal(first);
                            for argument in &arguments[1..] {
                                let actual = self.check_expression(argument)?;
                                self.require_same(
                                    &element,
                                    &actual,
                                    argument.span,
                                    "List element",
                                )?;
                            }
                            return Ok(Type::List(Box::new(element)));
                        }
                        "with_capacity" if arguments.len() == 1 => {
                            let capacity = self.check_expression(&arguments[0])?;
                            if !is_integer(&capacity) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "List capacity must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::List(Box::new(Type::Infer)));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no List constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Mutex")
                {
                    if field == "new" && arguments.len() == 1 {
                        let value = materialize_literal(self.check_expression(&arguments[0])?);
                        return Ok(Type::Mutex(Box::new(value)));
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "no Mutex constructor `{field}` with {} arguments",
                            arguments.len()
                        ),
                        expression.span,
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "AtomicInt")
                {
                    if field == "new" && arguments.len() == 1 {
                        let value = self.check_expression(&arguments[0])?;
                        self.require_same(
                            &Type::Int,
                            &value,
                            arguments[0].span,
                            "AtomicInt value",
                        )?;
                        return Ok(Type::AtomicInt);
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "no AtomicInt constructor `{field}` with {} arguments",
                            arguments.len()
                        ),
                        expression.span,
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Channel")
                {
                    if field == "bounded" && arguments.len() == 1 {
                        let capacity = self.check_expression(&arguments[0])?;
                        if !is_integer(&capacity) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "Channel capacity must be an integer",
                                arguments[0].span,
                            ));
                        }
                        return Ok(Type::Result(
                            Box::new(Type::Channel(Box::new(Type::Infer))),
                            Box::new(Type::String),
                        ));
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "no Channel constructor `{field}` with {} arguments",
                            arguments.len()
                        ),
                        expression.span,
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Map")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => {
                            return Ok(Type::Map(Box::new(Type::Infer), Box::new(Type::Infer)));
                        }
                        "with_capacity" if arguments.len() == 1 => {
                            let capacity = self.check_expression(&arguments[0])?;
                            if !is_integer(&capacity) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Map capacity must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Map(Box::new(Type::Infer), Box::new(Type::Infer)));
                        }
                        "of" if !arguments.is_empty() && arguments.len() % 2 == 0 => {
                            let key = materialize_literal(self.check_expression(&arguments[0])?);
                            if !is_collection_key(&key) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Map keys must be integers, bool, char, String, or str",
                                    arguments[0].span,
                                ));
                            }
                            let value = materialize_literal(self.check_expression(&arguments[1])?);
                            for pair in arguments[2..].chunks_exact(2) {
                                let actual_key = self.check_expression(&pair[0])?;
                                let actual_value = self.check_expression(&pair[1])?;
                                self.require_same(&key, &actual_key, pair[0].span, "Map key")?;
                                self.require_same(
                                    &value,
                                    &actual_value,
                                    pair[1].span,
                                    "Map value",
                                )?;
                            }
                            return Ok(Type::Map(Box::new(key), Box::new(value)));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no Map constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Set")
                {
                    match field.as_str() {
                        "new" if arguments.is_empty() => {
                            return Ok(Type::Set(Box::new(Type::Infer)));
                        }
                        "with_capacity" if arguments.len() == 1 => {
                            let capacity = self.check_expression(&arguments[0])?;
                            if !is_integer(&capacity) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Set capacity must be an integer",
                                    arguments[0].span,
                                ));
                            }
                            return Ok(Type::Set(Box::new(Type::Infer)));
                        }
                        "of" if !arguments.is_empty() => {
                            let element =
                                materialize_literal(self.check_expression(&arguments[0])?);
                            if !is_collection_key(&element) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "Set elements must be integers, bool, char, String, or str",
                                    arguments[0].span,
                                ));
                            }
                            for argument in &arguments[1..] {
                                let actual = self.check_expression(argument)?;
                                self.require_same(&element, &actual, argument.span, "Set element")?;
                            }
                            return Ok(Type::Set(Box::new(element)));
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "no Set constructor `{field}` with {} arguments",
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                    }
                }
                if let Expression::Identifier(name) = &callee.node
                    && let Some(target) = numeric_type(name)
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "numeric conversion `{name}` expects 1 argument, found {}",
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !is_numeric(&source) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("cannot convert {} to {name}", self.format_type(&source)),
                            arguments[0].span,
                        ));
                    }
                    return Ok(target);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(name) = &object.node
                    && let Some(target) = numeric_type(name)
                    && field == "try_from"
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{name}.try_from` expects 1 argument"),
                            expression.span,
                        ));
                    }
                    let source = self.check_expression(&arguments[0])?;
                    if !is_numeric(&source) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "checked conversion requires a numeric value",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Result(
                        Box::new(target),
                        Box::new(Type::ConversionError),
                    ));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(
                        field.as_str(),
                        "wrapping_add"
                            | "wrapping_sub"
                            | "wrapping_mul"
                            | "saturating_add"
                            | "saturating_sub"
                            | "saturating_mul"
                    )
                {
                    let receiver = self.check_expression(object)?;
                    if !is_integer(&receiver) || matches!(receiver, Type::IntLiteral(_)) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{field}` requires a concretely typed integer"),
                            object.span,
                        ));
                    }
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{field}` expects 1 argument"),
                            expression.span,
                        ));
                    }
                    let argument = self.check_expression(&arguments[0])?;
                    self.require_same(
                        &receiver,
                        &argument,
                        arguments[0].span,
                        "integer method argument",
                    )?;
                    return Ok(receiver);
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && !matches!(&object.node, Expression::Identifier(name) if matches!(self.types.get(name), Some(Type::Enum(_, _))))
                {
                    let receiver = self.check_expression(object)?;
                    if matches!(
                        receiver,
                        Type::Array(_, _)
                            | Type::Slice(_)
                            | Type::List(_)
                            | Type::Map(_, _)
                            | Type::Set(_)
                    ) && matches!(field.as_str(), "len" | "is_empty" | "count" | "empty")
                    {
                        if !arguments.is_empty() {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`len` expects no arguments",
                                expression.span,
                            ));
                        }
                        return Ok(if matches!(field.as_str(), "is_empty" | "empty") {
                            Type::Bool
                        } else {
                            Type::UInt
                        });
                    }
                    if let Type::Array(element, _) | Type::Slice(element) = &receiver
                        && field == "iter"
                        && arguments.is_empty()
                    {
                        return Ok(Type::Slice(element.clone()));
                    }
                    if let Type::List(element) = &receiver {
                        let field = match field.as_str() {
                            "add" => "push",
                            "count" => "len",
                            "empty" => "is_empty",
                            other => other,
                        };
                        match field {
                            "iter" if arguments.is_empty() => {
                                return Ok(Type::Slice(element.clone()));
                            }
                            "capacity" if arguments.is_empty() => return Ok(Type::UInt),
                            "push" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[0].span,
                                    "List element",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "pop" if arguments.is_empty() => {
                                return Ok(Type::Option(element.clone()));
                            }
                            "get" | "get_mut" if arguments.len() == 1 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "List access index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Option(Box::new(Type::Reference(
                                    element.clone(),
                                    field == "get_mut",
                                ))));
                            }
                            "insert" if arguments.len() == 2 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "List insertion index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let actual = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[1].span,
                                    "List element",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "remove" if arguments.len() == 1 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "List removal index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok((**element).clone());
                            }
                            "clear" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if let Type::Map(key, value) = &receiver {
                        match field.as_str() {
                            "keys" if arguments.is_empty() => return Ok(Type::Slice(key.clone())),
                            "values" if arguments.is_empty() => {
                                return Ok(Type::Slice(value.clone()));
                            }
                            "capacity" if arguments.is_empty() => return Ok(Type::UInt),
                            "has" | "contains_key" | "get" | "get_mut" | "remove"
                                if arguments.len() == 1 =>
                            {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(key, &actual, arguments[0].span, "Map key")?;
                                return Ok(match field.as_str() {
                                    "has" | "contains_key" => Type::Bool,
                                    "get" => Type::Option(Box::new(Type::Reference(
                                        value.clone(),
                                        false,
                                    ))),
                                    "get_mut" => {
                                        Type::Option(Box::new(Type::Reference(value.clone(), true)))
                                    }
                                    _ => Type::Option(value.clone()),
                                });
                            }
                            "set" | "insert" if arguments.len() == 2 => {
                                let actual_key = self.check_expression(&arguments[0])?;
                                let actual_value = self.check_expression(&arguments[1])?;
                                self.require_same(key, &actual_key, arguments[0].span, "Map key")?;
                                self.require_same(
                                    value,
                                    &actual_value,
                                    arguments[1].span,
                                    "Map value",
                                )?;
                                return Ok(Type::Option(value.clone()));
                            }
                            "clear" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if let Type::Set(element) = &receiver {
                        match field.as_str() {
                            "iter" if arguments.is_empty() => {
                                return Ok(Type::Slice(element.clone()));
                            }
                            "capacity" if arguments.is_empty() => return Ok(Type::UInt),
                            "has" | "contains" | "remove" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[0].span,
                                    "Set element",
                                )?;
                                return Ok(Type::Bool);
                            }
                            "add" | "insert" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    element,
                                    &actual,
                                    arguments[0].span,
                                    "Set element",
                                )?;
                                return Ok(Type::Bool);
                            }
                            "clear" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if let Type::Thread(result) = &receiver
                        && field == "join"
                        && arguments.is_empty()
                    {
                        return Ok((**result).clone());
                    }
                    if matches!(receiver, Type::Task(_)) && arguments.is_empty() {
                        match field.as_str() {
                            "cancel" => return Ok(Type::Unit),
                            "is_finished" => return Ok(Type::Bool),
                            _ => {}
                        }
                    }
                    if let Type::Mutex(value) = &receiver {
                        if field == "share" && arguments.is_empty() {
                            return Ok(Type::Mutex(value.clone()));
                        }
                        if field == "lock" && arguments.is_empty() {
                            return Ok(Type::MutexGuard(value.clone()));
                        }
                    }
                    if let Type::Channel(value) = &receiver {
                        match field.as_str() {
                            "share" if arguments.is_empty() => {
                                return Ok(Type::Channel(value.clone()));
                            }
                            "send" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    value,
                                    &actual,
                                    arguments[0].span,
                                    "Channel message",
                                )?;
                                return Ok(Type::Bool);
                            }
                            "receive" if arguments.is_empty() => {
                                return Ok(Type::Option(value.clone()));
                            }
                            "close" if arguments.is_empty() => return Ok(Type::Unit),
                            "len" | "capacity" if arguments.is_empty() => {
                                return Ok(Type::UInt);
                            }
                            "is_closed" if arguments.is_empty() => return Ok(Type::Bool),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::AtomicInt) {
                        if field == "share" && arguments.is_empty() {
                            return Ok(Type::AtomicInt);
                        }
                        if atomic_load_method(field) && arguments.is_empty() {
                            return Ok(Type::Int);
                        }
                        if (atomic_store_method(field)
                            || atomic_add_method(field)
                            || atomic_fetch_add_method(field))
                            && arguments.len() == 1
                        {
                            let value = self.check_expression(&arguments[0])?;
                            self.require_same(
                                &Type::Int,
                                &value,
                                arguments[0].span,
                                "AtomicInt value",
                            )?;
                            return Ok(if atomic_store_method(field) {
                                Type::Unit
                            } else {
                                Type::Int
                            });
                        }
                    }
                    if matches!(receiver, Type::CString | Type::CStr) {
                        match field.as_str() {
                            "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_empty" if arguments.is_empty() => return Ok(Type::Bool),
                            "to_string" if arguments.is_empty() => return Ok(Type::String),
                            "as_c_str"
                                if arguments.is_empty() && matches!(receiver, Type::CString) =>
                            {
                                return Ok(Type::CStr);
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::SecretBytes) {
                        match field.as_str() {
                            "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_empty" if arguments.is_empty() => return Ok(Type::Bool),
                            "constant_time_equals" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::SecretBytes,
                                    &actual,
                                    arguments[0].span,
                                    "constant-time secret comparison",
                                )?;
                                return Ok(Type::Bool);
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Memory) {
                        match field.as_str() {
                            "len" | "alignment" if arguments.is_empty() => {
                                return Ok(Type::UInt);
                            }
                            "is_empty" if arguments.is_empty() => return Ok(Type::Bool),
                            "as_ptr" if arguments.is_empty() => {
                                return Ok(Type::MemoryPointer(Box::new(Type::Unsigned(8)), false));
                            }
                            "as_mut_ptr" if arguments.is_empty() => {
                                return Ok(Type::MemoryPointer(Box::new(Type::Unsigned(8)), true));
                            }
                            "read" if arguments.len() == 1 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Memory index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Unsigned(8));
                            }
                            "write" if arguments.len() == 2 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Memory index must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let byte = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Unsigned(8),
                                    &byte,
                                    arguments[1].span,
                                    "Memory byte",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "fill" if arguments.len() == 1 => {
                                let byte = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::Unsigned(8),
                                    &byte,
                                    arguments[0].span,
                                    "Memory byte",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "copy_from" if arguments.len() == 4 => {
                                let destination = self.check_expression(&arguments[0])?;
                                if !is_integer(&destination) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Memory destination offset must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let source = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Memory,
                                    &source,
                                    arguments[1].span,
                                    "Memory copy source",
                                )?;
                                for argument in &arguments[2..] {
                                    let actual = self.check_expression(argument)?;
                                    if !is_integer(&actual) {
                                        return Err(Diagnostic::new(
                                            DiagnosticKind::Type,
                                            "Memory source offset and count must be integers",
                                            argument.span,
                                        ));
                                    }
                                }
                                return Ok(Type::Unit);
                            }
                            _ => {}
                        }
                    }
                    if let Type::RawPointer(inner, mutable) | Type::MemoryPointer(inner, mutable) =
                        &receiver
                        && matches!(field.as_str(), "offset" | "read" | "write")
                    {
                        self.require_unsafe_capability(
                            Capability::RawMemory,
                            &format!("raw pointer operation `{field}`"),
                            expression.span,
                            "prove the pointer is live, aligned, and in bounds before using it",
                        )?;
                        if !self.type_is_copy(inner) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "raw pointer read/write currently requires a Copy element type",
                                object.span,
                            ));
                        }
                        match field.as_str() {
                            "offset" if arguments.len() == 1 => {
                                let offset = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::Int,
                                    &offset,
                                    arguments[0].span,
                                    "pointer offset",
                                )?;
                                return Ok(receiver);
                            }
                            "read" if arguments.is_empty() => return Ok((**inner).clone()),
                            "write" if arguments.len() == 1 && *mutable => {
                                let value = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    inner,
                                    &value,
                                    arguments[0].span,
                                    "pointer write value",
                                )?;
                                return Ok(Type::Unit);
                            }
                            "write" if !*mutable => {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "cannot write through a const raw pointer",
                                    object.span,
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::CRegistration) {
                        match field.as_str() {
                            "close" if arguments.is_empty() => return Ok(Type::Unit),
                            "is_active" if arguments.is_empty() => return Ok(Type::Bool),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Path) {
                        match field.as_str() {
                            "join" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                if !matches!(actual, Type::String | Type::Str | Type::Path) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Path.join expects Path, String, or str",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Path);
                            }
                            "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_empty" | "is_absolute" if arguments.is_empty() => {
                                return Ok(Type::Bool);
                            }
                            "as_string" if arguments.is_empty() => return Ok(Type::String),
                            "name" | "extension" if arguments.is_empty() => {
                                return Ok(Type::Option(Box::new(Type::String)));
                            }
                            "parent" if arguments.is_empty() => {
                                return Ok(Type::Option(Box::new(Type::Path)));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::ProcessOutput) {
                        match field.as_str() {
                            "status" if arguments.is_empty() => return Ok(Type::Int),
                            "success" if arguments.is_empty() => return Ok(Type::Bool),
                            "stdout" | "stderr" if arguments.is_empty() => {
                                return Ok(Type::List(Box::new(Type::Unsigned(8))));
                            }
                            "stdout_text" | "stderr_text" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::String),
                                    Box::new(Type::ConversionError),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::ProcessCommand) {
                        match field.as_str() {
                            "arg" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                if !matches!(actual, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "process argument must be String or str",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::ProcessCommand);
                            }
                            "arguments" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                if !matches!(actual, Type::List(ref element) if matches!(**element, Type::String))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "process arguments must be List<String>",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::ProcessCommand);
                            }
                            "directory" if arguments.len() == 1 => {
                                self.require_path(&arguments[0])?;
                                return Ok(Type::ProcessCommand);
                            }
                            "environment" if arguments.len() == 2 => {
                                for argument in arguments {
                                    let actual = self.check_expression(argument)?;
                                    if !matches!(actual, Type::String | Type::Str) {
                                        return Err(Diagnostic::new(
                                            DiagnosticKind::Type,
                                            "process environment names and values must be String or str",
                                            argument.span,
                                        ));
                                    }
                                }
                                return Ok(Type::ProcessCommand);
                            }
                            "clear_environment" if arguments.is_empty() => {
                                return Ok(Type::ProcessCommand);
                            }
                            "input" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                if !matches!(actual, Type::List(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "process input must be List<u8>",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::ProcessCommand);
                            }
                            "input_text" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                if !matches!(actual, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "process text input must be String or str",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::ProcessCommand);
                            }
                            "timeout" if arguments.len() == 1 => {
                                let actual = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::Duration,
                                    &actual,
                                    arguments[0].span,
                                    "process timeout",
                                )?;
                                return Ok(Type::ProcessCommand);
                            }
                            "run" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::ProcessOutput),
                                    Box::new(Type::IoError),
                                ));
                            }
                            "start" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::ChildProcess),
                                    Box::new(Type::IoError),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::ChildProcess) {
                        match field.as_str() {
                            "write" if arguments.len() == 1 => {
                                let bytes = self.check_expression(&arguments[0])?;
                                if !matches!(bytes, Type::List(ref element) | Type::Slice(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "child-process write expects List<u8> or a u8 slice",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::Unit),
                                    Box::new(Type::IoError),
                                ));
                            }
                            "write_text" if arguments.len() == 1 => {
                                let text = self.check_expression(&arguments[0])?;
                                if !matches!(text, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "child-process text input must be String or str",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::Unit),
                                    Box::new(Type::IoError),
                                ));
                            }
                            "read_stdout" | "read_stderr" if arguments.len() == 1 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "child-process read limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                    Box::new(Type::IoError),
                                ));
                            }
                            "close_input" | "kill" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Unit),
                                    Box::new(Type::IoError),
                                ));
                            }
                            "try_wait" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Option(Box::new(Type::Int))),
                                    Box::new(Type::IoError),
                                ));
                            }
                            "wait" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::ProcessOutput),
                                    Box::new(Type::IoError),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Database) {
                        match field.as_str() {
                            "execute" | "query" if arguments.len() == 2 => {
                                let sql = self.check_expression(&arguments[0])?;
                                if !matches!(sql, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "database SQL must be String or str",
                                        arguments[0].span,
                                    ));
                                }
                                let parameters = self.check_expression(&arguments[1])?;
                                if !matches!(parameters, Type::List(ref value) if matches!(**value, Type::Json))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "database parameters must be List<Json>",
                                        arguments[1].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(if field == "execute" {
                                        Type::UInt
                                    } else {
                                        Type::List(Box::new(Type::Json))
                                    }),
                                    Box::new(Type::DataError),
                                ));
                            }
                            "begin" | "commit" | "rollback" | "close" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Unit),
                                    Box::new(Type::DataError),
                                ));
                            }
                            "changes" if arguments.is_empty() => return Ok(Type::UInt),
                            "last_insert_id" if arguments.is_empty() => return Ok(Type::Int),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Url) {
                        match field.as_str() {
                            "as_string" | "scheme" | "path" if arguments.is_empty() => {
                                return Ok(Type::String);
                            }
                            "host" | "query" if arguments.is_empty() => {
                                return Ok(Type::Option(Box::new(Type::String)));
                            }
                            "port" if arguments.is_empty() => {
                                return Ok(Type::Option(Box::new(Type::UInt)));
                            }
                            "is_secure" if arguments.is_empty() => return Ok(Type::Bool),
                            "join_path" if arguments.len() == 1 => {
                                let segment = self.check_expression(&arguments[0])?;
                                if !matches!(segment, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Url.join_path expects a String or str segment",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::Url),
                                    Box::new(Type::NetworkError),
                                ));
                            }
                            "query_param" if arguments.len() == 2 => {
                                for argument in arguments {
                                    let actual = self.check_expression(argument)?;
                                    if !matches!(actual, Type::String | Type::Str) {
                                        return Err(Diagnostic::new(
                                            DiagnosticKind::Type,
                                            "Url.query_param expects String or str name and value",
                                            argument.span,
                                        ));
                                    }
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::Url),
                                    Box::new(Type::NetworkError),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Json) {
                        match field.as_str() {
                            "as_string" | "kind" if arguments.is_empty() => {
                                return Ok(Type::String);
                            }
                            "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_null" | "is_bool" | "is_number" | "is_string" | "is_array"
                            | "is_object"
                                if arguments.is_empty() =>
                            {
                                return Ok(Type::Bool);
                            }
                            "get" if arguments.len() == 1 => {
                                let key = self.check_expression(&arguments[0])?;
                                if !matches!(key, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Json.get expects a String or str key",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Option(Box::new(Type::Json)));
                            }
                            "at" if arguments.len() == 1 => {
                                let index = self.check_expression(&arguments[0])?;
                                if !is_integer(&index) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "Json.at expects an integer index",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Option(Box::new(Type::Json)));
                            }
                            "as_bool" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Bool),
                                    Box::new(Type::ConversionError),
                                ));
                            }
                            "as_int" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Int),
                                    Box::new(Type::ConversionError),
                                ));
                            }
                            "as_uint" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::UInt),
                                    Box::new(Type::ConversionError),
                                ));
                            }
                            "as_f64" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Float),
                                    Box::new(Type::ConversionError),
                                ));
                            }
                            "as_text" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::String),
                                    Box::new(Type::ConversionError),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::TcpStream) {
                        match field.as_str() {
                            "read" | "read_async" if arguments.len() == 1 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TCP read limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let result = Type::Result(
                                    Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                    Box::new(Type::NetworkError),
                                );
                                return Ok(if field == "read_async" {
                                    Type::Future(Box::new(result))
                                } else {
                                    result
                                });
                            }
                            "read_async_timeout" if arguments.len() == 2 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TCP read limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let timeout = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[1].span,
                                    "TCP read timeout",
                                )?;
                                return Ok(Type::Future(Box::new(Type::Result(
                                    Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                    Box::new(Type::NetworkError),
                                ))));
                            }
                            "write" | "write_async" if arguments.len() == 1 => {
                                let bytes = self.check_expression(&arguments[0])?;
                                if !matches!(bytes, Type::List(ref element) | Type::Slice(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TCP write expects List<u8> or a u8 slice",
                                        arguments[0].span,
                                    ));
                                }
                                let result = Type::Result(
                                    Box::new(Type::UInt),
                                    Box::new(Type::NetworkError),
                                );
                                return Ok(if field == "write_async" {
                                    Type::Future(Box::new(result))
                                } else {
                                    result
                                });
                            }
                            "write_async_timeout" if arguments.len() == 2 => {
                                let bytes = self.check_expression(&arguments[0])?;
                                if !matches!(bytes, Type::List(ref element) | Type::Slice(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TCP write expects List<u8> or a u8 slice",
                                        arguments[0].span,
                                    ));
                                }
                                let timeout = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[1].span,
                                    "TCP write timeout",
                                )?;
                                return Ok(Type::Future(Box::new(Type::Result(
                                    Box::new(Type::UInt),
                                    Box::new(Type::NetworkError),
                                ))));
                            }
                            "close" if arguments.is_empty() => return Ok(Type::Unit),
                            "shutdown_read" | "shutdown_write" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Unit),
                                    Box::new(Type::NetworkError),
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::TlsStream) {
                        let read_result = || {
                            Type::Result(
                                Box::new(Type::List(Box::new(Type::Unsigned(8)))),
                                Box::new(Type::NetworkError),
                            )
                        };
                        let write_result =
                            || Type::Result(Box::new(Type::UInt), Box::new(Type::NetworkError));
                        match field.as_str() {
                            "read" | "read_async" if arguments.len() == 1 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TLS read limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let result = read_result();
                                return Ok(if field == "read_async" {
                                    Type::Future(Box::new(result))
                                } else {
                                    result
                                });
                            }
                            "read_async_timeout" if arguments.len() == 2 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TLS read limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let timeout = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[1].span,
                                    "TLS read timeout",
                                )?;
                                return Ok(Type::Future(Box::new(read_result())));
                            }
                            "write" | "write_async" if arguments.len() == 1 => {
                                let bytes = self.check_expression(&arguments[0])?;
                                if !matches!(bytes, Type::List(ref element) | Type::Slice(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TLS write expects List<u8> or a u8 slice",
                                        arguments[0].span,
                                    ));
                                }
                                let result = write_result();
                                return Ok(if field == "write_async" {
                                    Type::Future(Box::new(result))
                                } else {
                                    result
                                });
                            }
                            "write_async_timeout" if arguments.len() == 2 => {
                                let bytes = self.check_expression(&arguments[0])?;
                                if !matches!(bytes, Type::List(ref element) | Type::Slice(ref element) if matches!(**element, Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "TLS write expects List<u8> or a u8 slice",
                                        arguments[0].span,
                                    ));
                                }
                                let timeout = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[1].span,
                                    "TLS write timeout",
                                )?;
                                return Ok(Type::Future(Box::new(write_result())));
                            }
                            "close" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::HttpResponse) {
                        match field.as_str() {
                            "status" | "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_success" | "is_empty" if arguments.is_empty() => {
                                return Ok(Type::Bool);
                            }
                            "body" if arguments.is_empty() => {
                                return Ok(Type::List(Box::new(Type::Unsigned(8))));
                            }
                            "text" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::String),
                                    Box::new(Type::HttpError),
                                ));
                            }
                            "json" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::Json),
                                    Box::new(Type::HttpError),
                                ));
                            }
                            "url" if arguments.is_empty() => return Ok(Type::String),
                            "header" if arguments.len() == 1 => {
                                let name = self.check_expression(&arguments[0])?;
                                if !matches!(name, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "HTTP header name must be String or str",
                                        arguments[0].span,
                                    ));
                                }
                                if let Expression::String(name) = &arguments[0].node
                                    && !http_header_token(name)
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "HTTP header name contains invalid characters",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Option(Box::new(Type::String)));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::HttpRequest) {
                        match field.as_str() {
                            "header" if arguments.len() == 2 => {
                                for (argument, description) in arguments
                                    .iter()
                                    .zip(["HTTP header name", "HTTP header value"])
                                {
                                    let actual = self.check_expression(argument)?;
                                    if !matches!(actual, Type::String | Type::Str) {
                                        return Err(Diagnostic::new(
                                            DiagnosticKind::Type,
                                            format!("{description} must be String or str"),
                                            argument.span,
                                        ));
                                    }
                                }
                                if let Expression::String(name) = &arguments[0].node
                                    && (!http_header_token(name) || http_forbidden_header(name))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "HTTP header name is invalid or controlled by the safe client",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::HttpRequest),
                                    Box::new(Type::HttpError),
                                ));
                            }
                            "text" if arguments.len() == 1 => {
                                let body = self.check_expression(&arguments[0])?;
                                if !matches!(body, Type::String | Type::Str) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "HTTP text body must be String or str",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::HttpRequest),
                                    Box::new(Type::HttpError),
                                ));
                            }
                            "bytes" if arguments.len() == 1 => {
                                let body = self.check_expression(&arguments[0])?;
                                if !matches!(body,Type::List(ref element)|Type::Slice(ref element) if matches!(**element,Type::Unsigned(8)))
                                {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "HTTP byte body must be List<u8> or a u8 slice",
                                        arguments[0].span,
                                    ));
                                }
                                return Ok(Type::Result(
                                    Box::new(Type::HttpRequest),
                                    Box::new(Type::HttpError),
                                ));
                            }
                            "json" if arguments.len() == 1 => {
                                let body = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::Json,
                                    &body,
                                    arguments[0].span,
                                    "HTTP JSON body",
                                )?;
                                return Ok(Type::Result(
                                    Box::new(Type::HttpRequest),
                                    Box::new(Type::HttpError),
                                ));
                            }
                            "send" if arguments.is_empty() => {
                                return Ok(Type::Future(Box::new(Type::Result(
                                    Box::new(Type::HttpResponse),
                                    Box::new(Type::HttpError),
                                ))));
                            }
                            "send_timeout" if arguments.len() == 1 => {
                                let timeout = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[0].span,
                                    "HTTP request timeout",
                                )?;
                                return Ok(Type::Future(Box::new(Type::Result(
                                    Box::new(Type::HttpResponse),
                                    Box::new(Type::HttpError),
                                ))));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::TcpListener) {
                        let accepted = || {
                            Type::Future(Box::new(Type::Result(
                                Box::new(Type::TcpStream),
                                Box::new(Type::NetworkError),
                            )))
                        };
                        match field.as_str() {
                            "accept" if arguments.is_empty() => return Ok(accepted()),
                            "accept_timeout" if arguments.len() == 1 => {
                                let timeout = self.check_expression(&arguments[0])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[0].span,
                                    "TCP accept timeout",
                                )?;
                                return Ok(accepted());
                            }
                            "local_port" if arguments.is_empty() => {
                                return Ok(Type::Result(
                                    Box::new(Type::UInt),
                                    Box::new(Type::NetworkError),
                                ));
                            }
                            "close" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::UdpSocket) {
                        let datagram = || {
                            Type::Result(Box::new(Type::UdpDatagram), Box::new(Type::NetworkError))
                        };
                        let sent =
                            || Type::Result(Box::new(Type::UInt), Box::new(Type::NetworkError));
                        match field.as_str() {
                            "receive_from" | "receive_from_async" if arguments.len() == 1 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "UDP receive limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let result = datagram();
                                return Ok(if field == "receive_from_async" {
                                    Type::Future(Box::new(result))
                                } else {
                                    result
                                });
                            }
                            "receive_from_async_timeout" if arguments.len() == 2 => {
                                let limit = self.check_expression(&arguments[0])?;
                                if !is_integer(&limit) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        "UDP receive limit must be an integer",
                                        arguments[0].span,
                                    ));
                                }
                                let timeout = self.check_expression(&arguments[1])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[1].span,
                                    "UDP receive timeout",
                                )?;
                                return Ok(Type::Future(Box::new(datagram())));
                            }
                            "send_to" | "send_to_async" if arguments.len() == 2 => {
                                self.require_udp_send_arguments(arguments)?;
                                let result = sent();
                                return Ok(if field == "send_to_async" {
                                    Type::Future(Box::new(result))
                                } else {
                                    result
                                });
                            }
                            "send_to_async_timeout" if arguments.len() == 3 => {
                                self.require_udp_send_arguments(&arguments[..2])?;
                                let timeout = self.check_expression(&arguments[2])?;
                                self.require_same(
                                    &Type::Duration,
                                    &timeout,
                                    arguments[2].span,
                                    "UDP send timeout",
                                )?;
                                return Ok(Type::Future(Box::new(sent())));
                            }
                            "local_port" if arguments.is_empty() => return Ok(sent()),
                            "close" if arguments.is_empty() => return Ok(Type::Unit),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::UdpDatagram) {
                        match field.as_str() {
                            "bytes" if arguments.is_empty() => {
                                return Ok(Type::List(Box::new(Type::Unsigned(8))));
                            }
                            "source" if arguments.is_empty() => return Ok(Type::SocketAddress),
                            "len" if arguments.is_empty() => return Ok(Type::UInt),
                            "is_empty" if arguments.is_empty() => return Ok(Type::Bool),
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::IpAddress) {
                        match field.as_str() {
                            "as_string" if arguments.is_empty() => return Ok(Type::String),
                            "is_ipv4" | "is_ipv6" | "is_loopback" | "is_unspecified"
                                if arguments.is_empty() =>
                            {
                                return Ok(Type::Bool);
                            }
                            "as_string" | "is_ipv4" | "is_ipv6" | "is_loopback"
                            | "is_unspecified" => {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    format!("`{field}` expects no arguments"),
                                    expression.span,
                                ));
                            }
                            _ => {}
                        }
                    }
                    if matches!(receiver, Type::Instant)
                        && field == "elapsed"
                        && arguments.is_empty()
                    {
                        return Ok(Type::Duration);
                    }
                    if matches!(receiver, Type::Duration)
                        && arguments.is_empty()
                        && matches!(field.as_str(), "nanos" | "millis" | "seconds")
                    {
                        return Ok(Type::UInt);
                    }
                    if matches!(receiver, Type::String | Type::Str)
                        && matches!(field.as_str(), "len" | "capacity" | "is_empty")
                    {
                        if !arguments.is_empty() {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`{field}` expects no arguments"),
                                expression.span,
                            ));
                        }
                        if matches!(receiver, Type::Str) && field == "capacity" {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "borrowed `str` has no capacity",
                                expression.span,
                            ));
                        }
                        return Ok(if field == "is_empty" {
                            Type::Bool
                        } else {
                            Type::UInt
                        });
                    }
                    if matches!(receiver, Type::String | Type::Str)
                        && matches!(field.as_str(), "contains" | "starts_with" | "ends_with")
                    {
                        if arguments.len() != 1 {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`{field}` expects 1 argument"),
                                expression.span,
                            ));
                        }
                        let actual = self.check_expression(&arguments[0])?;
                        if !matches!(actual, Type::String | Type::Str) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "string query argument must be `String` or `str`",
                                arguments[0].span,
                            ));
                        }
                        return Ok(Type::Bool);
                    }
                    if matches!(receiver, Type::String)
                        && matches!(
                            field.as_str(),
                            "push" | "push_str" | "append" | "add" | "clear"
                        )
                    {
                        let expected = if field == "clear" { 0 } else { 1 };
                        if arguments.len() != expected {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("`{field}` expects {expected} arguments"),
                                expression.span,
                            ));
                        }
                        if let Some(argument) = arguments.first() {
                            let actual = self.check_expression(argument)?;
                            let wanted = if field == "push" {
                                Type::Char
                            } else {
                                Type::String
                            };
                            self.require_same(
                                &wanted,
                                &actual,
                                argument.span,
                                "String method argument",
                            )?;
                        }
                        return Ok(Type::Unit);
                    }
                    let candidates = self
                        .implementations
                        .iter()
                        .filter_map(|implementation| {
                            let trait_info = &self.traits[&implementation.trait_name];
                            let method = trait_info.methods.get(field)?;
                            if !self.implementation_applies(implementation, &receiver) {
                                return None;
                            }
                            let mut substitutions = HashMap::new();
                            infer_substitutions(
                                &implementation.target,
                                &receiver,
                                &mut substitutions,
                                expression.span,
                            )
                            .ok()?;
                            for (generic, argument) in trait_info
                                .generics
                                .iter()
                                .zip(&implementation.trait_arguments)
                            {
                                let argument = substitute(argument, &substitutions);
                                substitutions.insert(generic.clone(), argument);
                            }
                            substitutions.insert("Self".into(), receiver.clone());
                            let method = Signature {
                                asynchronous: method.asynchronous,
                                generics: method.generics.clone(),
                                constraints: method.constraints.clone(),
                                parameters: method
                                    .parameters
                                    .iter()
                                    .map(|ty| {
                                        substitute(
                                            &substitute_associated(
                                                ty,
                                                &implementation.associated_types,
                                            ),
                                            &substitutions,
                                        )
                                    })
                                    .collect(),
                                result: substitute(
                                    &substitute_associated(
                                        &method.result,
                                        &implementation.associated_types,
                                    ),
                                    &substitutions,
                                ),
                                capabilities: method.capabilities.clone(),
                            };
                            Some((implementation.target.clone(), method))
                        })
                        .chain(match &receiver {
                            Type::Generic(name) => self
                                .generic_types
                                .get(name)
                                .into_iter()
                                .flatten()
                                .filter_map(|trait_name| {
                                    self.traits
                                        .get(trait_name)?
                                        .methods
                                        .get(field)
                                        .map(|method| (receiver.clone(), method.clone()))
                                })
                                .collect::<Vec<_>>()
                                .into_iter(),
                            _ => Vec::new().into_iter(),
                        })
                        .collect::<Vec<_>>();
                    if candidates.len() > 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "ambiguous method `{field}` for {}",
                                self.format_type(&receiver)
                            ),
                            callee.span,
                        ));
                    }
                    if let Some((_target, method)) = candidates.first() {
                        let mut substitutions = HashMap::new();
                        substitutions.insert("Self".into(), receiver.clone());
                        if method.parameters.len() != arguments.len() + 1 {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!(
                                    "method `{field}` expects {} arguments, found {}",
                                    method.parameters.len().saturating_sub(1),
                                    arguments.len()
                                ),
                                expression.span,
                            ));
                        }
                        let receiver_parameter = substitute(&method.parameters[0], &substitutions);
                        let expected_receiver = match &receiver_parameter {
                            Type::Reference(inner, _) => &**inner,
                            other => other,
                        };
                        self.require_same(
                            expected_receiver,
                            &receiver,
                            object.span,
                            "method receiver",
                        )?;
                        for (parameter, argument) in method.parameters[1..].iter().zip(arguments) {
                            let actual = self.check_expression(argument)?;
                            infer_substitutions(
                                parameter,
                                &actual,
                                &mut substitutions,
                                argument.span,
                            )?;
                            let expected = substitute(parameter, &substitutions);
                            self.require_same(
                                &expected,
                                &actual,
                                argument.span,
                                "method argument",
                            )?;
                        }
                        for generic in &method.generics {
                            let Some(concrete) = substitutions.get(generic) else {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    format!(
                                        "cannot infer generic argument `{generic}` for method `{field}`"
                                    ),
                                    expression.span,
                                ));
                            };
                            if matches!(concrete, Type::IntLiteral(_) | Type::NegativeIntLiteral(_))
                            {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "integer literal does not fit the default `int` type",
                                    expression.span,
                                )
                                .with_help(
                                    "bind it with an explicit integer type before this call",
                                ));
                            }
                            for constraint in &method.constraints[generic] {
                                if !self.type_satisfies_constraint(concrete, constraint) {
                                    return Err(Diagnostic::new(
                                        DiagnosticKind::Type,
                                        format!(
                                            "type {} does not satisfy constraint `{constraint}` for `{generic}`",
                                            self.format_type(concrete)
                                        ),
                                        expression.span,
                                    ));
                                }
                            }
                        }
                        let result = substitute(&method.result, &substitutions);
                        return Ok(if method.asynchronous {
                            Type::Future(Box::new(result))
                        } else {
                            result
                        });
                    }
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("no method `{field}` for {}", self.format_type(&receiver)),
                        callee.span,
                    ));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "print") {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`print` expects 1 argument, found {}", arguments.len()),
                            expression.span,
                        ));
                    }
                    let printable = self.check_expression(&arguments[0])?;
                    if matches!(printable, Type::SecretBytes | Type::Ed25519SigningKey) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "secret key material cannot be formatted or printed",
                            arguments[0].span,
                        ));
                    }
                    return Ok(Type::Unit);
                }
                if let Expression::Identifier(name) = &callee.node
                    && matches!(name.as_str(), "Some" | "Ok" | "Err")
                {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("`{name}` expects 1 payload, found {}", arguments.len()),
                            expression.span,
                        ));
                    }
                    let payload = self.check_expression(&arguments[0])?;
                    return Ok(match name.as_str() {
                        "Some" => Type::Option(Box::new(payload)),
                        "Ok" => Type::Result(Box::new(payload), Box::new(Type::Infer)),
                        "Err" => Type::Result(Box::new(Type::Infer), Box::new(payload)),
                        _ => unreachable!(),
                    });
                }
                if let Expression::Identifier(name) = &callee.node
                    && let Some(signature) = self.functions.get(name).cloned()
                    && !signature.generics.is_empty()
                {
                    if signature.parameters.len() != arguments.len() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!(
                                "function `{name}` expects {} arguments, found {}",
                                signature.parameters.len(),
                                arguments.len()
                            ),
                            expression.span,
                        ));
                    }
                    let mut substitutions = HashMap::new();
                    for (parameter, argument) in signature.parameters.iter().zip(arguments) {
                        let actual = self.check_expression(argument)?;
                        let inference_actual =
                            implicit_shared_borrow_target(parameter, &actual).unwrap_or(&actual);
                        if implicit_shared_borrow_target(parameter, &actual).is_some()
                            && !is_storage_expression(argument)
                        {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "implicit borrowing requires a named storage value",
                                argument.span,
                            )
                            .with_help("bind the value first or write an explicit borrow"));
                        }
                        let inference_parameter = match parameter {
                            Type::Reference(inner, false)
                                if !matches!(actual, Type::Reference(_, _)) =>
                            {
                                &**inner
                            }
                            _ => parameter,
                        };
                        infer_substitutions(
                            inference_parameter,
                            inference_actual,
                            &mut substitutions,
                            argument.span,
                        )?;
                        let expected = substitute(parameter, &substitutions);
                        let comparison =
                            implicit_shared_borrow_target(&expected, &actual).unwrap_or(&actual);
                        let expected = match &expected {
                            Type::Reference(inner, false)
                                if !matches!(actual, Type::Reference(_, _)) =>
                            {
                                &**inner
                            }
                            _ => &expected,
                        };
                        self.require_same(
                            expected,
                            comparison,
                            argument.span,
                            "function argument",
                        )?;
                    }
                    for generic in &signature.generics {
                        if !substitutions.contains_key(generic) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                format!("cannot infer generic argument `{generic}` for `{name}`"),
                                expression.span,
                            ));
                        }
                        let concrete = &substitutions[generic];
                        if matches!(concrete, Type::IntLiteral(_) | Type::NegativeIntLiteral(_)) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "integer literal does not fit the default `int` type",
                                expression.span,
                            )
                            .with_help(
                                "bind it with an explicit i128 or u128 annotation before this call",
                            ));
                        }
                        for constraint in &signature.constraints[generic] {
                            if !self.type_satisfies_constraint(concrete, constraint) {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    format!(
                                        "type {} does not satisfy constraint `{constraint}` for `{generic}`",
                                        self.format_type(concrete)
                                    ),
                                    expression.span,
                                ));
                            }
                        }
                    }
                    let mut result = substitute(&signature.result, &substitutions);
                    if signature.asynchronous {
                        result = Type::Future(Box::new(result));
                    }
                    self.validate_instantiated_type(&result, expression.span)?;
                    return Ok(result);
                }
                let callee_type = self.check_expression(callee)?;
                let direct_external = matches!(
                    &callee.node,
                    Expression::Identifier(name) if self.external_functions.contains(name)
                );
                let (parameters, result) = match callee_type {
                    Type::Function(parameters, result) => (parameters, result),
                    Type::CFunction(parameters, result) => {
                        if !direct_external {
                            self.require_explicit_unsafe_capability(
                                Capability::Foreign,
                                "C callback invocation",
                                expression.span,
                                "validate the callback provider and invoke it inside `unsafe uses Foreign { ... }`",
                            )?;
                        }
                        (parameters, result)
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "expression is not callable",
                            callee.span,
                        ));
                    }
                };
                if parameters.len() != arguments.len() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "function expects {} arguments, found {}",
                            parameters.len(),
                            arguments.len()
                        ),
                        expression.span,
                    ));
                }
                let mut substitutions = HashMap::new();
                for (parameter, argument) in parameters.iter().zip(arguments) {
                    let argument_type = self.check_expression(argument)?;
                    let inference_parameter = match parameter {
                        Type::Reference(inner, false)
                            if !matches!(argument_type, Type::Reference(_, _)) =>
                        {
                            &**inner
                        }
                        _ => parameter,
                    };
                    let inference_actual = implicit_shared_borrow_target(parameter, &argument_type)
                        .unwrap_or(&argument_type);
                    if implicit_shared_borrow_target(parameter, &argument_type).is_some()
                        && !is_storage_expression(argument)
                    {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "implicit borrowing requires a named storage value",
                            argument.span,
                        ));
                    }
                    infer_substitutions(
                        inference_parameter,
                        inference_actual,
                        &mut substitutions,
                        argument.span,
                    )?;
                    let expected = substitute(parameter, &substitutions);
                    let comparison = implicit_shared_borrow_target(&expected, &argument_type)
                        .unwrap_or(&argument_type);
                    let expected = match &expected {
                        Type::Reference(inner, false)
                            if !matches!(argument_type, Type::Reference(_, _)) =>
                        {
                            &**inner
                        }
                        _ => &expected,
                    };
                    self.require_same(expected, comparison, argument.span, "function argument")?;
                }
                let result = substitute(&result, &substitutions);
                self.validate_instantiated_type(&result, expression.span)?;
                Ok(result)
            }
            Expression::Match { value, arms } => {
                let matched_type = self.check_expression(value)?;
                let mut result_type = None;
                let mut coverage = MatchCoverage::new(&matched_type, self)?;
                let mut alternative_contracts: HashMap<u32, HashMap<String, Type>> = HashMap::new();
                for arm in arms {
                    self.begin_scope();
                    let pattern_result =
                        self.check_pattern(&arm.pattern.node, arm.pattern.span, &matched_type);
                    if pattern_result.is_ok()
                        && let Some(group) = arm.alternative_group
                    {
                        let contract = self
                            .scopes
                            .last()
                            .unwrap()
                            .iter()
                            .map(|(name, variable)| (name.clone(), variable.ty.clone()))
                            .collect::<HashMap<_, _>>();
                        if let Some(expected) = alternative_contracts.get(&group) {
                            if *expected != contract {
                                self.end_scope();
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Type,
                                    "every `|` pattern alternative must bind the same names with the same types",
                                    arm.pattern.span,
                                ));
                            }
                        } else {
                            alternative_contracts.insert(group, contract);
                        }
                    }
                    let arm_result = pattern_result.and_then(|key| {
                        coverage.add(key, arm.pattern.span, arm.guard.is_none())?;
                        if let Some(guard) = &arm.guard {
                            let guard_type = self.check_expression(guard)?;
                            self.require_same(&Type::Bool, &guard_type, guard.span, "match guard")?;
                        }
                        self.check_expression(&arm.value)
                    });
                    self.end_scope();
                    let arm_type = arm_result?;
                    if let Some(expected) = &result_type {
                        let merged = merge_types(expected, &arm_type);
                        self.require_same(&merged, expected, arm.value.span, "match arm")?;
                        self.require_same(&merged, &arm_type, arm.value.span, "match arm")?;
                        result_type = Some(merged);
                    } else {
                        result_type = Some(arm_type);
                    }
                }
                coverage.finish(expression.span)?;
                Ok(result_type.unwrap_or(Type::Unit))
            }
            Expression::Try(operand) => {
                let operand_type = self.check_expression(operand)?;
                match operand_type {
                    Type::Option(value) => {
                        if !matches!(self.expected_return, Type::Option(_)) {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`?` on Option requires the enclosing function to return Option",
                                expression.span,
                            ));
                        }
                        Ok(*value)
                    }
                    Type::Result(value, error) => {
                        let Type::Result(_, expected_error) = &self.expected_return else {
                            return Err(Diagnostic::new(
                                DiagnosticKind::Type,
                                "`?` on Result requires the enclosing function to return Result",
                                expression.span,
                            ));
                        };
                        self.require_same(
                            expected_error,
                            &error,
                            expression.span,
                            "propagated error",
                        )?;
                        Ok(*value)
                    }
                    other => Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("`?` requires Option or Result, found {other:?}"),
                        expression.span,
                    )),
                }
            }
        }
    }

    fn require_unsafe_capability(
        &self,
        capability: Capability,
        operation: &str,
        span: Span,
        help: &str,
    ) -> Result<(), Diagnostic> {
        if self.unsafe_depth == 0 {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("{operation} requires an `unsafe` block"),
                span,
            )
            .with_help(help));
        }

        if self
            .unsafe_contracts
            .iter()
            .filter_map(Option::as_ref)
            .any(|contract| !contract.contains(&capability))
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "unsafe block contract does not allow capability `{}` required by {operation}",
                    capability.name()
                ),
                span,
            )
            .with_help(format!(
                "add `{}` to every enclosing explicit `unsafe uses` contract that authorizes this operation",
                capability.name()
            )));
        }

        Ok(())
    }

    fn require_explicit_unsafe_capability(
        &self,
        capability: Capability,
        operation: &str,
        span: Span,
        help: &str,
    ) -> Result<(), Diagnostic> {
        self.require_unsafe_capability(capability, operation, span, help)?;
        if self
            .unsafe_contracts
            .iter()
            .filter_map(Option::as_ref)
            .any(|contract| contract.contains(&capability))
        {
            Ok(())
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "{operation} requires an explicit `unsafe uses {}` authority contract",
                    capability.name()
                ),
                span,
            )
            .with_help(help))
        }
    }

    fn check_binary(
        &self,
        operator: BinaryOperator,
        mut left: Type,
        mut right: Type,
        span: Span,
    ) -> Result<Type, Diagnostic> {
        match (&left, &right) {
            (
                Type::IntLiteral(_) | Type::NegativeIntLiteral(_),
                Type::IntLiteral(_) | Type::NegativeIntLiteral(_),
            ) => {
                left = Type::Int;
                right = Type::Int;
            }
            (Type::FloatLiteral, Type::FloatLiteral) => {
                left = Type::Float;
                right = Type::Float;
            }
            (Type::IntLiteral(_) | Type::NegativeIntLiteral(_), other)
                if is_integer(other) && types_compatible(other, &left) =>
            {
                left = other.clone()
            }
            (other, Type::IntLiteral(_) | Type::NegativeIntLiteral(_))
                if is_integer(other) && types_compatible(other, &right) =>
            {
                right = other.clone()
            }
            (Type::FloatLiteral, other) if matches!(other, Type::Float | Type::Float32) => {
                left = other.clone()
            }
            (other, Type::FloatLiteral) if matches!(other, Type::Float | Type::Float32) => {
                right = other.clone()
            }
            _ => {}
        }
        if !types_compatible(&left, &right) && types_compatible(&right, &left) {
            left = right.clone();
        }
        self.require_same(&left, &right, span, "binary operands")?;
        match operator {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder
                if is_numeric(&left) =>
            {
                Ok(left)
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual if left == Type::SecretBytes => {
                Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "SecretBytes equality must use `constant_time_equals`",
                    span,
                ))
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual if left == Type::Ed25519SigningKey => {
                Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    "Ed25519SigningKey values cannot be compared",
                    span,
                ))
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => Ok(Type::Bool),
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
                if is_numeric(&left) =>
            {
                Ok(Type::Bool)
            }
            BinaryOperator::And | BinaryOperator::Or if left == Type::Bool => Ok(Type::Bool),
            _ => Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("operator {operator:?} cannot be used with {left:?}"),
                span,
            )),
        }
    }

    fn variant_constructor_type(&self, variant: &VariantInfo) -> Type {
        let result = Type::Enum(
            variant.owner,
            self.enums[&variant.owner]
                .generics
                .iter()
                .cloned()
                .map(Type::Generic)
                .collect(),
        );
        if variant.payload.is_empty() {
            result
        } else {
            Type::Function(variant.payload.clone(), Box::new(result))
        }
    }

    fn check_pattern(
        &mut self,
        pattern: &Pattern,
        span: Span,
        expected: &Type,
    ) -> Result<CoveragePattern, Diagnostic> {
        match pattern {
            Pattern::Wildcard => Ok(CoveragePattern::Wildcard),
            Pattern::Or(_) => Err(Diagnostic::new(
                DiagnosticKind::Internal,
                "unexpanded `|` pattern reached type checking",
                span,
            )),
            Pattern::Binding(name) => {
                self.scopes.last_mut().unwrap().insert(
                    name.clone(),
                    Variable {
                        ty: expected.clone(),
                        constant: false,
                    },
                );
                Ok(CoveragePattern::Wildcard)
            }
            Pattern::Integer(value) => {
                self.require_same(expected, &Type::IntLiteral(*value), span, "integer pattern")?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::Integer(*value),
                    vec![],
                ))
            }
            Pattern::NegativeInteger(magnitude) => {
                let value = if *magnitude <= i128::MAX as u128 {
                    -(*magnitude as i128)
                } else if *magnitude == (1_u128 << 127) {
                    i128::MIN
                } else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "negative integer pattern is outside i128 range",
                        span,
                    ));
                };
                self.require_same(
                    expected,
                    &Type::NegativeIntLiteral(value),
                    span,
                    "negative integer pattern",
                )?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::NegativeInteger(value),
                    vec![],
                ))
            }
            Pattern::String(value) => {
                self.require_same(&Type::String, expected, span, "string pattern")?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::String(value.clone()),
                    vec![],
                ))
            }
            Pattern::Character(value) => {
                self.require_same(&Type::Char, expected, span, "character pattern")?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::Character(*value),
                    vec![],
                ))
            }
            Pattern::Bool(value) => {
                self.require_same(&Type::Bool, expected, span, "boolean pattern")?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::Bool(*value),
                    vec![],
                ))
            }
            Pattern::Struct {
                type_name,
                fields,
                rest,
            } => {
                let Type::Struct(id, arguments) = expected else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("struct pattern cannot match {}", self.format_type(expected)),
                        span,
                    ));
                };
                let info = self.structs[id].clone();
                if info.name != *type_name {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "struct pattern `{type_name}` cannot match {}",
                            self.format_type(expected)
                        ),
                        span,
                    ));
                }
                let mut provided = HashMap::new();
                for field in fields {
                    if !info.fields.contains_key(&field.name) {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("unknown field `{}` in `{type_name}` pattern", field.name),
                            field.name_span,
                        ));
                    }
                    if provided.insert(field.name.clone(), field).is_some() {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("duplicate field `{}` in `{type_name}` pattern", field.name),
                            field.name_span,
                        ));
                    }
                }
                if !rest && provided.len() != info.fields.len() {
                    let mut missing = info
                        .fields
                        .keys()
                        .filter(|name| !provided.contains_key(*name))
                        .cloned()
                        .collect::<Vec<_>>();
                    missing.sort();
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "struct pattern `{type_name}` is missing fields {}",
                            missing.join(", ")
                        ),
                        span,
                    )
                    .with_help("list every field or add `..` to ignore the remainder"));
                }
                let substitutions = info
                    .generics
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect();
                let mut names = info.fields.keys().cloned().collect::<Vec<_>>();
                names.sort();
                let patterns = names
                    .iter()
                    .map(|name| {
                        if let Some(field) = provided.get(name) {
                            self.check_pattern(
                                &field.pattern.node,
                                field.pattern.span,
                                &substitute(&info.fields[name], &substitutions),
                            )
                        } else {
                            Ok(CoveragePattern::Wildcard)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::Struct(type_name.clone()),
                    patterns,
                ))
            }
            Pattern::Variant {
                type_name,
                variant,
                arguments,
            } => {
                let payload = match expected {
                    Type::Option(value) => {
                        if type_name.as_deref().is_some_and(|name| name != "Option") {
                            return Err(self.wrong_variant_owner(
                                type_name.as_deref().unwrap(),
                                expected,
                                span,
                            ));
                        }
                        match variant.as_str() {
                            "Some" => vec![(**value).clone()],
                            "None" => vec![],
                            _ => return Err(self.unknown_variant(variant, expected, span)),
                        }
                    }
                    Type::Result(ok, error) => {
                        if type_name.as_deref().is_some_and(|name| name != "Result") {
                            return Err(self.wrong_variant_owner(
                                type_name.as_deref().unwrap(),
                                expected,
                                span,
                            ));
                        }
                        match variant.as_str() {
                            "Ok" => vec![(**ok).clone()],
                            "Err" => vec![(**error).clone()],
                            _ => return Err(self.unknown_variant(variant, expected, span)),
                        }
                    }
                    Type::Enum(id, arguments) => {
                        let info = &self.enums[id];
                        if type_name.as_deref().is_some_and(|name| name != info.name) {
                            return Err(self.wrong_variant_owner(
                                type_name.as_deref().unwrap(),
                                expected,
                                span,
                            ));
                        }
                        let payload = info
                            .variants
                            .get(variant)
                            .map(|info| info.payload.clone())
                            .ok_or_else(|| self.unknown_variant(variant, expected, span))?;
                        let substitutions = info
                            .generics
                            .iter()
                            .cloned()
                            .zip(arguments.iter().cloned())
                            .collect();
                        payload
                            .iter()
                            .map(|ty| substitute(ty, &substitutions))
                            .collect()
                    }
                    _ => {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            format!("variant pattern cannot match {expected:?}"),
                            span,
                        ));
                    }
                };
                if payload.len() != arguments.len() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "variant `{variant}` expects {} payload patterns, found {}",
                            payload.len(),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let arguments = arguments
                    .iter()
                    .zip(payload)
                    .map(|(argument, ty)| self.check_pattern(&argument.node, argument.span, &ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(CoveragePattern::Constructor(
                    CoverageConstructor::Variant(variant.clone()),
                    arguments,
                ))
            }
        }
    }

    fn wrong_variant_owner(&self, owner: &str, expected: &Type, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("variant from `{owner}` cannot match {expected:?}"),
            span,
        )
    }

    fn unknown_variant(&self, variant: &str, expected: &Type, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("`{variant}` is not a variant of {expected:?}"),
            span,
        )
    }

    fn resolve_type(&self, ty: &TypeName) -> Result<Type, Diagnostic> {
        if ty.qualifier != TypeQualifier::Owned {
            let inner = match ty.qualifier {
                TypeQualifier::SharedReference | TypeQualifier::MutableReference => {
                    let mut owned = ty.clone();
                    owned.qualifier = TypeQualifier::Owned;
                    self.resolve_type(&owned)?
                }
                TypeQualifier::RawConstPointer | TypeQualifier::RawMutPointer => {
                    if ty.name != "ptr" || ty.arguments.len() != 1 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Type,
                            "raw pointer type must be `ptr<T>` or `mut ptr<T>`",
                            ty.span,
                        ));
                    }
                    self.resolve_type(&ty.arguments[0])?
                }
                TypeQualifier::Owned => unreachable!(),
            };
            return Ok(match ty.qualifier {
                TypeQualifier::SharedReference => Type::Reference(Box::new(inner), false),
                TypeQualifier::MutableReference => Type::Reference(Box::new(inner), true),
                TypeQualifier::RawConstPointer => Type::RawPointer(Box::new(inner), false),
                TypeQualifier::RawMutPointer => Type::RawPointer(Box::new(inner), true),
                TypeQualifier::Owned => unreachable!(),
            });
        }
        let resolved = match ty.name.as_str() {
            "fn" if !ty.arguments.is_empty() => {
                let (result, parameters) = ty.arguments.split_last().unwrap();
                Type::Function(
                    parameters
                        .iter()
                        .map(|parameter| self.resolve_type(parameter))
                        .collect::<Result<Vec<_>, _>>()?,
                    Box::new(self.resolve_type(result)?),
                )
            }
            "CFunction" if ty.arguments.len() == 1 => match self.resolve_type(&ty.arguments[0])? {
                Type::Function(parameters, result) => Type::CFunction(parameters, result),
                _ => {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        "CFunction requires one function-signature type",
                        ty.span,
                    )
                    .with_help("write `CFunction<fn(CInt) -> CInt>`"));
                }
            },
            "CRegistration" if ty.arguments.is_empty() => Type::CRegistration,
            "int" if ty.arguments.is_empty() => Type::Int,
            "uint" if ty.arguments.is_empty() => Type::UInt,
            "f64" if ty.arguments.is_empty() => Type::Float,
            "f32" if ty.arguments.is_empty() => Type::Float32,
            "i8" if ty.arguments.is_empty() => Type::Signed(8),
            "i16" if ty.arguments.is_empty() => Type::Signed(16),
            "i32" if ty.arguments.is_empty() => Type::Signed(32),
            "i64" if ty.arguments.is_empty() => Type::Signed(64),
            "i128" if ty.arguments.is_empty() => Type::Signed(128),
            "u8" if ty.arguments.is_empty() => Type::Unsigned(8),
            "u16" if ty.arguments.is_empty() => Type::Unsigned(16),
            "u32" if ty.arguments.is_empty() => Type::Unsigned(32),
            "u64" if ty.arguments.is_empty() => Type::Unsigned(64),
            "u128" if ty.arguments.is_empty() => Type::Unsigned(128),
            "String" if ty.arguments.is_empty() => Type::String,
            "str" if ty.arguments.is_empty() => Type::Str,
            "CString" if ty.arguments.is_empty() => Type::CString,
            "CStr" if ty.arguments.is_empty() => Type::CStr,
            "Memory" if ty.arguments.is_empty() => Type::Memory,
            "SecretBytes" if ty.arguments.is_empty() => Type::SecretBytes,
            "AeadEnvelope" if ty.arguments.is_empty() => Type::AeadEnvelope,
            "Ed25519SigningKey" if ty.arguments.is_empty() => Type::Ed25519SigningKey,
            "MemoryPtr" if ty.arguments.len() == 1 => {
                Type::MemoryPointer(Box::new(self.resolve_type(&ty.arguments[0])?), false)
            }
            "MemoryMutPtr" if ty.arguments.len() == 1 => {
                Type::MemoryPointer(Box::new(self.resolve_type(&ty.arguments[0])?), true)
            }
            "CInt" if ty.arguments.is_empty() => Type::Signed(32),
            "CUInt" if ty.arguments.is_empty() => Type::Unsigned(32),
            "CSize" if ty.arguments.is_empty() => Type::UInt,
            "CSSize" if ty.arguments.is_empty() => Type::Int,
            "CChar" if ty.arguments.is_empty() => Type::Signed(8),
            "CUChar" if ty.arguments.is_empty() => Type::Unsigned(8),
            "CShort" if ty.arguments.is_empty() => Type::Signed(16),
            "CUShort" if ty.arguments.is_empty() => Type::Unsigned(16),
            "CLongLong" if ty.arguments.is_empty() => Type::Signed(64),
            "CULongLong" if ty.arguments.is_empty() => Type::Unsigned(64),
            "CFloat" if ty.arguments.is_empty() => Type::Float32,
            "CDouble" if ty.arguments.is_empty() => Type::Float,
            "Path" if ty.arguments.is_empty() => Type::Path,
            "ProcessOutput" if ty.arguments.is_empty() => Type::ProcessOutput,
            "ProcessCommand" if ty.arguments.is_empty() => Type::ProcessCommand,
            "ChildProcess" if ty.arguments.is_empty() => Type::ChildProcess,
            "Database" if ty.arguments.is_empty() => Type::Database,
            "DataStore" if ty.arguments.is_empty() => Type::DataStore,
            "Url" if ty.arguments.is_empty() => Type::Url,
            "Json" if ty.arguments.is_empty() => Type::Json,
            "IpAddress" if ty.arguments.is_empty() => Type::IpAddress,
            "SocketAddress" if ty.arguments.is_empty() => Type::SocketAddress,
            "TcpStream" if ty.arguments.is_empty() => Type::TcpStream,
            "TlsStream" if ty.arguments.is_empty() => Type::TlsStream,
            "HttpRequest" if ty.arguments.is_empty() => Type::HttpRequest,
            "HttpResponse" if ty.arguments.is_empty() => Type::HttpResponse,
            "TcpListener" if ty.arguments.is_empty() => Type::TcpListener,
            "UdpSocket" if ty.arguments.is_empty() => Type::UdpSocket,
            "UdpDatagram" if ty.arguments.is_empty() => Type::UdpDatagram,
            "Instant" if ty.arguments.is_empty() => Type::Instant,
            "Duration" if ty.arguments.is_empty() => Type::Duration,
            "IoError" if ty.arguments.is_empty() => Type::IoError,
            "NetworkError" if ty.arguments.is_empty() => Type::NetworkError,
            "HttpError" if ty.arguments.is_empty() => Type::HttpError,
            "DataError" if ty.arguments.is_empty() => Type::DataError,
            "CryptoError" if ty.arguments.is_empty() => Type::CryptoError,
            "AtomicInt" if ty.arguments.is_empty() => Type::AtomicInt,
            "[]" if ty.arguments.len() == 1 => {
                Type::Slice(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "List" if ty.arguments.len() == 1 => {
                Type::List(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Map" if ty.arguments.len() == 2 => Type::Map(
                Box::new(self.resolve_type(&ty.arguments[0])?),
                Box::new(self.resolve_type(&ty.arguments[1])?),
            ),
            "Set" if ty.arguments.len() == 1 => {
                Type::Set(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Thread" if ty.arguments.len() == 1 => {
                Type::Thread(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Future" if ty.arguments.len() == 1 => {
                Type::Future(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Task" if ty.arguments.len() == 1 => {
                Type::Task(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Mutex" if ty.arguments.len() == 1 => {
                Type::Mutex(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "MutexGuard" if ty.arguments.len() == 1 => {
                Type::MutexGuard(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Channel" if ty.arguments.len() == 1 => {
                Type::Channel(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            name if name.starts_with("[;") && name.ends_with(']') && ty.arguments.len() == 1 => {
                let length = name[2..name.len() - 1].parse::<usize>().map_err(|_| {
                    Diagnostic::new(DiagnosticKind::Type, "invalid array length", ty.span)
                })?;
                Type::Array(Box::new(self.resolve_type(&ty.arguments[0])?), length)
            }
            "char" if ty.arguments.is_empty() => Type::Char,
            "bool" if ty.arguments.is_empty() => Type::Bool,
            "ConversionError" if ty.arguments.is_empty() => Type::ConversionError,
            "Unit" if ty.arguments.is_empty() => Type::Unit,
            "Option" if ty.arguments.len() == 1 => {
                Type::Option(Box::new(self.resolve_type(&ty.arguments[0])?))
            }
            "Result" if ty.arguments.len() == 2 => Type::Result(
                Box::new(self.resolve_type(&ty.arguments[0])?),
                Box::new(self.resolve_type(&ty.arguments[1])?),
            ),
            name if ty.arguments.is_empty() && name.starts_with("Self.") => {
                let associated = name.trim_start_matches("Self.");
                if associated.is_empty() || !self.generic_types.contains_key("Self") {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("unknown associated type projection `{name}`"),
                        ty.span,
                    ));
                }
                Type::Associated(associated.to_owned())
            }
            name if ty.arguments.is_empty() && self.generic_types.contains_key(name) => {
                Type::Generic(name.into())
            }
            name if self.types.contains_key(name) => {
                let arguments = ty
                    .arguments
                    .iter()
                    .map(|argument| self.resolve_type(argument))
                    .collect::<Result<Vec<_>, _>>()?;
                match self.types[name] {
                    Type::Struct(id, _) => {
                        let expected = self.structs[&id].generics.len();
                        if arguments.len() != expected {
                            return Err(self.generic_arity(
                                name,
                                expected,
                                arguments.len(),
                                ty.span,
                            ));
                        }
                        self.require_constraints(
                            &self.structs[&id].constraints,
                            &arguments,
                            ty.span,
                        )?;
                        Type::Struct(id, arguments)
                    }
                    Type::Enum(id, _) => {
                        let expected = self.enums[&id].generics.len();
                        if arguments.len() != expected {
                            return Err(self.generic_arity(
                                name,
                                expected,
                                arguments.len(),
                                ty.span,
                            ));
                        }
                        self.require_constraints(
                            &self.enums[&id].constraints,
                            &arguments,
                            ty.span,
                        )?;
                        Type::Enum(id, arguments)
                    }
                    _ => unreachable!(),
                }
            }
            name => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Type,
                    format!("unknown type `{name}`"),
                    ty.span,
                ));
            }
        };
        Ok(resolved)
    }

    fn require_constraints(
        &self,
        constraints: &[Vec<String>],
        arguments: &[Type],
        span: Span,
    ) -> Result<(), Diagnostic> {
        for (requirements, argument) in constraints.iter().zip(arguments) {
            for requirement in requirements {
                let satisfied = match argument {
                    Type::Generic(name) => self
                        .generic_types
                        .get(name)
                        .is_some_and(|available| available.contains(requirement)),
                    concrete => self.type_satisfies_constraint(concrete, requirement),
                };
                if !satisfied {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!(
                            "type {} does not satisfy constraint `{requirement}`",
                            self.format_type(argument)
                        ),
                        span,
                    ));
                }
            }
        }
        Ok(())
    }

    fn type_satisfies_constraint(&self, ty: &Type, requirement: &str) -> bool {
        self.type_satisfies_constraint_inner(ty, requirement, &mut HashSet::new())
    }

    fn type_satisfies_constraint_inner(
        &self,
        ty: &Type,
        requirement: &str,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        if let Type::Generic(name) = ty {
            return self
                .generic_types
                .get(name)
                .is_some_and(|available| available.iter().any(|item| item == requirement));
        }
        if requirement == "Copy" && self.type_is_copy_inner(ty, visiting) {
            return true;
        }
        let key = (requirement.to_owned(), format!("{ty:?}"));
        if !visiting.insert(key.clone()) {
            return false;
        }
        let satisfied = self.implementations.iter().any(|implementation| {
            implementation.trait_name == requirement
                && self.implementation_applies_inner(implementation, ty, visiting)
        });
        visiting.remove(&key);
        satisfied
    }

    fn implementation_applies(&self, implementation: &ImplInfo, concrete: &Type) -> bool {
        self.implementation_applies_inner(implementation, concrete, &mut HashSet::new())
    }

    fn implementation_applies_inner(
        &self,
        implementation: &ImplInfo,
        concrete: &Type,
        visiting: &mut HashSet<(String, String)>,
    ) -> bool {
        let mut substitutions = HashMap::new();
        if infer_substitutions(
            &implementation.target,
            concrete,
            &mut substitutions,
            Span::point(1, 1),
        )
        .is_err()
            || !types_compatible(
                &substitute(&implementation.target, &substitutions),
                concrete,
            )
        {
            return false;
        }
        implementation
            .constraints
            .iter()
            .all(|(generic, constraints)| {
                substitutions.get(generic).is_some_and(|argument| {
                    constraints.iter().all(|constraint| {
                        self.type_satisfies_constraint_inner(argument, constraint, visiting)
                    })
                })
            })
    }

    fn validate_instantiated_type(&self, ty: &Type, span: Span) -> Result<(), Diagnostic> {
        match ty {
            Type::Struct(id, arguments) => {
                self.require_constraints(&self.structs[id].constraints, arguments, span)
            }
            Type::Enum(id, arguments) => {
                self.require_constraints(&self.enums[id].constraints, arguments, span)
            }
            Type::Option(value) => self.validate_instantiated_type(value, span),
            Type::Thread(value)
            | Type::Mutex(value)
            | Type::MutexGuard(value)
            | Type::Channel(value) => self.validate_instantiated_type(value, span),
            Type::Result(ok, error) => {
                self.validate_instantiated_type(ok, span)?;
                self.validate_instantiated_type(error, span)
            }
            _ => Ok(()),
        }
    }

    fn type_is_copy(&self, ty: &Type) -> bool {
        self.type_is_copy_inner(ty, &mut HashSet::new())
    }

    fn type_is_copy_inner(&self, ty: &Type, visiting: &mut HashSet<(String, String)>) -> bool {
        match ty {
            Type::Int
            | Type::IntLiteral(_)
            | Type::NegativeIntLiteral(_)
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
            | Type::Float
            | Type::Float32
            | Type::FloatLiteral
            | Type::Char
            | Type::Bool
            | Type::Instant
            | Type::Duration
            | Type::IpAddress
            | Type::CStr
            | Type::Unit
            | Type::Reference(_, false)
            | Type::RawPointer(_, _)
            | Type::MemoryPointer(_, _)
            | Type::CFunction(_, _) => true,
            Type::Option(value) => self.type_is_copy_inner(value, visiting),
            Type::Result(ok, error) => {
                self.type_is_copy_inner(ok, visiting) && self.type_is_copy_inner(error, visiting)
            }
            Type::Struct(_, _) | Type::Enum(_, _) => {
                let key = ("Copy".to_owned(), format!("{ty:?}"));
                if !visiting.insert(key.clone()) {
                    return false;
                }
                let copy = self.implementations.iter().any(|implementation| {
                    implementation.trait_name == "Copy"
                        && self.implementation_applies_inner(implementation, ty, visiting)
                });
                visiting.remove(&key);
                copy
            }
            _ => false,
        }
    }

    fn type_is_send(&self, ty: &Type, visiting: &mut HashSet<TypeId>) -> bool {
        match ty {
            Type::Reference(_, _)
            | Type::RawPointer(_, _)
            | Type::MemoryPointer(_, _)
            | Type::MutexGuard(_)
            | Type::Slice(_)
            | Type::Str
            | Type::CStr
            | Type::SecretBytes
            | Type::AeadEnvelope
            | Type::Ed25519SigningKey
            | Type::Function(_, _)
            | Type::CFunction(_, _)
            | Type::CRegistration => false,
            Type::Generic(_) | Type::Infer => false,
            Type::Array(element, _)
            | Type::List(element)
            | Type::Set(element)
            | Type::Option(element)
            | Type::Thread(element)
            | Type::Mutex(element)
            | Type::Channel(element) => self.type_is_send(element, visiting),
            Type::Map(key, value) | Type::Result(key, value) => {
                self.type_is_send(key, visiting) && self.type_is_send(value, visiting)
            }
            Type::Struct(id, arguments) => {
                if !visiting.insert(*id) {
                    return true;
                }
                let info = &self.structs[id];
                let substitutions = info
                    .generics
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect();
                let send = info
                    .fields
                    .values()
                    .all(|field| self.type_is_send(&substitute(field, &substitutions), visiting));
                visiting.remove(id);
                send
            }
            Type::Enum(id, arguments) => {
                if !visiting.insert(*id) {
                    return true;
                }
                let info = &self.enums[id];
                let substitutions = info
                    .generics
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned())
                    .collect();
                let send = info.variants.values().all(|variant| {
                    variant.payload.iter().all(|payload| {
                        self.type_is_send(&substitute(payload, &substitutions), visiting)
                    })
                });
                visiting.remove(id);
                send
            }
            _ => true,
        }
    }

    fn ensure_json_codec_type(
        &self,
        ty: &Type,
        span: Span,
        decoding: bool,
    ) -> Result<(), Diagnostic> {
        fn visit(
            checker: &TypeChecker,
            ty: &Type,
            decoding: bool,
            visiting: &mut HashSet<TypeId>,
        ) -> bool {
            fn may_encode_as_json_null(ty: &Type) -> bool {
                matches!(ty, Type::Json | Type::Unit | Type::Option(_))
            }
            match ty {
                Type::Int
                | Type::IntLiteral(_)
                | Type::NegativeIntLiteral(_)
                | Type::UInt
                | Type::Signed(_)
                | Type::Unsigned(_)
                | Type::Float
                | Type::Float32
                | Type::FloatLiteral
                | Type::String
                | Type::Json
                | Type::Char
                | Type::Bool
                | Type::Unit => true,
                Type::Str => !decoding,
                Type::Array(element, _) | Type::List(element) => {
                    visit(checker, element, decoding, visiting)
                }
                Type::Option(element) => {
                    !may_encode_as_json_null(element) && visit(checker, element, decoding, visiting)
                }
                Type::Map(key, value) => {
                    matches!(key.as_ref(), Type::String)
                        && visit(checker, value, decoding, visiting)
                }
                Type::Result(ok, error) => {
                    visit(checker, ok, decoding, visiting)
                        && visit(checker, error, decoding, visiting)
                }
                Type::Struct(id, arguments) => {
                    if !visiting.insert(*id) {
                        return true;
                    }
                    let info = &checker.structs[id];
                    let substitutions = info
                        .generics
                        .iter()
                        .cloned()
                        .zip(arguments.iter().cloned())
                        .collect();
                    let supported = info.fields.values().all(|field| {
                        visit(
                            checker,
                            &substitute(field, &substitutions),
                            decoding,
                            visiting,
                        )
                    });
                    visiting.remove(id);
                    supported
                }
                Type::Enum(id, arguments) => {
                    if !visiting.insert(*id) {
                        return true;
                    }
                    let info = &checker.enums[id];
                    let substitutions = info
                        .generics
                        .iter()
                        .cloned()
                        .zip(arguments.iter().cloned())
                        .collect();
                    let supported = info.variants.values().all(|variant| {
                        variant.payload.iter().all(|payload| {
                            visit(
                                checker,
                                &substitute(payload, &substitutions),
                                decoding,
                                visiting,
                            )
                        })
                    });
                    visiting.remove(id);
                    supported
                }
                _ => false,
            }
        }

        if visit(self, ty, decoding, &mut HashSet::new()) {
            Ok(())
        } else {
            let operation = if decoding { "decode from" } else { "encode as" };
            Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "{} cannot be used with automatic JSON conversion",
                    self.format_type(ty)
                ),
                span,
            )
            .with_help(format!(
                "automatic JSON conversion cannot {operation} borrowed views, handles, pointers, synchronization values, or unsupported map keys"
            )))
        }
    }

    fn ensure_data_field_type(&self, ty: &Type, span: Span) -> Result<(), Diagnostic> {
        let supported = match ty {
            Type::Int
            | Type::Signed(8 | 16 | 32 | 64)
            | Type::Float
            | Type::Float32
            | Type::String
            | Type::Char
            | Type::Bool => true,
            Type::Unsigned(8 | 16 | 32) => true,
            Type::Option(inner) => matches!(
                inner.as_ref(),
                Type::Int
                    | Type::Signed(8 | 16 | 32 | 64)
                    | Type::Unsigned(8 | 16 | 32)
                    | Type::Float
                    | Type::Float32
                    | Type::String
                    | Type::Char
                    | Type::Bool
            ),
            _ => false,
        };
        if supported {
            Ok(())
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "{} cannot be stored directly in a data schema",
                    self.format_type(ty)
                ),
                span,
            )
            .with_help("data fields currently support signed integers through i64, u8/u16/u32, finite floats, bool, char, String, and Option of those types"))
        }
    }

    fn require_data_schema(&self, name: &str, span: Span) -> Result<TypeId, Diagnostic> {
        let Some(Type::Struct(id, arguments)) = self.types.get(name) else {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("unknown data schema `{name}`"),
                span,
            ));
        };
        if !arguments.is_empty() || !self.structs[id].data {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!("`{name}` is not a data schema"),
                span,
            )
            .with_help("declare persistent records with `data Name { ... }`"));
        }
        Ok(*id)
    }

    fn generic_arity(&self, name: &str, expected: usize, actual: usize, span: Span) -> Diagnostic {
        Diagnostic::new(
            DiagnosticKind::Type,
            format!("type `{name}` expects {expected} type arguments, found {actual}"),
            span,
        )
    }

    fn require_same(
        &self,
        expected: &Type,
        actual: &Type,
        span: Span,
        context: &str,
    ) -> Result<(), Diagnostic> {
        if types_compatible(expected, actual) {
            return Ok(());
        }
        Err(Diagnostic::new(
            DiagnosticKind::Type,
            format!(
                "{context} expected {}, found {}",
                self.format_type(expected),
                self.format_type(actual)
            ),
            span,
        ))
    }

    fn require_path(&mut self, expression: &Expr) -> Result<(), Diagnostic> {
        let actual = self.check_expression(expression)?;
        if matches!(actual, Type::Path) {
            Ok(())
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Type,
                format!(
                    "filesystem path must be Path, found {}",
                    self.format_type(&actual)
                ),
                expression.span,
            ))
        }
    }

    fn require_byte_list(&mut self, expression: &Expr, message: &str) -> Result<(), Diagnostic> {
        let actual = self.check_expression(expression)?;
        if matches!(actual, Type::List(ref element) if matches!(**element, Type::Unsigned(8))) {
            Ok(())
        } else {
            Err(Diagnostic::new(
                DiagnosticKind::Type,
                message,
                expression.span,
            ))
        }
    }

    fn require_udp_send_arguments(&mut self, arguments: &[Expr]) -> Result<(), Diagnostic> {
        let bytes = self.check_expression(&arguments[0])?;
        if !matches!(bytes, Type::List(ref element) | Type::Slice(ref element) if matches!(**element, Type::Unsigned(8)))
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "UDP send expects List<u8> or a u8 slice",
                arguments[0].span,
            ));
        }
        let address = self.check_expression(&arguments[1])?;
        self.require_same(
            &Type::SocketAddress,
            &address,
            arguments[1].span,
            "UDP destination address",
        )
    }

    fn format_type(&self, ty: &Type) -> String {
        match ty {
            Type::Int => "Int".into(),
            Type::IntLiteral(_) => "Int".into(),
            Type::NegativeIntLiteral(_) => "Int".into(),
            Type::UInt => "uint".into(),
            Type::Signed(width) => format!("i{width}"),
            Type::Unsigned(width) => format!("u{width}"),
            Type::Float => "Float".into(),
            Type::Float32 => "f32".into(),
            Type::FloatLiteral => "floating-point literal".into(),
            Type::String => "String".into(),
            Type::CString => "CString".into(),
            Type::CStr => "CStr".into(),
            Type::Memory => "Memory".into(),
            Type::SecretBytes => "SecretBytes".into(),
            Type::AeadEnvelope => "AeadEnvelope".into(),
            Type::Ed25519SigningKey => "Ed25519SigningKey".into(),
            Type::Path => "Path".into(),
            Type::ProcessOutput => "ProcessOutput".into(),
            Type::ProcessCommand => "ProcessCommand".into(),
            Type::ChildProcess => "ChildProcess".into(),
            Type::Database => "Database".into(),
            Type::DataStore => "DataStore".into(),
            Type::Url => "Url".into(),
            Type::Json => "Json".into(),
            Type::IpAddress => "IpAddress".into(),
            Type::SocketAddress => "SocketAddress".into(),
            Type::TcpStream => "TcpStream".into(),
            Type::TlsStream => "TlsStream".into(),
            Type::HttpRequest => "HttpRequest".into(),
            Type::HttpResponse => "HttpResponse".into(),
            Type::TcpListener => "TcpListener".into(),
            Type::UdpSocket => "UdpSocket".into(),
            Type::UdpDatagram => "UdpDatagram".into(),
            Type::Instant => "Instant".into(),
            Type::Duration => "Duration".into(),
            Type::Str => "str".into(),
            Type::Array(element, length) => format!("[{}; {length}]", self.format_type(element)),
            Type::Slice(element) => format!("[{}]", self.format_type(element)),
            Type::List(element) => format!("List<{}>", self.format_type(element)),
            Type::Map(key, value) => format!(
                "Map<{}, {}>",
                self.format_type(key),
                self.format_type(value)
            ),
            Type::Set(element) => format!("Set<{}>", self.format_type(element)),
            Type::Thread(result) => format!("Thread<{}>", self.format_type(result)),
            Type::Future(result) => format!("Future<{}>", self.format_type(result)),
            Type::Task(result) => format!("Task<{}>", self.format_type(result)),
            Type::Mutex(value) => format!("Mutex<{}>", self.format_type(value)),
            Type::MutexGuard(value) => format!("MutexGuard<{}>", self.format_type(value)),
            Type::Channel(value) => format!("Channel<{}>", self.format_type(value)),
            Type::AtomicInt => "AtomicInt".into(),
            Type::Char => "Char".into(),
            Type::Bool => "Bool".into(),
            Type::ConversionError => "ConversionError".into(),
            Type::IoError => "IoError".into(),
            Type::NetworkError => "NetworkError".into(),
            Type::HttpError => "HttpError".into(),
            Type::DataError => "DataError".into(),
            Type::CryptoError => "CryptoError".into(),
            Type::Unit => "Unit".into(),
            Type::Struct(id, arguments) => self.format_nominal(&self.structs[id].name, arguments),
            Type::Enum(id, arguments) => self.format_nominal(&self.enums[id].name, arguments),
            Type::Generic(name) => name.clone(),
            Type::Associated(name) => format!("Self.{name}"),
            Type::Reference(inner, mutable) => format!(
                "&{}{}",
                if *mutable { "mut " } else { "" },
                self.format_type(inner)
            ),
            Type::RawPointer(inner, mutable) => format!(
                "{}ptr<{}>",
                if *mutable { "mut " } else { "" },
                self.format_type(inner)
            ),
            Type::MemoryPointer(inner, mutable) => format!(
                "{}<{}>",
                if *mutable {
                    "MemoryMutPtr"
                } else {
                    "MemoryPtr"
                },
                self.format_type(inner)
            ),
            Type::Option(value) => format!("Option<{}>", self.format_type(value)),
            Type::Result(ok, error) => format!(
                "Result<{}, {}>",
                self.format_type(ok),
                self.format_type(error)
            ),
            Type::Function(parameters, result) => format!(
                "fn({}) -> {}",
                parameters
                    .iter()
                    .map(|parameter| self.format_type(parameter))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.format_type(result)
            ),
            Type::CFunction(parameters, result) => format!(
                "CFunction<fn({}) -> {}>",
                parameters
                    .iter()
                    .map(|parameter| self.format_type(parameter))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.format_type(result)
            ),
            Type::CRegistration => "CRegistration".into(),
            Type::Infer => "_".into(),
        }
    }

    fn format_nominal(&self, name: &str, arguments: &[Type]) -> String {
        if arguments.is_empty() {
            name.into()
        } else {
            format!(
                "{name}<{}>",
                arguments
                    .iter()
                    .map(|argument| self.format_type(argument))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn is_constant_expression(&self, expression: &Expr) -> bool {
        match &expression.node {
            Expression::Array(values) => values
                .iter()
                .all(|value| self.is_constant_expression(value)),
            Expression::Index { .. } => false,
            Expression::Subslice { .. } => false,
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => true,
            Expression::Identifier(name) => self
                .lookup_variable(name)
                .is_some_and(|variable| variable.constant),
            Expression::Unary { operand, .. } => self.is_constant_expression(operand),
            Expression::Binary { left, right, .. } => {
                self.is_constant_expression(left) && self.is_constant_expression(right)
            }
            Expression::StructConstruct { fields, .. } => fields
                .iter()
                .all(|field| self.is_constant_expression(&field.value)),
            Expression::FieldAccess { object, .. } => self.is_constant_expression(object),
            Expression::Match { value, arms } => {
                self.is_constant_expression(value)
                    && arms.iter().all(|arm| {
                        arm.guard
                            .as_ref()
                            .is_none_or(|guard| self.is_constant_expression(guard))
                            && self.is_constant_expression(&arm.value)
                    })
            }
            Expression::Try(_)
            | Expression::Await(_)
            | Expression::Spawn(_)
            | Expression::Call { .. }
            | Expression::Closure { .. }
            | Expression::Move(_)
            | Expression::Borrow { .. }
            | Expression::Dereference(_)
            | Expression::DataWrite { .. }
            | Expression::DataStore { .. }
            | Expression::DataQuery { .. }
            | Expression::DataRemove { .. } => false,
        }
    }

    fn lookup_variable(&self, name: &str) -> Option<Variable> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn begin_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CoverageConstructor {
    Variant(String),
    Struct(String),
    Bool(bool),
    Integer(u128),
    NegativeInteger(i128),
    String(String),
    Character(char),
}

#[derive(Debug, Clone)]
enum CoveragePattern {
    Wildcard,
    Constructor(CoverageConstructor, Vec<CoveragePattern>),
}

#[derive(Debug, Clone)]
enum CoverageType {
    Open,
    Finite(Vec<(CoverageConstructor, Vec<CoverageType>)>),
}

struct MatchCoverage {
    ty: CoverageType,
    rows: Vec<Vec<CoveragePattern>>,
}

impl MatchCoverage {
    fn new(expected: &Type, checker: &TypeChecker) -> Result<Self, Diagnostic> {
        Ok(Self {
            ty: coverage_type(expected, checker, &mut HashSet::new(), 0),
            rows: vec![],
        })
    }

    fn add(
        &mut self,
        pattern: CoveragePattern,
        span: Span,
        contributes: bool,
    ) -> Result<(), Diagnostic> {
        if !pattern_is_useful(
            &self.rows,
            std::slice::from_ref(&pattern),
            std::slice::from_ref(&self.ty),
            0,
        ) {
            return Err(Diagnostic::new(
                DiagnosticKind::Type,
                "unreachable match arm; its pattern is already covered",
                span,
            ));
        }
        if contributes {
            self.rows.push(vec![pattern]);
        }
        Ok(())
    }

    fn finish(&self, span: Span) -> Result<(), Diagnostic> {
        if !pattern_is_useful(
            &self.rows,
            &[CoveragePattern::Wildcard],
            std::slice::from_ref(&self.ty),
            0,
        ) {
            return Ok(());
        }
        let detail = match &self.ty {
            CoverageType::Finite(constructors) => {
                let mut missing = constructors
                    .iter()
                    .filter(|(constructor, fields)| {
                        pattern_is_useful(
                            &self.rows,
                            &[CoveragePattern::Constructor(
                                constructor.clone(),
                                vec![CoveragePattern::Wildcard; fields.len()],
                            )],
                            std::slice::from_ref(&self.ty),
                            0,
                        )
                    })
                    .map(|(constructor, _)| coverage_constructor_name(constructor))
                    .collect::<Vec<_>>();
                missing.sort();
                if missing.is_empty() {
                    String::new()
                } else {
                    format!("; missing coverage for {}", missing.join(", "))
                }
            }
            CoverageType::Open => " over an open value domain".into(),
        };
        Err(Diagnostic::new(
            DiagnosticKind::Type,
            format!("non-exhaustive match{detail}"),
            span,
        )
        .with_help("add patterns for the remaining values or a `_` catch-all arm"))
    }
}

fn coverage_type(
    ty: &Type,
    checker: &TypeChecker,
    visiting: &mut HashSet<String>,
    depth: usize,
) -> CoverageType {
    if depth >= 64 {
        return CoverageType::Open;
    }
    match ty {
        Type::Bool => CoverageType::Finite(vec![
            (CoverageConstructor::Bool(false), vec![]),
            (CoverageConstructor::Bool(true), vec![]),
        ]),
        Type::Option(value) => CoverageType::Finite(vec![
            (
                CoverageConstructor::Variant("Some".into()),
                vec![coverage_type(value, checker, visiting, depth + 1)],
            ),
            (CoverageConstructor::Variant("None".into()), vec![]),
        ]),
        Type::Result(ok, error) => CoverageType::Finite(vec![
            (
                CoverageConstructor::Variant("Ok".into()),
                vec![coverage_type(ok, checker, visiting, depth + 1)],
            ),
            (
                CoverageConstructor::Variant("Err".into()),
                vec![coverage_type(error, checker, visiting, depth + 1)],
            ),
        ]),
        Type::Struct(id, arguments) => {
            let key = format!("struct:{id:?}:{arguments:?}");
            if !visiting.insert(key.clone()) {
                return CoverageType::Open;
            }
            let info = &checker.structs[id];
            let substitutions = info
                .generics
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            let mut fields = info.fields.iter().collect::<Vec<_>>();
            fields.sort_by_key(|(name, _)| *name);
            let fields = fields
                .into_iter()
                .map(|(_, field)| {
                    coverage_type(
                        &substitute(field, &substitutions),
                        checker,
                        visiting,
                        depth + 1,
                    )
                })
                .collect();
            visiting.remove(&key);
            CoverageType::Finite(vec![(
                CoverageConstructor::Struct(info.name.clone()),
                fields,
            )])
        }
        Type::Enum(id, arguments) => {
            let key = format!("{id:?}:{arguments:?}");
            if !visiting.insert(key.clone()) {
                return CoverageType::Open;
            }
            let info = &checker.enums[id];
            let substitutions = info
                .generics
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect();
            let mut variants = info
                .variants
                .iter()
                .map(|(name, variant)| {
                    (
                        CoverageConstructor::Variant(name.clone()),
                        variant
                            .payload
                            .iter()
                            .map(|payload| {
                                coverage_type(
                                    &substitute(payload, &substitutions),
                                    checker,
                                    visiting,
                                    depth + 1,
                                )
                            })
                            .collect(),
                    )
                })
                .collect::<Vec<_>>();
            variants.sort_by(|(left, _), (right, _)| {
                coverage_constructor_name(left).cmp(&coverage_constructor_name(right))
            });
            visiting.remove(&key);
            CoverageType::Finite(variants)
        }
        _ => CoverageType::Open,
    }
}

fn coverage_constructor_name(constructor: &CoverageConstructor) -> String {
    match constructor {
        CoverageConstructor::Variant(name) => name.clone(),
        CoverageConstructor::Struct(name) => name.clone(),
        CoverageConstructor::Bool(value) => value.to_string(),
        CoverageConstructor::Integer(value) => value.to_string(),
        CoverageConstructor::NegativeInteger(value) => value.to_string(),
        CoverageConstructor::String(value) => format!("{value:?}"),
        CoverageConstructor::Character(value) => format!("{value:?}"),
    }
}

fn pattern_is_useful(
    matrix: &[Vec<CoveragePattern>],
    candidate: &[CoveragePattern],
    types: &[CoverageType],
    depth: usize,
) -> bool {
    if depth >= 128 {
        return true;
    }
    let Some(first) = candidate.first() else {
        return matrix.is_empty();
    };
    let ty = &types[0];
    match first {
        CoveragePattern::Constructor(constructor, arguments) => {
            let fields = match ty {
                CoverageType::Finite(constructors) => constructors
                    .iter()
                    .find(|(candidate, _)| candidate == constructor)
                    .map(|(_, fields)| fields.clone())
                    .unwrap_or_default(),
                CoverageType::Open => vec![],
            };
            let specialized = specialize_matrix(matrix, constructor, fields.len());
            let mut vector = arguments.clone();
            vector.extend_from_slice(&candidate[1..]);
            let mut specialized_types = fields;
            specialized_types.extend_from_slice(&types[1..]);
            pattern_is_useful(&specialized, &vector, &specialized_types, depth + 1)
        }
        CoveragePattern::Wildcard => match ty {
            CoverageType::Finite(constructors) => {
                constructors.iter().any(|(constructor, fields)| {
                    let specialized = specialize_matrix(matrix, constructor, fields.len());
                    let mut vector = vec![CoveragePattern::Wildcard; fields.len()];
                    vector.extend_from_slice(&candidate[1..]);
                    let mut specialized_types = fields.clone();
                    specialized_types.extend_from_slice(&types[1..]);
                    pattern_is_useful(&specialized, &vector, &specialized_types, depth + 1)
                })
            }
            CoverageType::Open => {
                let default = matrix
                    .iter()
                    .filter_map(|row| match row.first() {
                        Some(CoveragePattern::Wildcard) => Some(row[1..].to_vec()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                pattern_is_useful(&default, &candidate[1..], &types[1..], depth + 1)
            }
        },
    }
}

fn specialize_matrix(
    matrix: &[Vec<CoveragePattern>],
    constructor: &CoverageConstructor,
    arity: usize,
) -> Vec<Vec<CoveragePattern>> {
    matrix
        .iter()
        .filter_map(|row| match row.first()? {
            CoveragePattern::Wildcard => {
                let mut specialized = vec![CoveragePattern::Wildcard; arity];
                specialized.extend_from_slice(&row[1..]);
                Some(specialized)
            }
            CoveragePattern::Constructor(candidate, arguments) if candidate == constructor => {
                let mut specialized = arguments.clone();
                specialized.extend_from_slice(&row[1..]);
                Some(specialized)
            }
            CoveragePattern::Constructor(_, _) => None,
        })
        .collect()
}

fn types_compatible(expected: &Type, actual: &Type) -> bool {
    match (expected, actual) {
        (Type::Infer, _) | (_, Type::Infer) => true,
        (Type::Int, Type::IntLiteral(value)) => *value <= i64::MAX as u128,
        (Type::Int, Type::NegativeIntLiteral(value)) => *value >= i64::MIN as i128,
        (Type::UInt, Type::IntLiteral(value)) => *value <= u64::MAX as u128,
        (Type::Signed(width), Type::IntLiteral(value)) => {
            *value <= i128::MAX as u128 && signed_fits(*value as i128, *width)
        }
        (Type::Signed(width), Type::NegativeIntLiteral(value)) => signed_fits(*value, *width),
        (Type::Unsigned(width), Type::IntLiteral(value)) => unsigned_fits(*value, *width),
        (Type::Float | Type::Float32, Type::FloatLiteral) => true,
        (Type::Signed(expected), Type::Signed(actual)) => expected >= actual,
        (Type::Unsigned(expected), Type::Unsigned(actual)) => expected >= actual,
        (Type::Int, Type::Signed(actual)) => *actual <= 64,
        (Type::Signed(expected), Type::Int) => *expected >= 128,
        (Type::UInt, Type::Unsigned(actual)) => *actual <= 64,
        (Type::Unsigned(expected), Type::UInt) => *expected >= 128,
        (Type::Signed(expected), Type::Unsigned(actual)) => *expected > *actual,
        (Type::Float, Type::Float32) => true,
        (Type::Generic(left), Type::Generic(right)) => left == right,
        (Type::Reference(left, lm), Type::Reference(right, rm)) => {
            (!*lm || *rm) && types_compatible(left, right)
        }
        (Type::RawPointer(left, lm), Type::RawPointer(right, rm)) => {
            lm == rm && types_compatible(left, right)
        }
        (Type::MemoryPointer(left, lm), Type::MemoryPointer(right, rm)) => {
            lm == rm && types_compatible(left, right)
        }
        (Type::Str, Type::String) => true,
        (Type::Struct(left, a), Type::Struct(right, b))
        | (Type::Enum(left, a), Type::Enum(right, b)) => {
            left == right
                && a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| types_compatible(x, y))
        }
        (Type::Option(left), Type::Option(right)) => types_compatible(left, right),
        (Type::Array(left, left_len), Type::Array(right, right_len)) => {
            left_len == right_len && types_compatible(left, right)
        }
        (Type::Slice(left), Type::Slice(right)) => types_compatible(left, right),
        (Type::List(left), Type::List(right)) => types_compatible(left, right),
        (Type::Channel(left), Type::Channel(right)) => types_compatible(left, right),
        (Type::Map(lk, lv), Type::Map(rk, rv)) => {
            types_compatible(lk, rk) && types_compatible(lv, rv)
        }
        (Type::Set(left), Type::Set(right)) => types_compatible(left, right),
        (Type::Future(left), Type::Future(right)) | (Type::Task(left), Type::Task(right)) => {
            types_compatible(left, right)
        }
        (Type::Result(left_ok, left_error), Type::Result(right_ok, right_error)) => {
            types_compatible(left_ok, right_ok) && types_compatible(left_error, right_error)
        }
        (
            Type::Function(left_parameters, left_result),
            Type::Function(right_parameters, right_result),
        )
        | (
            Type::CFunction(left_parameters, left_result),
            Type::CFunction(right_parameters, right_result),
        ) => {
            left_parameters.len() == right_parameters.len()
                && left_parameters
                    .iter()
                    .zip(right_parameters)
                    .all(|(left, right)| types_compatible(left, right))
                && types_compatible(left_result, right_result)
        }
        _ => expected == actual,
    }
}

fn signed_fits(value: i128, width: u16) -> bool {
    width == 128 || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1).contains(&value)
}

fn unsigned_fits(value: u128, width: u16) -> bool {
    width == 128 || value < (1_u128 << width)
}

fn materialize_literal(ty: Type) -> Type {
    match ty {
        Type::IntLiteral(value) if value <= i64::MAX as u128 => Type::Int,
        Type::NegativeIntLiteral(value) if value >= i64::MIN as i128 => Type::Int,
        Type::FloatLiteral => Type::Float,
        other => other,
    }
}

fn infer_substitutions(
    template: &Type,
    actual: &Type,
    substitutions: &mut HashMap<String, Type>,
    span: Span,
) -> Result<(), Diagnostic> {
    match (template, actual) {
        (Type::Generic(name), actual) => {
            if let Some(previous) = substitutions.get(name) {
                if !types_compatible(previous, actual) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Type,
                        format!("conflicting inferred types for `{name}`"),
                        span,
                    ));
                }
            } else {
                substitutions.insert(name.clone(), materialize_literal(actual.clone()));
            }
        }
        (Type::Struct(a, xs), Type::Struct(b, ys)) | (Type::Enum(a, xs), Type::Enum(b, ys))
            if a == b && xs.len() == ys.len() =>
        {
            for (x, y) in xs.iter().zip(ys) {
                infer_substitutions(x, y, substitutions, span)?;
            }
        }
        (Type::Option(x), Type::Option(y)) => infer_substitutions(x, y, substitutions, span)?,
        (Type::Array(x, a), Type::Array(y, b)) if a == b => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::Slice(x), Type::Slice(y)) => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::List(x), Type::List(y)) => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::Thread(x), Type::Thread(y))
        | (Type::Future(x), Type::Future(y))
        | (Type::Task(x), Type::Task(y))
        | (Type::Mutex(x), Type::Mutex(y))
        | (Type::MutexGuard(x), Type::MutexGuard(y))
        | (Type::Channel(x), Type::Channel(y)) => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::Map(ak, av), Type::Map(bk, bv)) => {
            infer_substitutions(ak, bk, substitutions, span)?;
            infer_substitutions(av, bv, substitutions, span)?;
        }
        (Type::Set(x), Type::Set(y)) => infer_substitutions(x, y, substitutions, span)?,
        (Type::Result(a, b), Type::Result(x, y)) => {
            infer_substitutions(a, x, substitutions, span)?;
            infer_substitutions(b, y, substitutions, span)?;
        }
        (Type::Function(parameters, result), Type::Function(actual_parameters, actual_result))
        | (
            Type::CFunction(parameters, result),
            Type::CFunction(actual_parameters, actual_result),
        ) if parameters.len() == actual_parameters.len() => {
            for (parameter, actual) in parameters.iter().zip(actual_parameters) {
                infer_substitutions(parameter, actual, substitutions, span)?;
            }
            infer_substitutions(result, actual_result, substitutions, span)?;
        }
        (Type::Reference(x, xm), Type::Reference(y, ym)) if !*xm || *ym => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::RawPointer(x, xm), Type::RawPointer(y, ym)) if xm == ym => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        (Type::MemoryPointer(x, xm), Type::MemoryPointer(y, ym)) if xm == ym => {
            infer_substitutions(x, y, substitutions, span)?;
        }
        _ => {}
    }
    Ok(())
}

fn substitute(ty: &Type, substitutions: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Generic(name) => substitutions
            .get(name)
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Type::Struct(id, args) => Type::Struct(
            *id,
            args.iter().map(|x| substitute(x, substitutions)).collect(),
        ),
        Type::Enum(id, args) => Type::Enum(
            *id,
            args.iter().map(|x| substitute(x, substitutions)).collect(),
        ),
        Type::Option(x) => Type::Option(Box::new(substitute(x, substitutions))),
        Type::Array(x, length) => Type::Array(Box::new(substitute(x, substitutions)), *length),
        Type::Slice(x) => Type::Slice(Box::new(substitute(x, substitutions))),
        Type::List(x) => Type::List(Box::new(substitute(x, substitutions))),
        Type::Map(key, value) => Type::Map(
            Box::new(substitute(key, substitutions)),
            Box::new(substitute(value, substitutions)),
        ),
        Type::Set(x) => Type::Set(Box::new(substitute(x, substitutions))),
        Type::Thread(x) => Type::Thread(Box::new(substitute(x, substitutions))),
        Type::Future(x) => Type::Future(Box::new(substitute(x, substitutions))),
        Type::Task(x) => Type::Task(Box::new(substitute(x, substitutions))),
        Type::Mutex(x) => Type::Mutex(Box::new(substitute(x, substitutions))),
        Type::MutexGuard(x) => Type::MutexGuard(Box::new(substitute(x, substitutions))),
        Type::Channel(x) => Type::Channel(Box::new(substitute(x, substitutions))),
        Type::Result(a, b) => Type::Result(
            Box::new(substitute(a, substitutions)),
            Box::new(substitute(b, substitutions)),
        ),
        Type::Function(args, result) => Type::Function(
            args.iter().map(|x| substitute(x, substitutions)).collect(),
            Box::new(substitute(result, substitutions)),
        ),
        Type::CFunction(args, result) => Type::CFunction(
            args.iter().map(|x| substitute(x, substitutions)).collect(),
            Box::new(substitute(result, substitutions)),
        ),
        Type::Reference(inner, mutable) => {
            Type::Reference(Box::new(substitute(inner, substitutions)), *mutable)
        }
        Type::RawPointer(inner, mutable) => {
            Type::RawPointer(Box::new(substitute(inner, substitutions)), *mutable)
        }
        Type::MemoryPointer(inner, mutable) => {
            Type::MemoryPointer(Box::new(substitute(inner, substitutions)), *mutable)
        }
        _ => ty.clone(),
    }
}

fn capability_set(capabilities: &Option<Vec<CapabilityUse>>) -> Option<HashSet<Capability>> {
    capabilities.as_ref().map(|capabilities| {
        capabilities
            .iter()
            .map(|capability| capability.capability)
            .collect()
    })
}

fn positional_constraints(signature: &Signature) -> Vec<HashSet<String>> {
    signature
        .generics
        .iter()
        .map(|generic| {
            signature.constraints[generic]
                .iter()
                .cloned()
                .collect::<HashSet<_>>()
        })
        .collect()
}

fn validate_associated_references(
    ty: &Type,
    declared: &HashSet<String>,
    span: Span,
) -> Result<(), Diagnostic> {
    match ty {
        Type::Associated(name) if !declared.contains(name) => Err(Diagnostic::new(
            DiagnosticKind::Type,
            format!("trait method references undeclared associated type `Self.{name}`"),
            span,
        )),
        Type::Associated(_) => Ok(()),
        Type::Struct(_, arguments) | Type::Enum(_, arguments) => {
            for argument in arguments {
                validate_associated_references(argument, declared, span)?;
            }
            Ok(())
        }
        Type::Option(value)
        | Type::Array(value, _)
        | Type::Slice(value)
        | Type::List(value)
        | Type::Set(value)
        | Type::Thread(value)
        | Type::Future(value)
        | Type::Task(value)
        | Type::Mutex(value)
        | Type::MutexGuard(value)
        | Type::Channel(value)
        | Type::Reference(value, _)
        | Type::RawPointer(value, _)
        | Type::MemoryPointer(value, _) => validate_associated_references(value, declared, span),
        Type::Map(key, value) | Type::Result(key, value) => {
            validate_associated_references(key, declared, span)?;
            validate_associated_references(value, declared, span)
        }
        Type::Function(parameters, result) | Type::CFunction(parameters, result) => {
            for parameter in parameters {
                validate_associated_references(parameter, declared, span)?;
            }
            validate_associated_references(result, declared, span)
        }
        _ => Ok(()),
    }
}

fn substitute_associated(ty: &Type, associated: &HashMap<String, Type>) -> Type {
    match ty {
        Type::Associated(name) => associated.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Type::Struct(id, arguments) => Type::Struct(
            *id,
            arguments
                .iter()
                .map(|argument| substitute_associated(argument, associated))
                .collect(),
        ),
        Type::Enum(id, arguments) => Type::Enum(
            *id,
            arguments
                .iter()
                .map(|argument| substitute_associated(argument, associated))
                .collect(),
        ),
        Type::Option(value) => Type::Option(Box::new(substitute_associated(value, associated))),
        Type::Array(value, length) => {
            Type::Array(Box::new(substitute_associated(value, associated)), *length)
        }
        Type::Slice(value) => Type::Slice(Box::new(substitute_associated(value, associated))),
        Type::List(value) => Type::List(Box::new(substitute_associated(value, associated))),
        Type::Map(key, value) => Type::Map(
            Box::new(substitute_associated(key, associated)),
            Box::new(substitute_associated(value, associated)),
        ),
        Type::Set(value) => Type::Set(Box::new(substitute_associated(value, associated))),
        Type::Thread(value) => Type::Thread(Box::new(substitute_associated(value, associated))),
        Type::Future(value) => Type::Future(Box::new(substitute_associated(value, associated))),
        Type::Task(value) => Type::Task(Box::new(substitute_associated(value, associated))),
        Type::Mutex(value) => Type::Mutex(Box::new(substitute_associated(value, associated))),
        Type::MutexGuard(value) => {
            Type::MutexGuard(Box::new(substitute_associated(value, associated)))
        }
        Type::Channel(value) => Type::Channel(Box::new(substitute_associated(value, associated))),
        Type::Result(ok, error) => Type::Result(
            Box::new(substitute_associated(ok, associated)),
            Box::new(substitute_associated(error, associated)),
        ),
        Type::Function(parameters, result) => Type::Function(
            parameters
                .iter()
                .map(|parameter| substitute_associated(parameter, associated))
                .collect(),
            Box::new(substitute_associated(result, associated)),
        ),
        Type::CFunction(parameters, result) => Type::CFunction(
            parameters
                .iter()
                .map(|parameter| substitute_associated(parameter, associated))
                .collect(),
            Box::new(substitute_associated(result, associated)),
        ),
        Type::Reference(inner, mutable) => {
            Type::Reference(Box::new(substitute_associated(inner, associated)), *mutable)
        }
        Type::RawPointer(inner, mutable) => {
            Type::RawPointer(Box::new(substitute_associated(inner, associated)), *mutable)
        }
        Type::MemoryPointer(inner, mutable) => {
            Type::MemoryPointer(Box::new(substitute_associated(inner, associated)), *mutable)
        }
        _ => ty.clone(),
    }
}

fn type_contains_generic_name(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Generic(generic) => generic == name,
        Type::Struct(_, arguments) | Type::Enum(_, arguments) => arguments
            .iter()
            .any(|argument| type_contains_generic_name(argument, name)),
        Type::Option(value)
        | Type::Array(value, _)
        | Type::Slice(value)
        | Type::List(value)
        | Type::Set(value)
        | Type::Thread(value)
        | Type::Future(value)
        | Type::Task(value)
        | Type::Mutex(value)
        | Type::MutexGuard(value)
        | Type::Channel(value)
        | Type::Reference(value, _)
        | Type::RawPointer(value, _)
        | Type::MemoryPointer(value, _) => type_contains_generic_name(value, name),
        Type::Map(key, value) | Type::Result(key, value) => {
            type_contains_generic_name(key, name) || type_contains_generic_name(value, name)
        }
        Type::Function(parameters, result) | Type::CFunction(parameters, result) => {
            parameters
                .iter()
                .any(|parameter| type_contains_generic_name(parameter, name))
                || type_contains_generic_name(result, name)
        }
        _ => false,
    }
}

fn types_overlap(left: &Type, right: &Type) -> bool {
    match (left, right) {
        (Type::Generic(_), _) | (_, Type::Generic(_)) => true,
        (Type::Struct(a, xs), Type::Struct(b, ys)) | (Type::Enum(a, xs), Type::Enum(b, ys)) => {
            a == b && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| types_overlap(x, y))
        }
        (Type::Option(x), Type::Option(y)) => types_overlap(x, y),
        (Type::Array(x, a), Type::Array(y, b)) => a == b && types_overlap(x, y),
        (Type::Slice(x), Type::Slice(y)) => types_overlap(x, y),
        (Type::List(x), Type::List(y)) => types_overlap(x, y),
        (Type::Thread(x), Type::Thread(y))
        | (Type::Future(x), Type::Future(y))
        | (Type::Task(x), Type::Task(y))
        | (Type::Mutex(x), Type::Mutex(y))
        | (Type::MutexGuard(x), Type::MutexGuard(y))
        | (Type::Channel(x), Type::Channel(y)) => types_overlap(x, y),
        (Type::Map(ak, av), Type::Map(bk, bv)) => types_overlap(ak, bk) && types_overlap(av, bv),
        (Type::Set(x), Type::Set(y)) => types_overlap(x, y),
        (Type::Result(a, b), Type::Result(x, y)) => types_overlap(a, x) && types_overlap(b, y),
        _ => left == right,
    }
}

fn contains_infer(ty: &Type) -> bool {
    match ty {
        Type::Infer => true,
        Type::Option(value) => contains_infer(value),
        Type::Array(element, _)
        | Type::Slice(element)
        | Type::List(element)
        | Type::Thread(element)
        | Type::Future(element)
        | Type::Task(element)
        | Type::Mutex(element)
        | Type::MutexGuard(element)
        | Type::Channel(element) => contains_infer(element),
        Type::Map(key, value) => contains_infer(key) || contains_infer(value),
        Type::Set(element) => contains_infer(element),
        Type::Result(ok, error) => contains_infer(ok) || contains_infer(error),
        Type::Function(parameters, result) | Type::CFunction(parameters, result) => {
            parameters.iter().any(contains_infer) || contains_infer(result)
        }
        Type::Reference(inner, _) | Type::RawPointer(inner, _) | Type::MemoryPointer(inner, _) => {
            contains_infer(inner)
        }
        _ => false,
    }
}

fn is_c_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    let syntax_valid = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    syntax_valid
        && !name.starts_with("__")
        && !name.starts_with("disp_")
        && !name.starts_with("dv_")
        && !matches!(
            name,
            "alignas"
                | "alignof"
                | "and"
                | "and_eq"
                | "asm"
                | "auto"
                | "bitand"
                | "bitor"
                | "bool"
                | "break"
                | "case"
                | "catch"
                | "char"
                | "char16_t"
                | "char32_t"
                | "class"
                | "compl"
                | "concept"
                | "const"
                | "const_cast"
                | "constexpr"
                | "continue"
                | "decltype"
                | "default"
                | "delete"
                | "do"
                | "double"
                | "dynamic_cast"
                | "else"
                | "enum"
                | "explicit"
                | "extern"
                | "false"
                | "float"
                | "for"
                | "friend"
                | "goto"
                | "if"
                | "inline"
                | "int"
                | "long"
                | "mutable"
                | "namespace"
                | "new"
                | "noexcept"
                | "not"
                | "not_eq"
                | "nullptr"
                | "operator"
                | "or"
                | "or_eq"
                | "private"
                | "protected"
                | "public"
                | "register"
                | "reinterpret_cast"
                | "requires"
                | "restrict"
                | "return"
                | "short"
                | "signed"
                | "sizeof"
                | "static"
                | "static_assert"
                | "static_cast"
                | "struct"
                | "switch"
                | "template"
                | "this"
                | "thread_local"
                | "throw"
                | "true"
                | "try"
                | "typedef"
                | "typeid"
                | "typename"
                | "union"
                | "unsigned"
                | "using"
                | "virtual"
                | "void"
                | "volatile"
                | "wchar_t"
                | "while"
                | "xor"
                | "xor_eq"
                | "_Alignas"
                | "_Alignof"
                | "_Atomic"
                | "_Bool"
                | "_Complex"
                | "_Generic"
                | "_Imaginary"
                | "_Noreturn"
                | "_Static_assert"
                | "_Thread_local"
        )
}

fn type_crosses_thread_by_borrow(ty: &Type) -> bool {
    match ty {
        Type::Reference(_, _)
        | Type::RawPointer(_, _)
        | Type::MemoryPointer(_, _)
        | Type::Slice(_)
        | Type::Str
        | Type::CStr
        | Type::MutexGuard(_)
        | Type::CFunction(_, _) => true,
        Type::CRegistration => true,
        Type::Array(inner, _)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Option(inner)
        | Type::Thread(inner)
        | Type::Future(inner)
        | Type::Task(inner)
        | Type::Mutex(inner)
        | Type::Channel(inner) => type_crosses_thread_by_borrow(inner),
        Type::Map(key, value) | Type::Result(key, value) => {
            type_crosses_thread_by_borrow(key) || type_crosses_thread_by_borrow(value)
        }
        _ => false,
    }
}

fn type_contains_task(ty: &Type) -> bool {
    match ty {
        Type::Task(_) => true,
        Type::Struct(_, arguments) | Type::Enum(_, arguments) => {
            arguments.iter().any(type_contains_task)
        }
        Type::Option(inner)
        | Type::Array(inner, _)
        | Type::Slice(inner)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Thread(inner)
        | Type::Future(inner)
        | Type::Mutex(inner)
        | Type::MutexGuard(inner)
        | Type::Channel(inner)
        | Type::Reference(inner, _)
        | Type::RawPointer(inner, _)
        | Type::MemoryPointer(inner, _) => type_contains_task(inner),
        Type::Map(key, value) | Type::Result(key, value) => {
            type_contains_task(key) || type_contains_task(value)
        }
        Type::Function(parameters, result) | Type::CFunction(parameters, result) => {
            parameters.iter().any(type_contains_task) || type_contains_task(result)
        }
        _ => false,
    }
}

fn merge_types(left: &Type, right: &Type) -> Type {
    match (left, right) {
        (Type::Infer, other) | (other, Type::Infer) => other.clone(),
        (
            Type::IntLiteral(_) | Type::NegativeIntLiteral(_),
            Type::IntLiteral(_) | Type::NegativeIntLiteral(_),
        ) => Type::Int,
        (Type::FloatLiteral, Type::FloatLiteral) => Type::Float,
        (Type::Option(left), Type::Option(right)) => {
            Type::Option(Box::new(merge_types(left, right)))
        }
        (Type::Array(left, length), Type::Array(right, other)) if length == other => {
            Type::Array(Box::new(merge_types(left, right)), *length)
        }
        (Type::List(left), Type::List(right)) => Type::List(Box::new(merge_types(left, right))),
        (Type::Thread(left), Type::Thread(right)) => {
            Type::Thread(Box::new(merge_types(left, right)))
        }
        (Type::Future(left), Type::Future(right)) => {
            Type::Future(Box::new(merge_types(left, right)))
        }
        (Type::Task(left), Type::Task(right)) => Type::Task(Box::new(merge_types(left, right))),
        (Type::Mutex(left), Type::Mutex(right)) => Type::Mutex(Box::new(merge_types(left, right))),
        (Type::MutexGuard(left), Type::MutexGuard(right)) => {
            Type::MutexGuard(Box::new(merge_types(left, right)))
        }
        (Type::Channel(left), Type::Channel(right)) => {
            Type::Channel(Box::new(merge_types(left, right)))
        }
        (Type::Map(lk, lv), Type::Map(rk, rv)) => {
            Type::Map(Box::new(merge_types(lk, rk)), Box::new(merge_types(lv, rv)))
        }
        (Type::Set(left), Type::Set(right)) => Type::Set(Box::new(merge_types(left, right))),
        (Type::Result(left_ok, left_error), Type::Result(right_ok, right_error)) => Type::Result(
            Box::new(merge_types(left_ok, right_ok)),
            Box::new(merge_types(left_error, right_error)),
        ),
        _ => left.clone(),
    }
}

fn atomic_load_method(name: &str) -> bool {
    matches!(
        name,
        "load" | "load_relaxed" | "load_acquire" | "load_seq_cst"
    )
}

fn atomic_store_method(name: &str) -> bool {
    matches!(
        name,
        "store" | "store_relaxed" | "store_release" | "store_seq_cst"
    )
}

fn atomic_add_method(name: &str) -> bool {
    matches!(
        name,
        "add" | "add_relaxed" | "add_acquire" | "add_release" | "add_acq_rel" | "add_seq_cst"
    )
}

fn atomic_fetch_add_method(name: &str) -> bool {
    matches!(
        name,
        "fetch_add"
            | "fetch_add_relaxed"
            | "fetch_add_acquire"
            | "fetch_add_release"
            | "fetch_add_acq_rel"
            | "fetch_add_seq_cst"
    )
}

fn is_numeric(ty: &Type) -> bool {
    is_integer(ty) || matches!(ty, Type::Float | Type::Float32 | Type::FloatLiteral)
}

fn is_integer(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int
            | Type::IntLiteral(_)
            | Type::NegativeIntLiteral(_)
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
    )
}

fn is_data_order_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
            | Type::Float
            | Type::Float32
            | Type::String
            | Type::Char
            | Type::Bool
    )
}

fn is_collection_key(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int
            | Type::UInt
            | Type::Signed(_)
            | Type::Unsigned(_)
            | Type::IntLiteral(_)
            | Type::NegativeIntLiteral(_)
            | Type::Bool
            | Type::Char
            | Type::String
            | Type::Str
    )
}

fn numeric_type(name: &str) -> Option<Type> {
    Some(match name {
        "int" => Type::Int,
        "uint" => Type::UInt,
        "i8" => Type::Signed(8),
        "i16" => Type::Signed(16),
        "i32" => Type::Signed(32),
        "i64" => Type::Signed(64),
        "i128" => Type::Signed(128),
        "u8" => Type::Unsigned(8),
        "u16" => Type::Unsigned(16),
        "u32" => Type::Unsigned(32),
        "u64" => Type::Unsigned(64),
        "u128" => Type::Unsigned(128),
        "f32" => Type::Float32,
        "f64" => Type::Float,
        _ => return None,
    })
}

fn contains_break_for_current_loop(block: &Block) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.node {
            Statement::Break => true,
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                contains_break_for_current_loop(then_branch)
                    || else_branch
                        .as_ref()
                        .is_some_and(contains_break_for_current_loop)
            }
            Statement::While { .. }
            | Statement::For { .. }
            | Statement::ForEach { .. }
            | Statement::Loop(_) => false,
            _ => false,
        })
}

fn implicit_shared_borrow_target<'a>(expected: &Type, actual: &'a Type) -> Option<&'a Type> {
    matches!(expected, Type::Reference(_, false))
        .then_some(actual)
        .filter(|_| !matches!(actual, Type::Reference(_, _)))
}

fn is_storage_expression(expression: &Expr) -> bool {
    matches!(
        expression.node,
        Expression::Identifier(_) | Expression::FieldAccess { .. } | Expression::Index { .. }
    )
}

fn http_header_token(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn http_forbidden_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "proxy-connection"
            | "proxy-authorization"
            | "trailer"
            | "te"
            | "upgrade"
    )
}

fn http_method_token(method: &str) -> bool {
    !method.is_empty()
        && method.len() <= 32
        && http_header_token(method)
        && !matches!(method.to_ascii_uppercase().as_str(), "CONNECT" | "TRACE")
}

fn http_body_type(ty: &Type) -> bool {
    matches!(ty, Type::String | Type::Str)
        || matches!(ty,Type::List(element)|Type::Slice(element) if matches!(**element,Type::Unsigned(8)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lexer::Lexer, parser::Parser, resolver::Resolver};

    fn check(source: &str) -> Result<(), Diagnostic> {
        let program = Parser::new(Lexer::new(source).tokenize()?).parse()?;
        Resolver::new().resolve(&program)?;
        TypeChecker::new().check(&program)
    }

    #[test]
    fn checks_user_functions_and_returns() {
        assert!(
            check(
                "fn add(a: int, b: int) -> int { return a + b } fn main() { print(add(10, 20)) }"
            )
            .is_ok()
        );
        assert!(check("fn bad() -> bool { return 1 } fn main() {}").is_err());
        assert!(check("fn bad() -> int { if true { return 1 } } fn main() {}").is_err());
    }

    #[test]
    fn checks_annotations_assignments_and_conditions() {
        assert!(check("fn main() { var x: int = 1 x += 2 if x > 0 { print(x) } }").is_ok());
        assert!(check("fn main() { var x: bool = 1 }").is_err());
        assert!(check("fn main() { if 1 {} }").is_err());
    }

    #[test]
    fn constants_require_compile_time_expressions() {
        assert!(check("fn main() { const x = 1 + 2 const y = x * 3 print(y) }").is_ok());
        assert!(check("fn value() -> int { return 1 } fn main() { const x = value() }").is_err());
    }

    #[test]
    fn checks_unary_and_call_arity() {
        assert!(
            check("fn id(x: bool) -> bool { return !x } fn main() { print(id(false)) }").is_ok()
        );
        assert!(check("fn id(x: int) -> int { return x } fn main() { print(id()) }").is_err());
        assert!(check("fn main() { print(-true) }").is_err());
    }

    #[test]
    fn checks_exact_width_numeric_ranges() {
        assert!(
            check("fn value(x: i32) -> i32 { return x } fn main() { let x: i8 = 127 }").is_ok()
        );
        assert!(check("fn main() { let x: i8 = 128 }").is_err());
        assert!(check("fn main() { let x: u8 = -1 }").is_err());
    }
}
