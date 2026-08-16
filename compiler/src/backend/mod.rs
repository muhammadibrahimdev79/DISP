pub mod abi;
pub mod allocator;
pub mod c_header;
pub mod codegen;
pub mod crypto_runtime;
pub mod layout;
pub mod linker;
pub mod mono;
pub mod native_types;
pub mod runtime;
pub mod target;
pub mod typed_codegen;

use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    hir, limits, mir,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    pub optimized: bool,
    pub emit_c: bool,
    pub emit_object: bool,
    pub sanitizers: bool,
    pub library: bool,
}
#[derive(Debug, Clone)]
pub struct BuildArtifacts {
    pub executable: PathBuf,
    pub object: Option<PathBuf>,
    pub backend_ir: Option<PathBuf>,
    pub reused: bool,
}

struct BuildPaths {
    directory: PathBuf,
    c: PathBuf,
    object: PathBuf,
    executable: PathBuf,
    fingerprint: PathBuf,
}

pub fn build(
    hir: &hir::Program,
    mir: &mir::Program,
    source_path: &Path,
    options: BuildOptions,
) -> Result<BuildArtifacts, Diagnostic> {
    mir::validate(mir)?;
    if let Some(span) = mir.functions.iter().find_map(|function| {
        function
            .blocks
            .iter()
            .find_map(|block| match &block.terminator {
                mir::Terminator::Call {
                    target: hir::CallTarget::Intrinsic(name),
                    span,
                    ..
                } if name.starts_with("Port.") || name.starts_with("Mmio.") => Some(*span),
                _ => None,
            })
    }) {
        return Err(Diagnostic::new(
            DiagnosticKind::Backend,
            "direct hardware I/O is unavailable in hosted native processes",
            span,
        )
        .with_help(
            "compile authorized port I/O with `--freestanding32`/`--freestanding64`, or authenticated MMIO with `--freestanding-aarch64`",
        ));
    }
    validate_exports(mir, options.library)?;
    let paths = build_paths(source_path, options.library)?;
    let native_crypto = mir.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            matches!(
                &block.terminator,
                mir::Terminator::Call {
                    target: hir::CallTarget::Intrinsic(name),
                    ..
                } if matches!(name.as_str(), "Crypto.aes256_gcm_siv_seal" | "Crypto.aes256_gcm_siv_open" | "Crypto.ed25519_generate" | "Crypto.ed25519_public_key" | "Crypto.ed25519_sign" | "Crypto.ed25519_verify" | "Crypto.ed25519_key_id" | "Crypto.ed25519_verify_keyed" | "Crypto.ed25519_verify_lifecycle" | "Crypto.argon2id_hash_password" | "Crypto.argon2id_verify_password")
            )
        })
    });
    let crypto_runtime = native_crypto.then(crypto_runtime::locate).transpose()?;
    let fingerprint = build_fingerprint(hir, options, crypto_runtime.as_deref());
    if fingerprint
        .as_deref()
        .is_some_and(|expected| cache_matches(&paths, options, expected))
    {
        if native_crypto {
            crypto_runtime::stage_for(&paths.executable)?;
        }
        return Ok(BuildArtifacts {
            executable: paths.executable,
            object: options.emit_object.then_some(paths.object),
            backend_ir: options.emit_c.then_some(paths.c),
            reused: true,
        });
    }

    let target = target::Target::host()?;
    let mono = mono::collect(mir)?;
    validate_layouts(hir, mir, &mono, target)?;
    let abi = abi::lower(hir, mir, &mono, target)?;
    let native_types = native_types::generate(hir, &mono, target)?;
    let generated = codegen::generate(mir, &mono, &abi, &native_types, options.library)?;
    if generated.source.len() > limits::MAX_GENERATED_C_BYTES {
        return Err(error(&format!(
            "generated native source exceeds the {}-byte safety limit",
            limits::MAX_GENERATED_C_BYTES
        )));
    }
    let http = mir.functions.iter().any(|function| {
        function
            .locals
            .iter()
            .any(|local| type_uses_http(&local.ty))
            || function.blocks.iter().any(|block| {
                matches!(
                    &block.terminator,
                    mir::Terminator::Call {
                        target: hir::CallTarget::Intrinsic(name),
                        ..
                    } if name.starts_with("Http.") || name.starts_with("HttpResponse.")
                )
            })
    });
    let networking = http || mir.functions.iter().any(|function| {
        function
            .locals
            .iter()
            .any(|local| type_uses_networking(&local.ty))
            || function.blocks.iter().any(|block| {
                matches!(
                    &block.terminator,
                    mir::Terminator::Call {
                        target: hir::CallTarget::Intrinsic(name),
                        ..
                    } if name.starts_with("Async.connect") || name.starts_with("Http.") || name.starts_with("HttpResponse.") || name.starts_with("TcpListener.") || name.starts_with("TcpStream.") || name.starts_with("UdpSocket.") || name.starts_with("UdpDatagram.")
                )
            })
    });
    let database = mir.functions.iter().any(|function| {
        function
            .locals
            .iter()
            .any(|local| type_uses_database(&local.ty))
            || function.blocks.iter().any(|block| {
                matches!(
                    &block.terminator,
                    mir::Terminator::Call {
                        target: hir::CallTarget::Intrinsic(name),
                        ..
                    } if name.starts_with("Database.")
                )
            })
    });
    let data = mir.functions.iter().any(|function| {
        function
            .locals
            .iter()
            .any(|local| type_uses_data_store(&local.ty))
            || function.blocks.iter().any(|block| {
                matches!(
                    &block.terminator,
                    mir::Terminator::Call {
                        target: hir::CallTarget::Intrinsic(name),
                        ..
                    } if name.starts_with("DataStore.")
                ) || matches!(
                    &block.terminator,
                    mir::Terminator::Call {
                        target: hir::CallTarget::Data(_),
                        ..
                    }
                )
            })
    });
    fs::create_dir_all(&paths.directory)
        .map_err(|cause| error(&format!("could not create native build directory: {cause}")))?;
    fs::write(&paths.c, generated.source)
        .map_err(|cause| error(&format!("could not write backend C: {cause}")))?;
    let libraries = mono
        .instances
        .iter()
        .filter_map(|instance| {
            hir.functions[instance.function.0]
                .external
                .as_ref()
                .and_then(|external| external.library.clone())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    linker::compile_and_link(
        &paths.c,
        &paths.object,
        &paths.executable,
        options.optimized,
        options.sanitizers,
        linker::RuntimeFeatures {
            networking,
            http,
            database,
            data,
            native_crypto_library: crypto_runtime.clone(),
            shared: options.library,
        },
        &libraries,
    )?;
    if native_crypto {
        crypto_runtime::stage_for(&paths.executable)?;
    }
    if !options.emit_c {
        let _ = fs::remove_file(&paths.c);
    }
    if !options.emit_object {
        let _ = fs::remove_file(&paths.object);
    }
    if let Some(fingerprint) = fingerprint {
        let _ = fs::write(&paths.fingerprint, format!("{fingerprint}\n"));
    }
    Ok(BuildArtifacts {
        executable: paths.executable,
        object: options.emit_object.then_some(paths.object),
        backend_ir: options.emit_c.then_some(paths.c),
        reused: false,
    })
}

fn build_paths(source_path: &Path, library: bool) -> Result<BuildPaths, Diagnostic> {
    let project_root;
    let (stem, parent) = if source_path.is_dir() {
        project_root = if source_path.is_absolute() {
            source_path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|cause| error(&format!("could not resolve current directory: {cause}")))?
                .join(source_path)
        };
        let stem = project_root
            .file_name()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| error("project directory has no valid output name"))?;
        (stem, project_root.as_path())
    } else {
        let stem = source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| error("source file has no valid output name"))?;
        (stem, source_path.parent().unwrap_or_else(|| Path::new(".")))
    };
    let safe_stem = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let root = if let Some(configured) =
        std::env::var_os("DISP_BUILD_ROOT").filter(|value| !value.is_empty())
    {
        let absolute_source = if source_path.is_absolute() {
            source_path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|cause| error(&format!("could not resolve current directory: {cause}")))?
                .join(source_path)
        };
        let mut digest = Sha256::new();
        digest.update(b"DISP external build path v1\0");
        digest.update(absolute_source.to_string_lossy().as_bytes());
        let identity = digest
            .finalize()
            .iter()
            .take(12)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        PathBuf::from(configured).join(identity)
    } else {
        parent.join("build")
    };
    let directory = root.join(&safe_stem);
    let extension = if library {
        if cfg!(windows) {
            "dll"
        } else if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        }
    } else {
        "exe"
    };
    Ok(BuildPaths {
        c: directory.join(format!("{safe_stem}.backend.c")),
        object: directory.join(format!("{safe_stem}.o")),
        fingerprint: directory.join("fingerprint.sha256"),
        executable: root.join(format!("{safe_stem}.{extension}")),
        directory,
    })
}

fn build_fingerprint(
    program: &hir::Program,
    options: BuildOptions,
    crypto_runtime: Option<&Path>,
) -> Option<String> {
    // `lower_source` intentionally has no filesystem identity. Callers may reuse an
    // output path for different in-memory programs, so those builds cannot be cached
    // without an explicit content identity from the caller.
    if program.source_files.is_empty() {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(b"DISP native build cache v1\0");
    digest.update(env!("CARGO_PKG_VERSION").as_bytes());
    digest.update([
        options.optimized as u8,
        options.emit_c as u8,
        options.emit_object as u8,
        options.sanitizers as u8,
        options.library as u8,
    ]);
    digest.update(std::env::consts::OS.as_bytes());
    digest.update([0]);
    digest.update(std::env::consts::ARCH.as_bytes());
    digest.update([0]);
    for variable in ["DISP_CC", "DISP_ZIG"] {
        digest.update(variable.as_bytes());
        digest.update([0]);
        if let Some(value) = std::env::var_os(variable) {
            digest.update(value.to_string_lossy().as_bytes());
        }
        digest.update([0]);
    }

    let compiler = std::env::current_exe().ok()?;
    digest.update(fs::read(compiler).ok()?);
    if let Some(runtime) = crypto_runtime {
        digest.update(fs::read(runtime).ok()?);
    }

    let mut sources = program.source_files.iter().collect::<Vec<_>>();
    sources.sort_by(|left, right| left.identity_path.cmp(&right.identity_path));
    for source in sources {
        digest.update(source.identity_path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(fs::read(&source.identity_path).ok()?);
        digest.update([0]);
    }
    Some(format!("{:x}", digest.finalize()))
}

fn cache_matches(paths: &BuildPaths, options: BuildOptions, expected: &str) -> bool {
    let executable_exists =
        fs::metadata(&paths.executable).is_ok_and(|metadata| metadata.len() > 0);
    let object_exists = !options.emit_object || paths.object.is_file();
    let c_exists = !options.emit_c || paths.c.is_file();
    executable_exists
        && object_exists
        && c_exists
        && (!options.sanitizers || sanitized_runtime_is_present(&paths.executable))
        && fs::read_to_string(&paths.fingerprint).is_ok_and(|actual| actual.trim() == expected)
}

fn sanitized_runtime_is_present(executable: &Path) -> bool {
    if !cfg!(windows) {
        return true;
    }
    executable
        .parent()
        .and_then(|directory| fs::read_dir(directory).ok())
        .is_some_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with("clang_rt.asan_dynamic-") && name.ends_with(".dll")
                })
            })
        })
}

fn type_uses_database(ty: &hir::Type) -> bool {
    match ty {
        hir::Type::Database => true,
        hir::Type::Array(inner, _)
        | hir::Type::Slice(inner)
        | hir::Type::List(inner)
        | hir::Type::Set(inner)
        | hir::Type::Thread(inner)
        | hir::Type::Future(inner)
        | hir::Type::Task(inner)
        | hir::Type::Mutex(inner)
        | hir::Type::MutexGuard(inner)
        | hir::Type::Channel(inner)
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. }
        | hir::Type::MemoryPointer { inner, .. } => type_uses_database(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_database(key) || type_uses_database(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_database)
        }
        hir::Type::Function(arguments, result) | hir::Type::CFunction(arguments, result) => {
            arguments.iter().any(type_uses_database) || type_uses_database(result)
        }
        _ => false,
    }
}

fn type_uses_data_store(ty: &hir::Type) -> bool {
    match ty {
        hir::Type::DataStore => true,
        hir::Type::Array(inner, _)
        | hir::Type::Slice(inner)
        | hir::Type::List(inner)
        | hir::Type::Set(inner)
        | hir::Type::Thread(inner)
        | hir::Type::Future(inner)
        | hir::Type::Task(inner)
        | hir::Type::Mutex(inner)
        | hir::Type::MutexGuard(inner)
        | hir::Type::Channel(inner)
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. }
        | hir::Type::MemoryPointer { inner, .. } => type_uses_data_store(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_data_store(key) || type_uses_data_store(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_data_store)
        }
        hir::Type::Function(arguments, result) | hir::Type::CFunction(arguments, result) => {
            arguments.iter().any(type_uses_data_store) || type_uses_data_store(result)
        }
        _ => false,
    }
}

fn type_uses_http(ty: &hir::Type) -> bool {
    match ty {
        hir::Type::Url | hir::Type::HttpRequest | hir::Type::HttpResponse => true,
        hir::Type::Array(inner, _)
        | hir::Type::Slice(inner)
        | hir::Type::List(inner)
        | hir::Type::Set(inner)
        | hir::Type::Thread(inner)
        | hir::Type::Future(inner)
        | hir::Type::Task(inner)
        | hir::Type::Mutex(inner)
        | hir::Type::MutexGuard(inner)
        | hir::Type::Channel(inner)
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. }
        | hir::Type::MemoryPointer { inner, .. } => type_uses_http(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_http(key) || type_uses_http(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_http)
        }
        hir::Type::Function(arguments, result) | hir::Type::CFunction(arguments, result) => {
            arguments.iter().any(type_uses_http) || type_uses_http(result)
        }
        _ => false,
    }
}

fn type_uses_networking(ty: &hir::Type) -> bool {
    match ty {
        hir::Type::IpAddress
        | hir::Type::Url
        | hir::Type::SocketAddress
        | hir::Type::TcpStream
        | hir::Type::TlsStream
        | hir::Type::HttpRequest
        | hir::Type::HttpResponse
        | hir::Type::TcpListener
        | hir::Type::UdpSocket
        | hir::Type::UdpDatagram => true,
        hir::Type::Array(inner, _)
        | hir::Type::Slice(inner)
        | hir::Type::List(inner)
        | hir::Type::Set(inner)
        | hir::Type::Thread(inner)
        | hir::Type::Future(inner)
        | hir::Type::Task(inner)
        | hir::Type::Mutex(inner)
        | hir::Type::MutexGuard(inner)
        | hir::Type::Channel(inner)
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. }
        | hir::Type::MemoryPointer { inner, .. } => type_uses_networking(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_networking(key) || type_uses_networking(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_networking)
        }
        hir::Type::Function(arguments, result) | hir::Type::CFunction(arguments, result) => {
            arguments.iter().any(type_uses_networking) || type_uses_networking(result)
        }
        _ => false,
    }
}

fn validate_layouts(
    program: &hir::Program,
    mir: &mir::Program,
    mono: &mono::MonoProgram,
    target: target::Target,
) -> Result<(), Diagnostic> {
    let mut engine = layout::LayoutEngine::new(target, program);
    for instance in &mono.instances {
        let function = &mir.functions[instance.function.0];
        let substitutions = mono::mapping(function, instance);
        for local in &function.locals {
            engine.layout(&layout::substitute(&local.ty, &substitutions))?;
        }
    }
    Ok(())
}

fn validate_exports(program: &mir::Program, library: bool) -> Result<(), Diagnostic> {
    let mut callback_roots = Vec::new();
    for function in &program.functions {
        for block in &function.blocks {
            if let mir::Terminator::Call {
                target: hir::CallTarget::Intrinsic(name),
                span,
                ..
            } = &block.terminator
                && let Some(encoded) = name.strip_prefix("CRegistration.register_async:")
            {
                let target = encoded.parse::<usize>().map_err(|_| {
                    Diagnostic::new(
                        DiagnosticKind::Backend,
                        "captured callback contains an invalid handler identity",
                        *span,
                    )
                })?;
                callback_roots.push(hir::FunctionId(target));
            }
        }
    }
    let mut callback_verified = BTreeSet::new();
    for root in callback_roots {
        validate_c_callback_graph(
            program,
            root,
            root,
            &mut BTreeSet::new(),
            &mut callback_verified,
        )?;
    }
    if library && !program.functions.iter().any(|function| function.exported) {
        return Err(error(
            "a DISP shared library must declare at least one `export C fn`",
        ));
    }
    let mut verified = BTreeSet::new();
    for function in program
        .functions
        .iter()
        .filter(|function| function.exported)
    {
        validate_export_graph(
            program,
            function.id,
            function.id,
            &mut BTreeSet::new(),
            &mut verified,
        )?;
    }
    Ok(())
}

fn validate_c_callback_graph(
    program: &mir::Program,
    root: hir::FunctionId,
    current: hir::FunctionId,
    visiting: &mut BTreeSet<hir::FunctionId>,
    verified: &mut BTreeSet<hir::FunctionId>,
) -> Result<(), Diagnostic> {
    if verified.contains(&current) || !visiting.insert(current) {
        return Ok(());
    }
    let root_function = program
        .functions
        .get(root.0)
        .ok_or_else(|| error("captured callback references an invalid handler identity"))?;
    let function = program
        .functions
        .get(current.0)
        .ok_or_else(|| error("captured callback reaches an invalid function identity"))?;
    if function.external.is_some() || function.asynchronous {
        return Err(Diagnostic::new(
            DiagnosticKind::Backend,
            format!(
                "captured C callback `{}` reaches unsupported function `{}`",
                root_function.name, function.name
            ),
            function.span,
        )
        .with_help(
            "keep captured callbacks synchronous and perform foreign work outside the callback",
        ));
    }
    if function.locals.iter().any(|local| local.needs_drop) {
        return Err(Diagnostic::new(
            DiagnosticKind::Backend,
            format!(
                "captured C callback `{}` reaches cleanup-bearing storage in `{}`",
                root_function.name, function.name
            ),
            function.span,
        )
        .with_help(
            "keep the initial callback body allocation-free so checked failure cannot skip cleanup",
        ));
    }
    for block in &function.blocks {
        match &block.terminator {
            mir::Terminator::Call {
                target: hir::CallTarget::Function(callee),
                ..
            } => validate_c_callback_graph(program, root, *callee, visiting, verified)?,
            mir::Terminator::Call {
                target: hir::CallTarget::Intrinsic(name),
                ..
            } if matches!(
                name.as_str(),
                "String.len"
                    | "String.capacity"
                    | "String.is_empty"
                    | "CString.len"
                    | "CString.is_empty"
            ) => {}
            mir::Terminator::Call { span, .. }
            | mir::Terminator::Spawn { span, .. }
            | mir::Terminator::Await { span, .. } => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Backend,
                    format!(
                        "captured C callback `{}` reaches an indirect, intrinsic, foreign, asynchronous, or data operation",
                        root_function.name
                    ),
                    *span,
                )
                .with_help("use an allocation-free direct DISP helper graph inside captured callbacks"));
            }
            _ => {}
        }
    }
    visiting.remove(&current);
    verified.insert(current);
    Ok(())
}

fn validate_export_graph(
    program: &mir::Program,
    root: hir::FunctionId,
    current: hir::FunctionId,
    visiting: &mut BTreeSet<hir::FunctionId>,
    verified: &mut BTreeSet<hir::FunctionId>,
) -> Result<(), Diagnostic> {
    if verified.contains(&current) || !visiting.insert(current) {
        return Ok(());
    }
    let root_function = &program.functions[root.0];
    let function = program
        .functions
        .get(current.0)
        .ok_or_else(|| error("an exported function references an invalid function identity"))?;
    if function.external.is_some() || function.asynchronous {
        return Err(Diagnostic::new(
            DiagnosticKind::Backend,
            format!(
                "exported function `{}` reaches unsupported function `{}` across its contained C boundary",
                root_function.name, function.name
            ),
            function.span,
        )
        .with_help("use only synchronous DISP helpers in the allocation-free export graph"));
    }
    if let Some(local) = function.locals.iter().find(|local| {
        local.needs_drop && !export_abort_cleanup_safe(program, &local.ty, &mut BTreeSet::new())
    }) {
        return Err(Diagnostic::new(
            DiagnosticKind::Backend,
            format!(
                "exported function `{}` reaches side-effecting cleanup storage `{}` in `{}` that cannot cross a contained C panic boundary",
                root_function.name, local.name, function.name
            ),
            function.span,
        )
        .with_help(
            "use heap-only owned values or CRegistration; every other handle, secret, task, thread, or callable environment requires its own typed rollback hook",
        ));
    }
    for block in &function.blocks {
        match &block.terminator {
            mir::Terminator::Call {
                target: hir::CallTarget::Function(callee),
                ..
            } => validate_export_graph(program, root, *callee, visiting, verified)?,
            mir::Terminator::Call {
                target: hir::CallTarget::ForeignCallable,
                ..
            } => {}
            mir::Terminator::Call {
                target: hir::CallTarget::Intrinsic(name),
                ..
            } if export_cleanup_safe_intrinsic(name) => {}
            mir::Terminator::Call { span, .. }
            | mir::Terminator::Spawn { span, .. }
            | mir::Terminator::Await { span, .. } => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Backend,
                    format!(
                        "exported function `{}` reaches an unsupported indirect DISP, side-effecting intrinsic, asynchronous, or data operation outside the contained ABI-v1 subset",
                        root_function.name
                    ),
                    *span,
                )
                .with_help(
                    "use synchronous direct DISP helpers and only rollback-safe heap operations",
                ));
            }
            _ => {}
        }
    }
    visiting.remove(&current);
    verified.insert(current);
    Ok(())
}

fn export_cleanup_safe_intrinsic(name: &str) -> bool {
    name.starts_with("CRegistration.register_async:")
        || matches!(
            name,
            "String.new"
                | "String.with_capacity"
                | "String.len"
                | "String.capacity"
                | "String.is_empty"
                | "String.push"
                | "String.push_str"
                | "String.clear"
                | "CString.new"
                | "CString.len"
                | "CString.is_empty"
                | "CString.as_c_str"
                | "CRegistration.adopt"
                | "CRegistration.adopt_async"
                | "CRegistration.close"
                | "CRegistration.is_active"
        )
}

fn export_abort_cleanup_safe(
    program: &mir::Program,
    ty: &hir::Type,
    visiting: &mut BTreeSet<hir::Type>,
) -> bool {
    if !visiting.insert(ty.clone()) {
        return true;
    }
    let safe = match ty {
        hir::Type::String
        | hir::Type::CString
        | hir::Type::Memory
        | hir::Type::Path
        | hir::Type::Url
        | hir::Type::Json
        | hir::Type::SocketAddress
        | hir::Type::ProcessCommand
        | hir::Type::ProcessOutput
        | hir::Type::CRegistration => true,
        hir::Type::Unit
        | hir::Type::Bool
        | hir::Type::Char
        | hir::Type::Int { .. }
        | hir::Type::Float { .. }
        | hir::Type::Instant
        | hir::Type::Duration
        | hir::Type::IpAddress
        | hir::Type::Reference { .. }
        | hir::Type::RawPointer { .. }
        | hir::Type::MemoryPointer { .. }
        | hir::Type::Str
        | hir::Type::CStr
        | hir::Type::Slice(_)
        | hir::Type::CFunction(_, _) => true,
        hir::Type::Array(element, _)
        | hir::Type::List(element)
        | hir::Type::Set(element)
        | hir::Type::Option(element) => export_abort_cleanup_safe(program, element, visiting),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            export_abort_cleanup_safe(program, key, visiting)
                && export_abort_cleanup_safe(program, value, visiting)
        }
        hir::Type::Struct(id, arguments) => {
            let declaration = &program.structs[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            declaration.fields.iter().all(|field| {
                export_abort_cleanup_safe(
                    program,
                    &hir::substitute_type(&field.ty, &substitutions),
                    visiting,
                )
            })
        }
        hir::Type::Enum(id, arguments) => {
            let declaration = &program.enums[id.0];
            let substitutions = declaration
                .generic_parameters
                .iter()
                .cloned()
                .zip(arguments.iter().cloned())
                .collect::<HashMap<_, _>>();
            declaration.variants.iter().all(|variant| {
                variant.payload.iter().all(|payload| {
                    export_abort_cleanup_safe(
                        program,
                        &hir::substitute_type(payload, &substitutions),
                        visiting,
                    )
                })
            })
        }
        _ => false,
    };
    visiting.remove(ty);
    safe
}

fn error(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}
