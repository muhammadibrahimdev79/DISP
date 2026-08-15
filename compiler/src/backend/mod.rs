pub mod abi;
pub mod allocator;
pub mod codegen;
pub mod layout;
pub mod linker;
pub mod mono;
pub mod native_types;
pub mod runtime;
pub mod target;
pub mod typed_codegen;

use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    hir, mir,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    pub optimized: bool,
    pub emit_c: bool,
    pub emit_object: bool,
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
    let paths = build_paths(source_path)?;
    let fingerprint = build_fingerprint(hir, options);
    if fingerprint
        .as_deref()
        .is_some_and(|expected| cache_matches(&paths, options, expected))
    {
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
    let generated = codegen::generate(mir, &mono, &abi, &native_types)?;
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
        linker::RuntimeFeatures {
            networking,
            http,
            database,
            data,
        },
        &libraries,
    )?;
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

fn build_paths(source_path: &Path) -> Result<BuildPaths, Diagnostic> {
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
    Ok(BuildPaths {
        c: directory.join(format!("{safe_stem}.backend.c")),
        object: directory.join(format!("{safe_stem}.o")),
        fingerprint: directory.join("fingerprint.sha256"),
        executable: root.join(format!("{safe_stem}.exe")),
        directory,
    })
}

fn build_fingerprint(program: &hir::Program, options: BuildOptions) -> Option<String> {
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
    ]);
    digest.update(std::env::consts::OS.as_bytes());
    digest.update([0]);
    digest.update(std::env::consts::ARCH.as_bytes());
    digest.update([0]);

    let compiler = std::env::current_exe().ok()?;
    digest.update(fs::read(compiler).ok()?);

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
        && fs::read_to_string(&paths.fingerprint).is_ok_and(|actual| actual.trim() == expected)
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
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. } => type_uses_database(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_database(key) || type_uses_database(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_database)
        }
        hir::Type::Function(arguments, result) => {
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
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. } => type_uses_data_store(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_data_store(key) || type_uses_data_store(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_data_store)
        }
        hir::Type::Function(arguments, result) => {
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
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. } => type_uses_http(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_http(key) || type_uses_http(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_http)
        }
        hir::Type::Function(arguments, result) => {
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
        | hir::Type::Option(inner)
        | hir::Type::Reference { inner, .. }
        | hir::Type::RawPointer { inner, .. } => type_uses_networking(inner),
        hir::Type::Map(key, value) | hir::Type::Result(key, value) => {
            type_uses_networking(key) || type_uses_networking(value)
        }
        hir::Type::Struct(_, arguments) | hir::Type::Enum(_, arguments) => {
            arguments.iter().any(type_uses_networking)
        }
        hir::Type::Function(arguments, result) => {
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
fn error(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Backend, message, Span::point(1, 1))
}
