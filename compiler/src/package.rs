use crate::{
    diagnostics::{Diagnostic, DiagnosticKind, Span},
    project::{PackageManifest, parse_manifest},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

pub const LOCKFILE_NAME: &str = "DISP.lock";
pub const LOCK_VERSION: &str = "1";
pub use crate::limits::{
    MAX_DEPENDENCY_DEPTH, MAX_LOCKFILE_BYTES, MAX_PACKAGE_FILES, MAX_PACKAGE_SOURCE_BYTES,
    MAX_PACKAGES,
};

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub id: String,
    pub root: PathBuf,
    pub source_root: PathBuf,
    pub manifest: PackageManifest,
    pub dependencies: BTreeMap<String, String>,
    pub digest: String,
    pub root_package: bool,
}

#[derive(Debug, Clone)]
pub struct PackageGraph {
    pub project_root: PathBuf,
    pub root_id: String,
    pub packages: BTreeMap<String, ResolvedPackage>,
    pub manifest_digest: String,
}

#[derive(Debug, Clone)]
pub struct DependencyTreeLine {
    pub depth: usize,
    pub alias: Option<String>,
    pub id: String,
}

impl PackageGraph {
    pub fn root(&self) -> &ResolvedPackage {
        &self.packages[&self.root_id]
    }

    pub fn has_dependencies(&self) -> bool {
        self.packages.len() > 1 || !self.root().dependencies.is_empty()
    }

    pub fn package(&self, id: &str) -> &ResolvedPackage {
        &self.packages[id]
    }

    pub fn tree(&self) -> Vec<DependencyTreeLine> {
        fn walk(
            graph: &PackageGraph,
            id: &str,
            alias: Option<String>,
            depth: usize,
            expanded: &mut HashSet<String>,
            output: &mut Vec<DependencyTreeLine>,
        ) {
            output.push(DependencyTreeLine {
                depth,
                alias,
                id: id.to_owned(),
            });
            if !expanded.insert(id.to_owned()) {
                return;
            }
            for (dependency_alias, dependency_id) in &graph.package(id).dependencies {
                walk(
                    graph,
                    dependency_id,
                    Some(dependency_alias.clone()),
                    depth + 1,
                    expanded,
                    output,
                );
            }
        }
        let mut output = Vec::new();
        walk(
            self,
            &self.root_id,
            None,
            0,
            &mut HashSet::new(),
            &mut output,
        );
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Visiting,
    Complete,
}

pub fn resolve(project_root: &Path) -> Result<PackageGraph, Diagnostic> {
    let project_root = fs::canonicalize(project_root).map_err(|cause| {
        package_error(
            project_root.join("DISP.toml"),
            format!("could not resolve package root: {cause}"),
        )
    })?;
    let manifest_path = project_root.join("DISP.toml");
    let manifest_bytes = fs::read(&manifest_path).map_err(|cause| {
        package_error(
            manifest_path.clone(),
            format!("could not read root package manifest: {cause}"),
        )
    })?;
    let manifest_digest = sha256_text(&manifest_bytes).map_err(|message| {
        package_error(
            manifest_path.clone(),
            format!("could not hash manifest: {message}"),
        )
    })?;
    let mut resolver = Resolver {
        packages: BTreeMap::new(),
        paths: HashMap::new(),
        identities: HashMap::new(),
        visits: HashMap::new(),
        stack: Vec::new(),
    };
    let root_id = resolver.visit(&project_root, true, &manifest_path, 1)?;
    Ok(PackageGraph {
        project_root,
        root_id,
        packages: resolver.packages,
        manifest_digest,
    })
}

pub fn verify(project_root: &Path) -> Result<PackageGraph, Diagnostic> {
    let graph = resolve(project_root)?;
    let lock_path = graph.project_root.join(LOCKFILE_NAME);
    if !graph.has_dependencies() && !lock_path.exists() {
        return Ok(graph);
    }
    let metadata = fs::metadata(&lock_path).map_err(|cause| {
        lock_error(
            &lock_path,
            format!("package dependencies require `{LOCKFILE_NAME}`: {cause}"),
        )
        .with_help("run `disp lock <project-directory>` and commit the generated lockfile")
    })?;
    if metadata.len() > MAX_LOCKFILE_BYTES as u64 {
        return Err(lock_error(
            &lock_path,
            format!("lockfile exceeds the {MAX_LOCKFILE_BYTES}-byte safety limit"),
        ));
    }
    let actual = fs::read_to_string(&lock_path)
        .map_err(|cause| lock_error(&lock_path, format!("lockfile is not valid UTF-8: {cause}")))?;
    let expected = render(&graph)?;
    if actual != expected {
        return Err(lock_error(
            &lock_path,
            "lockfile does not match the package manifests or dependency contents",
        )
        .with_help(
            "review the manifest/source changes, then run `disp lock <project-directory>` explicitly",
        ));
    }
    Ok(graph)
}

pub fn write_lock(project_root: &Path) -> Result<PathBuf, Diagnostic> {
    let graph = resolve(project_root)?;
    let contents = render(&graph)?;
    let lock_path = graph.project_root.join(LOCKFILE_NAME);
    transactional_write(&lock_path, contents.as_bytes())?;
    Ok(lock_path)
}

pub fn render(graph: &PackageGraph) -> Result<String, Diagnostic> {
    let mut output = String::new();
    output.push_str("# Generated by DISP. Do not edit by hand.\n");
    output.push_str(&format!("lock-version = \"{LOCK_VERSION}\"\n"));
    output.push_str(&format!("root = \"{}\"\n", graph.root_id));
    output.push_str(&format!(
        "manifest-sha256 = \"{}\"\n",
        graph.manifest_digest
    ));
    for package in graph.packages.values() {
        output.push_str("\n[[package]]\n");
        output.push_str(&format!("id = \"{}\"\n", package.id));
        output.push_str(&format!("name = \"{}\"\n", package.manifest.name));
        output.push_str(&format!("version = \"{}\"\n", package.manifest.version));
        let source = relative_source(&graph.project_root, &package.root)?;
        output.push_str(&format!("source = \"{source}\"\n"));
        output.push_str(&format!("sha256 = \"{}\"\n", package.digest));
        output.push_str("dependencies = [");
        for (index, (alias, id)) in package.dependencies.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            output.push('"');
            output.push_str(alias);
            output.push('=');
            output.push_str(id);
            output.push('"');
        }
        output.push_str("]\n");
    }
    Ok(output)
}

struct Resolver {
    packages: BTreeMap<String, ResolvedPackage>,
    paths: HashMap<PathBuf, String>,
    identities: HashMap<String, PathBuf>,
    visits: HashMap<PathBuf, Visit>,
    stack: Vec<PathBuf>,
}

impl Resolver {
    fn visit(
        &mut self,
        root: &Path,
        root_package: bool,
        requested_from: &Path,
        requested_line: usize,
    ) -> Result<String, Diagnostic> {
        let root = fs::canonicalize(root).map_err(|cause| {
            Diagnostic::new(
                DiagnosticKind::Resolve,
                format!(
                    "could not resolve local dependency `{}`: {cause}",
                    root.display()
                ),
                Span::point(requested_line, 1),
            )
            .with_file(requested_from.display().to_string())
        })?;
        match self.visits.get(&root) {
            Some(Visit::Complete) => return Ok(self.paths[&root].clone()),
            Some(Visit::Visiting) => {
                let start = self
                    .stack
                    .iter()
                    .position(|candidate| candidate == &root)
                    .unwrap_or(0);
                let mut names = self.stack[start..]
                    .iter()
                    .map(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("<package>")
                            .to_owned()
                    })
                    .collect::<Vec<_>>();
                names.push(
                    root.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("<package>")
                        .to_owned(),
                );
                return Err(Diagnostic::new(
                    DiagnosticKind::Resolve,
                    format!("package dependency cycle: {}", names.join(" -> ")),
                    Span::point(requested_line, 1),
                )
                .with_file(requested_from.display().to_string()));
            }
            None => {}
        }
        if self.visits.len() >= MAX_PACKAGES {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!("package graph exceeds the safety limit of {MAX_PACKAGES} packages"),
                Span::point(requested_line, 1),
            )
            .with_file(requested_from.display().to_string()));
        }
        if self.stack.len() >= MAX_DEPENDENCY_DEPTH {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!("package graph exceeds the maximum depth of {MAX_DEPENDENCY_DEPTH}"),
                Span::point(requested_line, 1),
            )
            .with_file(requested_from.display().to_string()));
        }
        let manifest_path = root.join("DISP.toml");
        let manifest = parse_manifest(&manifest_path)?;
        let id = format!("{}@{}", manifest.name, manifest.version);
        if let Some(previous) = self.identities.get(&id)
            && previous != &root
        {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                format!(
                    "package identity `{id}` resolves to both `{}` and `{}`",
                    previous.display(),
                    root.display()
                ),
                Span::point(requested_line, 1),
            )
            .with_file(requested_from.display().to_string())
            .with_help("package name and version together must identify exactly one source tree"));
        }
        let entry = root.join(&manifest.entry);
        let entry = fs::canonicalize(&entry).map_err(|cause| {
            package_error(
                manifest_path.clone(),
                format!(
                    "could not resolve package entry `{}`: {cause}",
                    entry.display()
                ),
            )
        })?;
        if !entry.starts_with(&root) {
            return Err(package_error(
                manifest_path.clone(),
                "package entry resolves outside its package root",
            ));
        }
        let source_root = entry
            .parent()
            .ok_or_else(|| {
                package_error(manifest_path.clone(), "package entry has no source root")
            })?
            .to_path_buf();

        self.visits.insert(root.clone(), Visit::Visiting);
        self.stack.push(root.clone());
        self.identities.insert(id.clone(), root.clone());
        self.paths.insert(root.clone(), id.clone());
        let mut dependencies = BTreeMap::new();
        for (alias, specification) in &manifest.dependencies {
            let dependency_root = root.join(&specification.path);
            let dependency_id =
                self.visit(&dependency_root, false, &manifest_path, specification.line)?;
            dependencies.insert(alias.clone(), dependency_id);
        }
        self.stack.pop();
        self.visits.insert(root.clone(), Visit::Complete);
        let digest = if root_package {
            sha256_text(&fs::read(&manifest_path).map_err(|cause| {
                package_error(
                    manifest_path.clone(),
                    format!("could not hash package manifest: {cause}"),
                )
            })?)
            .map_err(|message| package_error(manifest_path.clone(), message))?
        } else {
            hash_package(&root)?
        };
        self.packages.insert(
            id.clone(),
            ResolvedPackage {
                id: id.clone(),
                root,
                source_root,
                manifest,
                dependencies,
                digest,
                root_package,
            },
        );
        Ok(id)
    }
}

fn hash_package(root: &Path) -> Result<String, Diagnostic> {
    let mut files = Vec::<PathBuf>::new();
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0usize;
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|cause| {
                package_error(
                    root.join("DISP.toml"),
                    format!("could not inspect package source: {cause}"),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|cause| {
                package_error(
                    root.join("DISP.toml"),
                    format!("could not inspect package source: {cause}"),
                )
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name();
            if name.to_str().is_none() {
                return Err(package_error(
                    root.join("DISP.toml"),
                    format!(
                        "package source path is not valid UTF-8: `{}`",
                        path.display()
                    ),
                ));
            }
            let metadata = fs::symlink_metadata(&path).map_err(|cause| {
                package_error(
                    root.join("DISP.toml"),
                    format!("could not inspect `{}`: {cause}", path.display()),
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(package_error(
                    root.join("DISP.toml"),
                    format!(
                        "package source may not contain symbolic link `{}`",
                        path.display()
                    ),
                ));
            }
            if metadata.is_dir() {
                if name != "build" && name != ".git" && name != "target" {
                    pending.push(path);
                }
                continue;
            }
            if metadata.is_file()
                && (entry.file_name() == "DISP.toml"
                    || path.extension().and_then(|extension| extension.to_str()) == Some("disp"))
            {
                bytes = bytes.checked_add(metadata.len() as usize).ok_or_else(|| {
                    package_error(root.join("DISP.toml"), "package source size overflow")
                })?;
                if bytes > MAX_PACKAGE_SOURCE_BYTES {
                    return Err(package_error(
                        root.join("DISP.toml"),
                        format!(
                            "package source exceeds the {MAX_PACKAGE_SOURCE_BYTES}-byte safety limit"
                        ),
                    ));
                }
                files.push(path);
                if files.len() > MAX_PACKAGE_FILES {
                    return Err(package_error(
                        root.join("DISP.toml"),
                        format!(
                            "package exceeds the safety limit of {MAX_PACKAGE_FILES} source files"
                        ),
                    ));
                }
            }
        }
    }
    files.sort_by_key(|path| normalized_relative(root, path));
    let mut digest = Sha256::new();
    digest.update(b"DISP-PACKAGE-SHA256\0");
    for path in files {
        let relative = normalized_relative(root, &path);
        let content = fs::read(&path).map_err(|cause| {
            package_error(
                root.join("DISP.toml"),
                format!("could not hash `{}`: {cause}", path.display()),
            )
        })?;
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative.as_bytes());
        let content = canonical_text(&content).map_err(|message| {
            package_error(
                root.join("DISP.toml"),
                format!("could not hash `{}`: {message}", path.display()),
            )
        })?;
        digest.update((content.len() as u64).to_le_bytes());
        digest.update(content.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn normalized_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn relative_source(project_root: &Path, package_root: &Path) -> Result<String, Diagnostic> {
    if project_root == package_root {
        return Ok(".".to_owned());
    }
    let from = project_root.components().collect::<Vec<_>>();
    let to = package_root.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(package_error(
            project_root.join("DISP.toml"),
            "local dependencies must be on the same filesystem volume as the project",
        ));
    }
    let mut parts = vec![".."; from.len() - common]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for component in &to[common..] {
        if let Component::Normal(part) = component {
            let part = part.to_str().ok_or_else(|| {
                package_error(
                    project_root.join("DISP.toml"),
                    "local dependency paths must be valid UTF-8",
                )
            })?;
            if part.contains(['"', '\\']) || part.chars().any(char::is_control) {
                return Err(package_error(
                    project_root.join("DISP.toml"),
                    "local dependency paths contain unsupported characters",
                ));
            }
            parts.push(part.to_owned());
        }
    }
    Ok(parts.join("/"))
}

fn transactional_write(path: &Path, bytes: &[u8]) -> Result<(), Diagnostic> {
    let parent = path
        .parent()
        .ok_or_else(|| lock_error(path, "lockfile has no parent directory"))?;
    let temporary = parent.join(format!(".{}.{}.tmp", LOCKFILE_NAME, std::process::id()));
    let backup = parent.join(format!(".{}.{}.backup", LOCKFILE_NAME, std::process::id()));
    let result = (|| -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            fs::rename(path, &backup)?;
        }
        if let Err(cause) = fs::rename(&temporary, path) {
            if backup.exists() {
                let _ = fs::rename(&backup, path);
            }
            return Err(cause);
        }
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|cause| lock_error(path, format!("could not update lockfile safely: {cause}")))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_text(bytes: &[u8]) -> Result<String, &'static str> {
    Ok(sha256(canonical_text(bytes)?.as_bytes()))
}

fn canonical_text(bytes: &[u8]) -> Result<String, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "source is not valid UTF-8")?;
    Ok(text.replace("\r\n", "\n"))
}

fn package_error(path: PathBuf, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Resolve, message, Span::point(1, 1))
        .with_file(path.display().to_string())
}

fn lock_error(path: &Path, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Resolve, message, Span::point(1, 1))
        .with_file(path.display().to_string())
}
