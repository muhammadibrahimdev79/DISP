use crate::{
    MAX_SOURCE_BYTES,
    ast::{self, Block, Expr, Expression, GenericParameter, Pattern, Program, Statement, TypeName},
    diagnostics::{Diagnostic, DiagnosticKind, SourceFile, SourceMap, Span},
    lexer::Lexer,
    package::{self, PackageGraph},
    parser::Parser,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

pub use crate::limits::{
    MAX_MANIFEST_BYTES, MAX_MODULE_DEPTH, MAX_PROJECT_BYTES, MAX_PROJECT_MODULES,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub features: BTreeSet<String>,
    pub entry: PathBuf,
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub program: Program,
    pub sources: SourceMap,
    pub entry: PathBuf,
    pub package: Option<PackageManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub manifest: PathBuf,
    pub edition: String,
    pub changed: bool,
}

pub fn load(input: &Path) -> Result<Project, Diagnostic> {
    let input_metadata = fs::metadata(input).map_err(|cause| {
        Diagnostic::new(
            DiagnosticKind::Resolve,
            format!("could not inspect `{}`: {cause}", input.display()),
            Span::point(1, 1),
        )
        .with_file(input.display().to_string())
    })?;
    let graph = input_metadata
        .is_dir()
        .then(|| package::verify(input))
        .transpose()?;
    let (entry, package) = if let Some(graph) = &graph {
        let root = graph.root();
        (
            root.root.join(&root.manifest.entry),
            Some(root.manifest.clone()),
        )
    } else {
        resolve_input(input)?
    };
    let entry = fs::canonicalize(&entry).map_err(|cause| {
        Diagnostic::new(
            DiagnosticKind::Resolve,
            format!(
                "could not open project entry `{}`: {cause}",
                entry.display()
            ),
            Span::point(1, 1),
        )
        .with_file(entry.display().to_string())
    })?;
    let root = entry
        .parent()
        .ok_or_else(|| project_error("project entry has no parent directory", Span::point(1, 1)))?
        .to_path_buf();
    let mut loader = if let Some(graph) = graph {
        Loader::for_packages(graph, entry.clone())
    } else {
        Loader::new(root, entry.clone())
    };
    match loader.load_all() {
        Ok(program) => Ok(Project {
            program,
            sources: loader.sources,
            entry,
            package,
        }),
        Err(error) => Err(loader.sources.remap(error)),
    }
}

pub fn resolve_input(input: &Path) -> Result<(PathBuf, Option<PackageManifest>), Diagnostic> {
    let metadata = fs::metadata(input).map_err(|cause| {
        Diagnostic::new(
            DiagnosticKind::Resolve,
            format!("could not inspect `{}`: {cause}", input.display()),
            Span::point(1, 1),
        )
        .with_file(input.display().to_string())
    })?;
    if metadata.is_file() {
        if input.extension().and_then(|extension| extension.to_str()) != Some("disp") {
            return Err(Diagnostic::new(
                DiagnosticKind::Resolve,
                "DISP source files must end with `.disp`",
                Span::point(1, 1),
            )
            .with_file(input.display().to_string()));
        }
        return Ok((input.to_path_buf(), None));
    }
    if !metadata.is_dir() {
        return Err(Diagnostic::new(
            DiagnosticKind::Resolve,
            "DISP input must be a source file or project directory",
            Span::point(1, 1),
        )
        .with_file(input.display().to_string()));
    }
    let root = fs::canonicalize(input).map_err(|cause| {
        Diagnostic::new(
            DiagnosticKind::Resolve,
            format!("could not resolve project directory: {cause}"),
            Span::point(1, 1),
        )
        .with_file(input.display().to_string())
    })?;
    let manifest_path = root.join("DISP.toml");
    let manifest = parse_manifest(&manifest_path)?;
    let entry = root.join(&manifest.entry);
    let canonical_entry = fs::canonicalize(&entry).map_err(|cause| {
        Diagnostic::new(
            DiagnosticKind::Resolve,
            format!(
                "could not open package entry `{}`: {cause}",
                entry.display()
            ),
            Span::point(1, 1),
        )
        .with_file(manifest_path.display().to_string())
    })?;
    if !canonical_entry.starts_with(&root) {
        return Err(Diagnostic::new(
            DiagnosticKind::Resolve,
            "package entry resolves outside the project directory",
            Span::point(1, 1),
        )
        .with_file(manifest_path.display().to_string()));
    }
    Ok((canonical_entry, Some(manifest)))
}

pub fn create(path: &Path) -> Result<(), Diagnostic> {
    if path.exists() {
        return Err(Diagnostic::new(
            DiagnosticKind::Resolve,
            format!("refusing to overwrite existing path `{}`", path.display()),
            Span::point(1, 1),
        ));
    }
    let name = path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| {
            project_error("project path has no valid package name", Span::point(1, 1))
        })?;
    validate_package_name(name, Span::point(1, 1))?;
    let source_directory = path.join("src");
    fs::create_dir_all(&source_directory).map_err(|cause| {
        project_error(
            format!("could not create project `{}`: {cause}", path.display()),
            Span::point(1, 1),
        )
    })?;
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"1\"\nfeatures = []\nentry = \"src/main.disp\"\n"
    );
    let write_result = fs::write(path.join("DISP.toml"), manifest)
        .and_then(|_| {
            fs::write(
                source_directory.join("main.disp"),
                "fn main() {\n    print(\"Hello from DISP\")\n}\n",
            )
        })
        .and_then(|_| {
            fs::write(
                path.join(".gitattributes"),
                "*.disp text eol=lf\nDISP.toml text eol=lf\nDISP.lock text eol=lf\n",
            )
        });
    if let Err(cause) = write_result {
        let _ = fs::remove_dir_all(path);
        return Err(project_error(
            format!("could not initialize project: {cause}"),
            Span::point(1, 1),
        ));
    }
    Ok(())
}

pub fn migrate(path: &Path, check: bool) -> Result<MigrationReport, Diagnostic> {
    let metadata = fs::metadata(path).map_err(|cause| {
        project_error(
            format!("could not inspect project `{}`: {cause}", path.display()),
            Span::point(1, 1),
        )
        .with_file(path.display().to_string())
    })?;
    if !metadata.is_dir() {
        return Err(project_error(
            "`disp migrate` requires a project directory",
            Span::point(1, 1),
        )
        .with_file(path.display().to_string()));
    }
    let manifest_path = path.join("DISP.toml");
    let manifest = parse_manifest(&manifest_path)?;
    let source = fs::read_to_string(&manifest_path).map_err(|cause| {
        manifest_error(
            &manifest_path,
            1,
            format!("could not read package manifest for migration: {cause}"),
        )
    })?;
    let mut section = None::<&str>;
    let mut explicit_edition = false;
    let mut explicit_features = false;
    let mut version_line = None;
    let mut edition_line = None;
    let lines = source.lines().collect::<Vec<_>>();
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = Some(line[1..line.len() - 1].trim());
            continue;
        }
        if section == Some("package")
            && let Some((key, _)) = line.split_once('=')
        {
            match key.trim() {
                "edition" => {
                    explicit_edition = true;
                    edition_line = Some(index);
                }
                "features" => explicit_features = true,
                "version" => version_line = Some(index),
                _ => {}
            }
        }
    }
    if explicit_edition && explicit_features {
        return Ok(MigrationReport {
            manifest: manifest_path,
            edition: manifest.edition,
            changed: false,
        });
    }
    let version_line = version_line.expect("validated manifests always contain a version");
    let mut migrated = lines
        .iter()
        .map(|line| (*line).to_owned())
        .collect::<Vec<_>>();
    if !explicit_edition {
        migrated.insert(
            version_line + 1,
            format!("edition = \"{}\"", manifest.edition),
        );
        edition_line = Some(version_line + 1);
    }
    if !explicit_features {
        let edition_line = edition_line.expect("migration always has an explicit edition");
        migrated.insert(edition_line + 1, "features = []".to_owned());
    }
    let migrated = migrated.join("\n") + "\n";
    if !check {
        fs::write(&manifest_path, migrated).map_err(|cause| {
            manifest_error(
                &manifest_path,
                version_line + 2,
                format!("could not write migrated package manifest: {cause}"),
            )
        })?;
    }
    Ok(MigrationReport {
        manifest: manifest_path,
        edition: manifest.edition,
        changed: true,
    })
}

pub(crate) fn parse_manifest(path: &Path) -> Result<PackageManifest, Diagnostic> {
    let metadata = fs::metadata(path).map_err(|cause| {
        manifest_error(
            path,
            1,
            format!("could not open required package manifest `DISP.toml`: {cause}"),
        )
    })?;
    if metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(manifest_error(
            path,
            1,
            format!("package manifest exceeds the {MAX_MANIFEST_BYTES}-byte safety limit"),
        ));
    }
    let source = fs::read_to_string(path).map_err(|cause| {
        manifest_error(path, 1, format!("manifest is not valid UTF-8: {cause}"))
    })?;
    let mut section = None::<String>;
    let mut saw_package_section = false;
    let mut saw_dependencies_section = false;
    let mut values = BTreeMap::<String, (String, usize)>::new();
    let mut features = None::<(BTreeSet<String>, usize)>;
    let mut dependencies = BTreeMap::<String, DependencySpec>::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']')
                || line.matches('[').count() != 1
                || line.matches(']').count() != 1
            {
                return Err(manifest_error(
                    path,
                    line_number,
                    "malformed manifest section",
                ));
            }
            let name = line[1..line.len() - 1].trim();
            if !matches!(name, "package" | "dependencies") {
                if is_extension_manifest_name(name) {
                    return Err(extension_manifest_error(path, line_number, name));
                }
                return Err(manifest_error(
                    path,
                    line_number,
                    format!("unsupported manifest section `[{name}]`"),
                ));
            }
            if name == "package" && saw_package_section {
                return Err(manifest_error(
                    path,
                    line_number,
                    "duplicate `[package]` section",
                ));
            }
            if name == "dependencies" && saw_dependencies_section {
                return Err(manifest_error(
                    path,
                    line_number,
                    "duplicate `[dependencies]` section",
                ));
            }
            saw_package_section |= name == "package";
            saw_dependencies_section |= name == "dependencies";
            section = Some(name.to_owned());
            continue;
        }
        if section.is_none() {
            return Err(manifest_error(
                path,
                line_number,
                "manifest values must appear under `[package]`",
            ));
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            return Err(manifest_error(
                path,
                line_number,
                "expected `key = \"value\"`",
            ));
        };
        let key = key.trim();
        if section.as_deref() == Some("dependencies") {
            validate_dependency_alias(key, Span::point(line_number, 1))
                .map_err(|error| error.with_file(path.display().to_string()))?;
            if dependencies.contains_key(key) {
                return Err(manifest_error(
                    path,
                    line_number,
                    format!("duplicate dependency `{key}`"),
                ));
            }
            let dependency_path = parse_path_dependency(raw_value.trim()).ok_or_else(|| {
                manifest_error(
                    path,
                    line_number,
                    "a local dependency must use `{ path = \"relative/path\" }`",
                )
            })?;
            dependencies.insert(
                key.to_owned(),
                DependencySpec {
                    path: dependency_path,
                    line: line_number,
                },
            );
            continue;
        }
        if section.as_deref() != Some("package") {
            return Err(manifest_error(
                path,
                line_number,
                "manifest values must appear under `[package]` or `[dependencies]`",
            ));
        }
        if !matches!(key, "name" | "version" | "edition" | "features" | "entry") {
            if is_extension_manifest_name(key) {
                return Err(extension_manifest_error(path, line_number, key));
            }
            return Err(manifest_error(
                path,
                line_number,
                format!("unknown package field `{key}`"),
            ));
        }
        if values.contains_key(key) || (key == "features" && features.is_some()) {
            return Err(manifest_error(
                path,
                line_number,
                format!("duplicate package field `{key}`"),
            ));
        }
        if key == "features" {
            let parsed = parse_manifest_features(raw_value.trim()).ok_or_else(|| {
                manifest_error(
                    path,
                    line_number,
                    "package features must be a bounded array of unique quoted names",
                )
            })?;
            features = Some((parsed, line_number));
            continue;
        }
        let value = parse_manifest_string(raw_value.trim()).ok_or_else(|| {
            manifest_error(
                path,
                line_number,
                "manifest values must be quoted strings without escapes",
            )
        })?;
        values.insert(key.to_owned(), (value, line_number));
    }
    let (name, name_line) = required_manifest_value(path, &values, "name")?;
    validate_package_name(&name, Span::point(name_line, 1))
        .map_err(|error| error.with_file(path.display().to_string()))?;
    let (version, version_line) = required_manifest_value(path, &values, "version")?;
    validate_version(&version).map_err(|message| manifest_error(path, version_line, message))?;
    let edition = values
        .get("edition")
        .map(|(value, _)| value.clone())
        .unwrap_or_else(|| "1".to_owned());
    if edition != "1" {
        return Err(manifest_error(
            path,
            values.get("edition").map_or(1, |(_, line)| *line),
            format!("unsupported DISP edition `{edition}`; this compiler supports edition `1`"),
        ));
    }
    let (features, features_line) = features.unwrap_or_else(|| (BTreeSet::new(), 1));
    if let Some(feature) = features.first() {
        return Err(manifest_error(
            path,
            features_line,
            format!(
                "unsupported DISP feature `{feature}`; edition `1` currently has no unstable opt-in features"
            ),
        ));
    }
    let entry = values
        .get("entry")
        .map(|(value, _)| value.as_str())
        .unwrap_or("src/main.disp");
    let entry_path = PathBuf::from(entry);
    if entry_path.is_absolute()
        || entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("disp")
        || entry_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(manifest_error(
            path,
            values.get("entry").map_or(1, |(_, line)| *line),
            "package entry must be a relative `.disp` path without `.` or `..` components",
        ));
    }
    Ok(PackageManifest {
        name,
        version,
        edition,
        features,
        entry: entry_path,
        dependencies,
    })
}

fn parse_manifest_features(value: &str) -> Option<BTreeSet<String>> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return Some(BTreeSet::new());
    }
    let mut features = BTreeSet::new();
    for raw in inner.split(',') {
        if features.len() >= 64 {
            return None;
        }
        let feature = parse_manifest_string(raw.trim())?;
        if feature.is_empty()
            || feature.len() > 64
            || !feature.as_bytes()[0].is_ascii_lowercase()
            || !feature
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || !features.insert(feature)
        {
            return None;
        }
    }
    Some(features)
}

fn parse_path_dependency(value: &str) -> Option<PathBuf> {
    let inner = value.strip_prefix('{')?.strip_suffix('}')?.trim();
    let (key, value) = inner.split_once('=')?;
    if key.trim() != "path" {
        return None;
    }
    let value = parse_manifest_string(value.trim())?;
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path.as_os_str().is_empty()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        })
    {
        return None;
    }
    Some(path)
}

fn parse_manifest_string(value: &str) -> Option<String> {
    let rest = value.strip_prefix('"')?;
    let end = rest.find('"')?;
    let parsed = &rest[..end];
    if parsed.contains('\\') || parsed.chars().any(char::is_control) {
        return None;
    }
    let trailing = rest[end + 1..].trim();
    if !trailing.is_empty() && !trailing.starts_with('#') {
        return None;
    }
    Some(parsed.to_owned())
}

fn required_manifest_value(
    path: &Path,
    values: &BTreeMap<String, (String, usize)>,
    key: &str,
) -> Result<(String, usize), Diagnostic> {
    values
        .get(key)
        .cloned()
        .ok_or_else(|| manifest_error(path, 1, format!("missing required package field `{key}`")))
}

fn validate_package_name(name: &str, span: Span) -> Result<(), Diagnostic> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(project_error(
            "package names must be 1-64 lowercase ASCII letters, digits, `-`, or `_`",
            span,
        ));
    }
    Ok(())
}

fn validate_dependency_alias(name: &str, span: Span) -> Result<(), Diagnostic> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(project_error(
            "dependency aliases must be valid lowercase DISP identifiers",
            span,
        ));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), &'static str> {
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
                || part.parse::<u64>().is_err()
        })
    {
        return Err("package version must use `MAJOR.MINOR.PATCH` with numeric components");
    }
    Ok(())
}

fn manifest_error(path: &Path, line: usize, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Resolve, message, Span::point(line, 1))
        .with_file(path.display().to_string())
}

fn is_extension_manifest_name(name: &str) -> bool {
    matches!(
        name,
        "build"
            | "build-script"
            | "build-scripts"
            | "macro"
            | "macros"
            | "plugin"
            | "plugins"
            | "proc-macro"
            | "procedural-macro"
            | "procedural-macros"
    )
}

fn extension_manifest_error(path: &Path, line: usize, name: &str) -> Diagnostic {
    manifest_error(
        path,
        line,
        format!("compiler extension `{name}` is disabled in DISP edition 1"),
    )
    .with_help(
        "build scripts, procedural macros, and compiler plugins never execute in the compiler process; use ordinary DISP source or an explicitly sandboxed out-of-process tool",
    )
}

#[derive(Debug, Clone)]
struct Unit {
    path: Vec<String>,
    import_targets: Vec<String>,
    program: Program,
    root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Visiting,
    Complete,
}

struct Loader {
    root: PathBuf,
    entry: PathBuf,
    graph: Option<PackageGraph>,
    units: BTreeMap<String, Unit>,
    visits: HashMap<String, Visit>,
    stack: Vec<String>,
    order: Vec<String>,
    files: HashMap<PathBuf, String>,
    next_line: usize,
    total_bytes: usize,
    sources: SourceMap,
}

impl Loader {
    fn new(root: PathBuf, entry: PathBuf) -> Self {
        Self {
            root,
            entry,
            graph: None,
            units: BTreeMap::new(),
            visits: HashMap::new(),
            stack: Vec::new(),
            order: Vec::new(),
            files: HashMap::new(),
            next_line: 1,
            total_bytes: 0,
            sources: SourceMap::default(),
        }
    }

    fn for_packages(graph: PackageGraph, entry: PathBuf) -> Self {
        let root = graph.root().source_root.clone();
        Self {
            root,
            entry,
            graph: Some(graph),
            units: BTreeMap::new(),
            visits: HashMap::new(),
            stack: Vec::new(),
            order: Vec::new(),
            files: HashMap::new(),
            next_line: 1,
            total_bytes: 0,
            sources: SourceMap::default(),
        }
    }

    fn load_all(&mut self) -> Result<Program, Diagnostic> {
        let package_id = self.graph.as_ref().map(|graph| graph.root_id.clone());
        self.load_unit(
            package_id,
            vec![],
            self.entry.clone(),
            Span::point(1, 1),
            true,
        )?;
        let mut program = link(std::mem::take(&mut self.units), &self.order)?;
        program.source_files = self.sources.files.clone();
        Ok(program)
    }

    fn load_unit(
        &mut self,
        package_id: Option<String>,
        local_path: Vec<String>,
        file: PathBuf,
        import_span: Span,
        root: bool,
    ) -> Result<(), Diagnostic> {
        let path = self.identity_path(package_id.as_deref(), &local_path, root);
        let key = module_key(&path);
        match self.visits.get(&key) {
            Some(Visit::Complete) => return Ok(()),
            Some(Visit::Visiting) => {
                let start = self
                    .stack
                    .iter()
                    .position(|candidate| candidate == &key)
                    .unwrap_or(0);
                let mut cycle = self.stack[start..].to_vec();
                cycle.push(key.clone());
                return Err(project_error(
                    format!("module import cycle: {}", cycle.join(" -> ")),
                    import_span,
                )
                .with_help("move shared declarations into a module outside the cycle"));
            }
            None => {}
        }
        if self.visits.len() >= MAX_PROJECT_MODULES {
            return Err(project_error(
                format!("project exceeds the safety limit of {MAX_PROJECT_MODULES} modules"),
                import_span,
            ));
        }
        if self.stack.len() >= MAX_MODULE_DEPTH {
            return Err(project_error(
                format!("module imports exceed the maximum depth of {MAX_MODULE_DEPTH}"),
                import_span,
            ));
        }

        let source_root = self.source_root(package_id.as_deref()).to_path_buf();
        let canonical = fs::canonicalize(&file).map_err(|cause| {
            project_error(
                format!(
                    "could not load module `{}`: {cause}",
                    display_module(&local_path)
                ),
                import_span,
            )
        })?;
        if !canonical.starts_with(&source_root) {
            return Err(project_error(
                format!(
                    "module `{}` resolves outside the project source root",
                    display_module(&local_path)
                ),
                import_span,
            ));
        }
        if let Some(previous) = self.files.get(&canonical)
            && previous != &key
        {
            return Err(project_error(
                format!("module paths `{previous}` and `{key}` resolve to the same source file"),
                import_span,
            ));
        }

        let metadata = fs::metadata(&canonical).map_err(|cause| {
            project_error(
                format!(
                    "could not inspect module `{}`: {cause}",
                    canonical.display()
                ),
                import_span,
            )
        })?;
        if metadata.len() > MAX_SOURCE_BYTES as u64 {
            return Err(project_error(
                format!(
                    "module `{}` is {} bytes; the per-file safety limit is {MAX_SOURCE_BYTES} bytes",
                    canonical.display(),
                    metadata.len()
                ),
                import_span,
            ));
        }
        self.total_bytes = self
            .total_bytes
            .checked_add(metadata.len() as usize)
            .ok_or_else(|| project_error("project source size overflow", import_span))?;
        if self.total_bytes > MAX_PROJECT_BYTES {
            return Err(project_error(
                format!("project exceeds the source safety limit of {MAX_PROJECT_BYTES} bytes"),
                import_span,
            ));
        }
        let source = fs::read_to_string(&canonical).map_err(|cause| {
            project_error(
                format!("could not read `{}` as UTF-8: {cause}", canonical.display()),
                import_span,
            )
        })?;
        let start_line = self.next_line;
        let line_count = source.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let end_line = start_line + line_count;
        self.next_line = end_line + 2;
        self.sources.files.push(SourceFile {
            path: self.diagnostic_path(package_id.as_deref(), &canonical),
            identity_path: canonical.clone(),
            start_line,
            end_line,
        });
        let tokens = Lexer::with_start_line(&source, start_line).tokenize()?;
        let program = Parser::new(tokens).parse()?;
        let expected = if local_path.is_empty() {
            vec![
                canonical
                    .file_stem()
                    .and_then(|part| part.to_str())
                    .unwrap_or("main")
                    .to_owned(),
            ]
        } else {
            local_path.clone()
        };
        if let Some(declared) = &program.module {
            let actual = declared
                .path
                .iter()
                .map(|part| part.node.clone())
                .collect::<Vec<_>>();
            if actual != expected {
                return Err(project_error(
                    format!(
                        "module declaration `{}` does not match source path `{}`",
                        actual.join("."),
                        expected.join(".")
                    ),
                    declared.span,
                ));
            }
        }

        self.files.insert(canonical.clone(), key.clone());
        self.visits.insert(key.clone(), Visit::Visiting);
        self.stack.push(key.clone());
        let imports = program.imports.clone();
        let targets = imports
            .iter()
            .map(|import| {
                let import_path = import
                    .path
                    .iter()
                    .map(|part| part.node.clone())
                    .collect::<Vec<_>>();
                self.resolve_import(package_id.as_deref(), &import_path, import.span)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let import_targets = targets
            .iter()
            .map(|(target_package, target_path, _, target_root)| {
                module_key(&self.identity_path(
                    target_package.as_deref(),
                    target_path,
                    *target_root,
                ))
            })
            .collect::<Vec<_>>();
        self.units.insert(
            key.clone(),
            Unit {
                path,
                import_targets,
                program,
                root,
            },
        );
        for (import, (target_package, target_path, module_file, target_root)) in
            imports.into_iter().zip(targets)
        {
            self.load_unit(
                target_package,
                target_path,
                module_file,
                import.span,
                target_root,
            )?;
        }
        self.stack.pop();
        self.visits.insert(key.clone(), Visit::Complete);
        self.order.push(key);
        Ok(())
    }

    fn identity_path(
        &self,
        package_id: Option<&str>,
        local_path: &[String],
        root: bool,
    ) -> Vec<String> {
        let Some(graph) = &self.graph else {
            return local_path.to_vec();
        };
        if root || package_id == Some(graph.root_id.as_str()) {
            return local_path.to_vec();
        }
        let mut identity = vec![format!(
            "package:{package_id}",
            package_id = package_id.unwrap()
        )];
        identity.extend_from_slice(local_path);
        identity
    }

    fn source_root(&self, package_id: Option<&str>) -> &Path {
        match (&self.graph, package_id) {
            (Some(graph), Some(id)) => &graph.package(id).source_root,
            _ => &self.root,
        }
    }

    fn diagnostic_path(&self, package_id: Option<&str>, canonical: &Path) -> PathBuf {
        let source_root = self.source_root(package_id);
        let relative = canonical.strip_prefix(source_root).unwrap_or(canonical);
        let Some(graph) = &self.graph else {
            return relative.to_path_buf();
        };
        if package_id == Some(graph.root_id.as_str()) {
            return relative.to_path_buf();
        }
        let package = graph.package(package_id.unwrap());
        PathBuf::from("dependencies")
            .join(&package.id)
            .join(relative)
    }

    fn resolve_import(
        &self,
        package_id: Option<&str>,
        import_path: &[String],
        span: Span,
    ) -> Result<(Option<String>, Vec<String>, PathBuf, bool), Diagnostic> {
        let Some(graph) = &self.graph else {
            let mut file = self.root.clone();
            for part in import_path {
                file.push(part);
            }
            file.set_extension("disp");
            return Ok((None, import_path.to_vec(), file, false));
        };
        let package_id = package_id.unwrap_or(&graph.root_id);
        let package = graph.package(package_id);
        let dependency = import_path
            .first()
            .and_then(|alias| package.dependencies.get(alias));
        let (target_id, local_path) = if let Some(target) = dependency {
            let mut local_candidate = package.source_root.clone();
            for part in import_path {
                local_candidate.push(part);
            }
            local_candidate.set_extension("disp");
            if local_candidate.exists() {
                return Err(project_error(
                    format!(
                        "import `{}` is ambiguous between dependency alias `{}` and a local module",
                        import_path.join("."),
                        import_path[0]
                    ),
                    span,
                )
                .with_help("rename the dependency alias or the local module"));
            }
            (target.clone(), import_path[1..].to_vec())
        } else {
            (package_id.to_owned(), import_path.to_vec())
        };
        let target = graph.package(&target_id);
        let (file, target_root) = if local_path.is_empty() {
            (target.root.join(&target.manifest.entry), false)
        } else {
            let mut file = target.source_root.clone();
            for part in &local_path {
                file.push(part);
            }
            file.set_extension("disp");
            (file, false)
        };
        Ok((Some(target_id), local_path, file, target_root))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DefinitionKind {
    Item,
    Enum,
    Variant { owner: String },
}

#[derive(Debug, Clone)]
struct Definition {
    canonical: String,
    kind: DefinitionKind,
    span: Span,
}

fn link(mut units: BTreeMap<String, Unit>, order: &[String]) -> Result<Program, Diagnostic> {
    let mut own = BTreeMap::<String, BTreeMap<String, Definition>>::new();
    let mut variants = HashMap::<(String, String), String>::new();
    for (key, unit) in &units {
        let mut definitions = BTreeMap::new();
        for declaration in &unit.program.structs {
            insert_definition(
                &mut definitions,
                &declaration.name,
                Definition {
                    canonical: canonical_name(unit, &declaration.name),
                    kind: DefinitionKind::Item,
                    span: declaration.name_span,
                },
            )?;
        }
        for declaration in &unit.program.enums {
            let enum_name = canonical_name(unit, &declaration.name);
            insert_definition(
                &mut definitions,
                &declaration.name,
                Definition {
                    canonical: enum_name.clone(),
                    kind: DefinitionKind::Enum,
                    span: declaration.name_span,
                },
            )?;
            for variant in &declaration.variants {
                let canonical = canonical_variant(unit, &declaration.name, &variant.name);
                variants.insert((enum_name.clone(), variant.name.clone()), canonical.clone());
                let definition = Definition {
                    canonical,
                    kind: DefinitionKind::Variant {
                        owner: enum_name.clone(),
                    },
                    span: variant.name_span,
                };
                match definitions.get(&variant.name) {
                    None => {
                        definitions.insert(variant.name.clone(), definition);
                    }
                    Some(previous) if matches!(previous.kind, DefinitionKind::Variant { .. }) => {
                        definitions.remove(&variant.name);
                    }
                    Some(_) => {}
                }
            }
        }
        for declaration in &unit.program.traits {
            insert_definition(
                &mut definitions,
                &declaration.name,
                Definition {
                    canonical: canonical_name(unit, &declaration.name),
                    kind: DefinitionKind::Item,
                    span: declaration.name_span,
                },
            )?;
        }
        for function in &unit.program.functions {
            insert_definition(
                &mut definitions,
                &function.name,
                Definition {
                    canonical: canonical_name(unit, &function.name),
                    kind: DefinitionKind::Item,
                    span: function.name_span,
                },
            )?;
        }
        own.insert(key.clone(), definitions);
    }

    let mut exports = BTreeMap::<String, BTreeMap<String, Definition>>::new();
    for key in order {
        let unit = &units[key];
        let mut surface = BTreeMap::new();
        let definitions = &own[key];
        for public in &unit.program.public_items {
            let definition = definitions.get(&public.node).ok_or_else(|| {
                project_error(
                    format!("public item `{}` has no declaration", public.node),
                    public.span,
                )
            })?;
            insert_visible(&mut surface, &public.node, definition.clone(), public.span)?;
            if let DefinitionKind::Enum = definition.kind {
                let enum_declaration = unit
                    .program
                    .enums
                    .iter()
                    .find(|candidate| candidate.name == public.node)
                    .unwrap();
                for variant in &enum_declaration.variants {
                    let canonical =
                        variants[&(definition.canonical.clone(), variant.name.clone())].clone();
                    insert_visible(
                        &mut surface,
                        &variant.name,
                        Definition {
                            canonical,
                            kind: DefinitionKind::Variant {
                                owner: definition.canonical.clone(),
                            },
                            span: variant.name_span,
                        },
                        variant.name_span,
                    )?;
                }
            }
        }
        for (import, dependency) in unit
            .program
            .imports
            .iter()
            .zip(&unit.import_targets)
            .filter(|(import, _)| import.public)
        {
            import_surface(&mut surface, &exports[dependency], import)?;
        }
        exports.insert(key.clone(), surface);
    }

    for key in order {
        let unit = units.get_mut(key).unwrap();
        let mut namespace = own[key].clone();
        for (import, dependency) in unit.program.imports.iter().zip(&unit.import_targets) {
            import_surface(&mut namespace, &exports[dependency], import)?;
        }
        validate_public_api(unit, &namespace, &exports[key])?;
        rename_unit(unit, &namespace, &variants);
    }

    let mut linked = Program {
        source_files: vec![],
        module: None,
        imports: vec![],
        public_items: vec![],
        structs: vec![],
        enums: vec![],
        traits: vec![],
        implementations: vec![],
        functions: vec![],
    };
    for key in order {
        let mut unit = units.remove(key).unwrap();
        linked.structs.append(&mut unit.program.structs);
        linked.enums.append(&mut unit.program.enums);
        linked.traits.append(&mut unit.program.traits);
        linked
            .implementations
            .append(&mut unit.program.implementations);
        linked.functions.append(&mut unit.program.functions);
    }
    Ok(linked)
}

fn validate_public_api(
    unit: &Unit,
    namespace: &BTreeMap<String, Definition>,
    exports: &BTreeMap<String, Definition>,
) -> Result<(), Diagnostic> {
    let public_names = unit
        .program
        .public_items
        .iter()
        .map(|item| item.node.as_str())
        .collect::<HashSet<_>>();
    let public_canonical = exports
        .values()
        .map(|definition| definition.canonical.as_str())
        .collect::<HashSet<_>>();
    for declaration in &unit.program.structs {
        if !public_names.contains(declaration.name.as_str()) {
            continue;
        }
        let generics = declaration
            .generics
            .iter()
            .map(|generic| generic.name.as_str())
            .collect::<HashSet<_>>();
        validate_generic_constraints(
            &declaration.generics,
            &generics,
            namespace,
            &public_canonical,
        )?;
        for field in &declaration.fields {
            validate_public_type(&field.ty, &generics, namespace, &public_canonical)?;
        }
    }
    for declaration in &unit.program.enums {
        if !public_names.contains(declaration.name.as_str()) {
            continue;
        }
        let generics = declaration
            .generics
            .iter()
            .map(|generic| generic.name.as_str())
            .collect::<HashSet<_>>();
        validate_generic_constraints(
            &declaration.generics,
            &generics,
            namespace,
            &public_canonical,
        )?;
        for variant in &declaration.variants {
            for payload in &variant.payload {
                validate_public_type(payload, &generics, namespace, &public_canonical)?;
            }
        }
    }
    for declaration in &unit.program.traits {
        if !public_names.contains(declaration.name.as_str()) {
            continue;
        }
        let trait_generics = declaration
            .generics
            .iter()
            .map(|generic| generic.name.as_str())
            .collect::<HashSet<_>>();
        validate_generic_constraints(
            &declaration.generics,
            &trait_generics,
            namespace,
            &public_canonical,
        )?;
        for method in &declaration.methods {
            let mut generics = trait_generics.clone();
            generics.extend(method.generics.iter().map(|generic| generic.name.as_str()));
            validate_generic_constraints(
                &method.generics,
                &generics,
                namespace,
                &public_canonical,
            )?;
            for parameter in &method.parameters {
                validate_public_type(&parameter.ty, &generics, namespace, &public_canonical)?;
            }
            if let Some(result) = &method.return_type {
                validate_public_type(result, &generics, namespace, &public_canonical)?;
            }
        }
    }
    for function in &unit.program.functions {
        if !public_names.contains(function.name.as_str()) {
            continue;
        }
        let generics = function
            .generics
            .iter()
            .map(|generic| generic.name.as_str())
            .collect::<HashSet<_>>();
        validate_generic_constraints(&function.generics, &generics, namespace, &public_canonical)?;
        for parameter in &function.parameters {
            validate_public_type(&parameter.ty, &generics, namespace, &public_canonical)?;
        }
        if let Some(result) = &function.return_type {
            validate_public_type(result, &generics, namespace, &public_canonical)?;
        }
    }
    Ok(())
}

fn validate_generic_constraints<'a>(
    parameters: &'a [GenericParameter],
    generics: &HashSet<&'a str>,
    namespace: &BTreeMap<String, Definition>,
    public: &HashSet<&str>,
) -> Result<(), Diagnostic> {
    for parameter in parameters {
        for constraint in &parameter.constraints {
            validate_public_type(constraint, generics, namespace, public)?;
        }
    }
    Ok(())
}

fn validate_public_type(
    ty: &TypeName,
    generics: &HashSet<&str>,
    namespace: &BTreeMap<String, Definition>,
    public: &HashSet<&str>,
) -> Result<(), Diagnostic> {
    for argument in &ty.arguments {
        validate_public_type(argument, generics, namespace, public)?;
    }
    if ty.name == "Self" || generics.contains(ty.name.as_str()) {
        return Ok(());
    }
    if let Some(definition) = namespace.get(&ty.name)
        && !matches!(definition.kind, DefinitionKind::Variant { .. })
        && !public.contains(definition.canonical.as_str())
    {
        return Err(project_error(
            format!("public API exposes private type or trait `{}`", ty.name),
            ty.span,
        )
        .with_help("mark the declaration `pub` or publicly re-export the imported type"));
    }
    Ok(())
}

fn insert_definition(
    definitions: &mut BTreeMap<String, Definition>,
    name: &str,
    definition: Definition,
) -> Result<(), Diagnostic> {
    if let Some(previous) = definitions.get(name) {
        return Err(
            project_error(format!("duplicate module item `{name}`"), definition.span).with_help(
                format!(
                    "the previous declaration begins at {}:{}",
                    previous.span.start.line, previous.span.start.column
                ),
            ),
        );
    }
    definitions.insert(name.to_owned(), definition);
    Ok(())
}

fn insert_visible(
    surface: &mut BTreeMap<String, Definition>,
    name: &str,
    definition: Definition,
    span: Span,
) -> Result<(), Diagnostic> {
    if let Some(previous) = surface.get(name) {
        if previous.canonical == definition.canonical {
            return Ok(());
        }
        return Err(project_error(
            format!("module namespace contains conflicting items named `{name}`"),
            span,
        )
        .with_help("use selective imports so every visible name has one unambiguous meaning"));
    }
    surface.insert(name.to_owned(), definition);
    Ok(())
}

fn import_surface(
    destination: &mut BTreeMap<String, Definition>,
    source: &BTreeMap<String, Definition>,
    import: &ast::ImportDeclaration,
) -> Result<(), Diagnostic> {
    if let Some(items) = &import.items {
        for item in items {
            let definition = source.get(&item.name).ok_or_else(|| {
                project_error(
                    format!(
                        "module `{}` has no public item `{}`",
                        import
                            .path
                            .iter()
                            .map(|part| part.node.as_str())
                            .collect::<Vec<_>>()
                            .join("."),
                        item.name
                    ),
                    item.name_span,
                )
            })?;
            insert_visible(
                destination,
                &item.alias,
                definition.clone(),
                item.alias_span,
            )?;
        }
    } else {
        for (name, definition) in source {
            insert_visible(destination, name, definition.clone(), import.span)?;
        }
    }
    Ok(())
}

fn canonical_name(unit: &Unit, name: &str) -> String {
    canonical_name_parts(unit.root, &unit.path, name)
}

fn canonical_name_parts(root: bool, path: &[String], name: &str) -> String {
    if root {
        name.to_owned()
    } else {
        format!("$disp${}${name}", path.join("$"))
    }
}

fn canonical_variant(unit: &Unit, owner: &str, variant: &str) -> String {
    canonical_variant_parts(unit.root, &unit.path, owner, variant)
}

fn canonical_variant_parts(root: bool, path: &[String], owner: &str, variant: &str) -> String {
    if root {
        variant.to_owned()
    } else {
        format!("$disp${}${owner}${variant}", path.join("$"))
    }
}

fn rename_unit(
    unit: &mut Unit,
    namespace: &BTreeMap<String, Definition>,
    variants: &HashMap<(String, String), String>,
) {
    let root = unit.root;
    let module_path = unit.path.clone();
    let mut renamer = Renamer::new(namespace, variants);
    for declaration in &mut unit.program.structs {
        declaration.name = canonical_name_parts(root, &module_path, &declaration.name);
        renamer.rename_generics(&mut declaration.generics);
        for field in &mut declaration.fields {
            renamer.rename_type(&mut field.ty);
        }
    }
    for declaration in &mut unit.program.enums {
        let original = declaration.name.clone();
        declaration.name = canonical_name_parts(root, &module_path, &original);
        renamer.rename_generics(&mut declaration.generics);
        for variant in &mut declaration.variants {
            variant.name = canonical_variant_parts(root, &module_path, &original, &variant.name);
            for payload in &mut variant.payload {
                renamer.rename_type(payload);
            }
        }
    }
    for declaration in &mut unit.program.traits {
        declaration.name = canonical_name_parts(root, &module_path, &declaration.name);
        renamer.rename_generics(&mut declaration.generics);
        for method in &mut declaration.methods {
            renamer.rename_generics(&mut method.generics);
            for parameter in &mut method.parameters {
                renamer.rename_type(&mut parameter.ty);
            }
            if let Some(result) = &mut method.return_type {
                renamer.rename_type(result);
            }
        }
    }
    for implementation in &mut unit.program.implementations {
        renamer.rename_generics(&mut implementation.generics);
        if let Some(trait_name) = &mut implementation.trait_name {
            renamer.rename_type(trait_name);
        }
        renamer.rename_type(&mut implementation.target);
        for (_, ty, _) in &mut implementation.associated_types {
            renamer.rename_type(ty);
        }
        for method in &mut implementation.methods {
            renamer.rename_function(method, false);
        }
    }
    for function in &mut unit.program.functions {
        renamer.rename_function(function, true);
        function.name = canonical_name_parts(root, &module_path, &function.name);
    }
}

struct Renamer<'a> {
    namespace: &'a BTreeMap<String, Definition>,
    variants: &'a HashMap<(String, String), String>,
    scopes: Vec<HashSet<String>>,
    generics: Vec<HashSet<String>>,
}

impl<'a> Renamer<'a> {
    fn new(
        namespace: &'a BTreeMap<String, Definition>,
        variants: &'a HashMap<(String, String), String>,
    ) -> Self {
        Self {
            namespace,
            variants,
            scopes: vec![],
            generics: vec![],
        }
    }

    fn rename_generics(&mut self, generics: &mut [GenericParameter]) {
        let names = generics
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<HashSet<_>>();
        self.generics.push(names);
        for generic in generics {
            for constraint in &mut generic.constraints {
                self.rename_type(constraint);
            }
        }
        self.generics.pop();
    }

    fn rename_function(&mut self, function: &mut ast::Function, top_level: bool) {
        let generic_names = function
            .generics
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<HashSet<_>>();
        self.generics.push(generic_names);
        for generic in &mut function.generics {
            for constraint in &mut generic.constraints {
                self.rename_type(constraint);
            }
        }
        for parameter in &mut function.parameters {
            self.rename_type(&mut parameter.ty);
        }
        if let Some(result) = &mut function.return_type {
            self.rename_type(result);
        }
        self.scopes.push(
            function
                .parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect(),
        );
        self.rename_block(&mut function.body);
        self.scopes.pop();
        self.generics.pop();
        if !top_level {
            // Method names are selected through their receiver type and stay source-facing.
        }
    }

    fn rename_type(&self, ty: &mut TypeName) {
        for argument in &mut ty.arguments {
            self.rename_type(argument);
        }
        if ty.name == "Self"
            || self
                .generics
                .iter()
                .rev()
                .any(|scope| scope.contains(&ty.name))
        {
            return;
        }
        if let Some(definition) = self.namespace.get(&ty.name)
            && !matches!(definition.kind, DefinitionKind::Variant { .. })
        {
            ty.name = definition.canonical.clone();
        }
    }

    fn rename_block(&mut self, block: &mut Block) {
        self.scopes.push(HashSet::new());
        for statement in &mut block.statements {
            self.rename_statement(&mut statement.node);
        }
        self.scopes.pop();
    }

    fn rename_block_with(&mut self, block: &mut Block, binding: String) {
        self.scopes.push(HashSet::from([binding]));
        for statement in &mut block.statements {
            self.rename_statement(&mut statement.node);
        }
        self.scopes.pop();
    }

    fn rename_statement(&mut self, statement: &mut Statement) {
        match statement {
            Statement::Binding {
                name,
                annotation,
                value,
                ..
            } => {
                if let Some(annotation) = annotation {
                    self.rename_type(annotation);
                }
                if let Some(value) = value {
                    self.rename_expr(value);
                }
                self.scopes.last_mut().unwrap().insert(name.clone());
            }
            Statement::Assignment { value, .. } => self.rename_expr(value),
            Statement::PlaceAssignment { target, value, .. } => {
                self.rename_expr(target);
                self.rename_expr(value);
            }
            Statement::Expression(expression) => self.rename_expr(expression),
            Statement::Return(value) => {
                if let Some(value) = value {
                    self.rename_expr(value);
                }
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.rename_expr(condition);
                self.rename_block(then_branch);
                if let Some(branch) = else_branch {
                    self.rename_block(branch);
                }
            }
            Statement::While { condition, body } => {
                self.rename_expr(condition);
                self.rename_block(body);
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                self.rename_expr(start);
                self.rename_expr(end);
                self.rename_block_with(body, name.clone());
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                self.rename_expr(iterable);
                self.rename_block_with(body, name.clone());
            }
            Statement::Loop(body) | Statement::Unsafe { body, .. } => self.rename_block(body),
            Statement::Break | Statement::Continue => {}
        }
    }

    fn rename_expr(&mut self, expression: &mut Expr) {
        match &mut expression.node {
            Expression::Array(values) => {
                for value in values {
                    self.rename_expr(value);
                }
            }
            Expression::Closure {
                parameters,
                return_type,
                body,
                ..
            } => {
                for parameter in parameters.iter_mut() {
                    self.rename_type(&mut parameter.ty);
                }
                if let Some(return_type) = return_type {
                    self.rename_type(return_type);
                }
                self.scopes.push(
                    parameters
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                );
                match body {
                    crate::ast::ClosureBody::Expression(value) => self.rename_expr(value),
                    crate::ast::ClosureBody::Block(block) => self.rename_block(block),
                }
                self.scopes.pop();
            }
            Expression::Identifier(name) => {
                if !self.is_local(name)
                    && let Some(definition) = self.namespace.get(name)
                {
                    *name = definition.canonical.clone();
                }
            }
            Expression::StructConstruct { name, fields, .. } => {
                if let Some(definition) = self.namespace.get(name) {
                    *name = definition.canonical.clone();
                }
                for field in fields {
                    self.rename_expr(&mut field.value);
                }
            }
            Expression::DataWrite { value, store, .. } => {
                self.rename_expr(value);
                self.rename_expr(store);
            }
            Expression::DataStore { path } => {
                if let Some(path) = path {
                    self.rename_expr(path);
                }
            }
            Expression::DataQuery {
                schema,
                store,
                predicate,
                order,
                limit,
                ..
            } => {
                if let Some(definition) = self.namespace.get(schema) {
                    *schema = definition.canonical.clone();
                }
                self.rename_expr(store);
                if let Some(predicate) = predicate {
                    self.rename_expr(predicate);
                }
                if let Some(order) = order {
                    self.rename_expr(&mut order.key);
                }
                if let Some(limit) = limit {
                    self.rename_expr(limit);
                }
            }
            Expression::DataRemove {
                schema,
                store,
                predicate,
                ..
            } => {
                if let Some(definition) = self.namespace.get(schema) {
                    *schema = definition.canonical.clone();
                }
                self.rename_expr(store);
                self.rename_expr(predicate);
            }
            Expression::FieldAccess { object, field, .. } => {
                if let Expression::Identifier(owner) = &object.node
                    && !self.is_local(owner)
                    && let Some(definition) = self.namespace.get(owner)
                    && matches!(definition.kind, DefinitionKind::Enum)
                    && let Some(variant) = self
                        .variants
                        .get(&(definition.canonical.clone(), field.clone()))
                {
                    *field = variant.clone();
                }
                self.rename_expr(object);
            }
            Expression::Index { object, index } => {
                self.rename_expr(object);
                self.rename_expr(index);
            }
            Expression::Subslice { object, start, end } => {
                self.rename_expr(object);
                self.rename_expr(start);
                self.rename_expr(end);
            }
            Expression::Match { value, arms } => {
                self.rename_expr(value);
                for arm in arms {
                    let mut bindings = HashSet::new();
                    self.rename_pattern(&mut arm.pattern.node, &mut bindings);
                    self.scopes.push(bindings);
                    if let Some(guard) = &mut arm.guard {
                        self.rename_expr(guard);
                    }
                    self.rename_expr(&mut arm.value);
                    self.scopes.pop();
                }
            }
            Expression::Try(value)
            | Expression::Await(value)
            | Expression::Spawn(value)
            | Expression::Move(value)
            | Expression::Dereference(value)
            | Expression::Unary { operand: value, .. } => self.rename_expr(value),
            Expression::Borrow { target, .. } => self.rename_expr(target),
            Expression::Binary { left, right, .. } => {
                self.rename_expr(left);
                self.rename_expr(right);
            }
            Expression::Call { callee, arguments } => {
                self.rename_expr(callee);
                for argument in arguments {
                    self.rename_expr(argument);
                }
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => {}
        }
    }

    fn rename_pattern(&self, pattern: &mut Pattern, bindings: &mut HashSet<String>) {
        match pattern {
            Pattern::Binding(name) => {
                bindings.insert(name.clone());
            }
            Pattern::Or(alternatives) => {
                for alternative in alternatives {
                    self.rename_pattern(&mut alternative.node, bindings);
                }
            }
            Pattern::Struct {
                type_name, fields, ..
            } => {
                if let Some(definition) = self.namespace.get(type_name) {
                    *type_name = definition.canonical.clone();
                }
                for field in fields {
                    self.rename_pattern(&mut field.pattern.node, bindings);
                }
            }
            Pattern::Variant {
                type_name,
                variant,
                arguments,
            } => {
                if let Some(owner) = type_name {
                    if let Some(definition) = self.namespace.get(owner) {
                        let canonical_owner = definition.canonical.clone();
                        if let Some(canonical) = self
                            .variants
                            .get(&(canonical_owner.clone(), variant.clone()))
                        {
                            *variant = canonical.clone();
                        }
                        *owner = canonical_owner;
                    }
                } else if let Some(definition) = self.namespace.get(variant)
                    && matches!(definition.kind, DefinitionKind::Variant { .. })
                {
                    *variant = definition.canonical.clone();
                }
                for argument in arguments {
                    self.rename_pattern(&mut argument.node, bindings);
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

    fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(name))
    }
}

fn module_key(path: &[String]) -> String {
    if path.is_empty() {
        "<root>".to_owned()
    } else {
        path.join(".")
    }
}

fn display_module(path: &[String]) -> String {
    if path.is_empty() {
        "root".to_owned()
    } else {
        path.join(".")
    }
}

fn project_error(message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Resolve, message, span)
}
