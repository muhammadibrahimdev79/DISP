use disp::{
    MAX_SOURCE_BYTES, backend, check_path, constant_report_path,
    diagnostics::{Diagnostic, render_driver_json},
    effect_report_path, expansion_report_path, formatter, lower_path,
    process_sandbox::{SandboxProfile, SandboxedCommand},
    run_path_with_args,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

enum Command {
    Check,
    Run,
    DumpHir,
    DumpMir,
    DumpEffects,
    DumpConstants,
    DumpExpansions,
    Build(backend::BuildOptions),
    CHeader,
    BuildFreestanding,
    BuildFreestanding32,
    BuildFreestanding64,
    BuildFreestandingAarch64,
    Interpret,
}

#[derive(Clone, Copy)]
enum DiagnosticFormat {
    Human,
    Json,
}

struct DriverError {
    code: &'static str,
    message: String,
    diagnostic: Option<(Box<Diagnostic>, String)>,
}

impl DriverError {
    fn diagnostic(diagnostic: Diagnostic, fallback_file: impl Into<String>) -> Self {
        Self {
            code: diagnostic.kind.code(),
            message: diagnostic.message.clone(),
            diagnostic: Some((Box::new(diagnostic), fallback_file.into())),
        }
    }

    fn driver(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            code: "DISP-DRIVER-0001",
            message: message
                .strip_prefix("error: ")
                .unwrap_or(&message)
                .to_owned(),
            diagnostic: None,
        }
    }

    fn render(&self, format: DiagnosticFormat) -> String {
        match (format, &self.diagnostic) {
            (DiagnosticFormat::Human, Some((diagnostic, file))) => diagnostic.render(file),
            (DiagnosticFormat::Json, Some((diagnostic, file))) => diagnostic.render_json(file),
            (DiagnosticFormat::Human, None) => format!("error: {}", self.message),
            (DiagnosticFormat::Json, None) => render_driver_json(self.code, &self.message),
        }
    }
}

impl From<String> for DriverError {
    fn from(message: String) -> Self {
        Self::driver(message)
    }
}

impl From<&str> for DriverError {
    fn from(message: &str) -> Self {
        Self::driver(message)
    }
}

fn main() {
    let mut arguments = env::args().skip(1).collect::<Vec<_>>();
    let format = match take_diagnostic_format(&mut arguments) {
        Ok(format) => format,
        Err(error) => {
            eprintln!("{}", error.render(DiagnosticFormat::Human));
            process::exit(1);
        }
    };
    let result = match std::thread::Builder::new()
        .name("disp-driver".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || execute(arguments))
    {
        Ok(driver) => driver.join().unwrap_or_else(|_| {
            Err(DriverError::driver(
                "compiler driver terminated unexpectedly",
            ))
        }),
        Err(error) => Err(DriverError::driver(format!(
            "could not start compiler driver: {error}"
        ))),
    };
    if let Err(error) = result {
        eprintln!("{}", error.render(format));
        process::exit(1);
    }
}

fn take_diagnostic_format(arguments: &mut Vec<String>) -> Result<DiagnosticFormat, DriverError> {
    let mut separator = arguments
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(arguments.len());
    let mut selected = None;
    let mut index = 0usize;
    while index < separator {
        let argument = &arguments[index];
        if !argument.starts_with("--diagnostic-format") {
            index += 1;
            continue;
        }
        if selected.is_some() {
            return Err(DriverError::driver(
                "`--diagnostic-format` may be provided only once",
            ));
        }
        selected = Some(match argument.as_str() {
            "--diagnostic-format=human" => DiagnosticFormat::Human,
            "--diagnostic-format=json" => DiagnosticFormat::Json,
            _ => {
                return Err(DriverError::driver(
                    "diagnostic format must be `human` or `json`",
                ));
            }
        });
        arguments.remove(index);
        separator -= 1;
    }
    Ok(selected.unwrap_or(DiagnosticFormat::Human))
}

fn execute(arguments: Vec<String>) -> Result<(), DriverError> {
    if matches!(arguments.as_slice(), [flag] if flag == "--version" || flag == "-V") {
        println!("DISP {} Developer Preview", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if matches!(arguments.as_slice(), [flag] if flag == "--help" || flag == "-h") {
        println!(
            "DISP {} Developer Preview\n\nusage:\n  disp <file.disp|project-directory>\n  disp <check|build|run|interpret|header> [option] <file.disp|project-directory>\n  disp fmt [--check] <file.disp|project-directory>\n  disp migrate [--check] <project-directory>\n  disp <new|lock|tree> <directory>\n\nglobal options:\n  --diagnostic-format=<human|json>\n\nbuild/check options:\n  --release  --sanitize  --emit-c  --emit-obj  --library  --freestanding  --freestanding32  --freestanding64  --freestanding-aarch64  --dump-hir  --dump-mir  --dump-effects  --dump-constants  --dump-expansions\n\n`header` emits the deterministic DISP C ABI v1 header.\n`build --library` emits a shared library and its C consumer header.\n`build --freestanding` emits a deterministic x86 BIOS image directly.\n`build --freestanding32` emits the paged x86 protected-mode image directly.\n`build --freestanding64` emits the paged x86-64 long-mode image directly.\n`build --freestanding-aarch64` emits a direct AArch64 QEMU virt-8.2 image.\nprogram arguments follow `--`.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "new"
    {
        disp::project::create(Path::new(path))
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
        println!("created DISP project `{path}`");
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "lock"
    {
        let lock = disp::package::write_lock(Path::new(path))
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
        println!("{}", lock.display());
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "tree"
    {
        let graph = disp::package::verify(Path::new(path))
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
        for line in graph.tree() {
            let indent = "  ".repeat(line.depth);
            if let Some(alias) = line.alias {
                println!("{indent}{alias} -> {}", line.id);
            } else {
                println!("{indent}{}", line.id);
            }
        }
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "migrate"
    {
        let report = disp::project::migrate(Path::new(path), false)
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
        if report.changed {
            println!(
                "migrated {} to DISP edition {}",
                report.manifest.display(),
                report.edition
            );
        } else {
            println!(
                "{} already declares DISP edition {}",
                report.manifest.display(),
                report.edition
            );
        }
        return Ok(());
    }
    if let [command, flag, path] = arguments.as_slice()
        && command == "migrate"
        && flag == "--check"
    {
        let report = disp::project::migrate(Path::new(path), true)
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
        if report.changed {
            return Err(DriverError::driver(format!(
                "{} does not explicitly declare DISP edition {}; run `disp migrate {path}`",
                report.manifest.display(),
                report.edition
            )));
        }
        println!(
            "{} declares DISP edition {}",
            report.manifest.display(),
            report.edition
        );
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "fmt"
    {
        return format_path(Path::new(path), false);
    }
    if let [command, flag, path] = arguments.as_slice()
        && command == "fmt"
        && flag == "--check"
    {
        return format_path(Path::new(path), true);
    }
    let separator = arguments.iter().position(|argument| argument == "--");
    let (driver_arguments, program_arguments) = separator.map_or_else(
        || (arguments.as_slice(), &[][..]),
        |index| (&arguments[..index], &arguments[index + 1..]),
    );
    let (command, path) = match driver_arguments {
        [path] => (Command::Run, path.as_str()),
        [command, path] if command == "check" => (Command::Check, path.as_str()),
        [command, path] if command == "run" => (Command::Run, path.as_str()),
        [command, path] if command == "interpret" => (Command::Interpret, path.as_str()),
        [command, path] if command == "header" => (Command::CHeader, path.as_str()),
        [command, path] if command == "build" => (Command::Build(backend::BuildOptions::default()), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--release" => (Command::Build(backend::BuildOptions { optimized: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--sanitize" => (Command::Build(backend::BuildOptions { sanitizers: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--emit-c" => (Command::Build(backend::BuildOptions { emit_c: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--emit-obj" => (Command::Build(backend::BuildOptions { emit_object: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--library" => (Command::Build(backend::BuildOptions { library: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--freestanding" => (Command::BuildFreestanding, path.as_str()),
        [command, flag, path] if command == "build" && flag == "--freestanding32" => (Command::BuildFreestanding32, path.as_str()),
        [command, flag, path] if command == "build" && flag == "--freestanding64" => (Command::BuildFreestanding64, path.as_str()),
        [command, flag, path] if command == "build" && flag == "--freestanding-aarch64" => (Command::BuildFreestandingAarch64, path.as_str()),
        [command, flag, path] if command == "check" && flag == "--dump-hir" => {
            (Command::DumpHir, path.as_str())
        }
        [command, flag, path] if command == "check" && flag == "--dump-mir" => {
            (Command::DumpMir, path.as_str())
        }
        [command, flag, path] if command == "check" && flag == "--dump-effects" => {
            (Command::DumpEffects, path.as_str())
        }
        [command, flag, path] if command == "check" && flag == "--dump-constants" => {
            (Command::DumpConstants, path.as_str())
        }
        [command, flag, path] if command == "check" && flag == "--dump-expansions" => {
            (Command::DumpExpansions, path.as_str())
        }
        _ => return Err("usage: disp <new|lock|tree> <directory> | disp migrate [--check] <project-directory> | disp fmt [--check] <file.disp|project-directory> | disp <check|build|run|interpret|header> [--dump-hir|--dump-mir|--dump-effects|--dump-constants|--dump-expansions|--release|--sanitize|--emit-c|--emit-obj|--library|--freestanding|--freestanding32|--freestanding64|--freestanding-aarch64] <file.disp|project-directory> [-- program-arguments...]\ntry `disp --help` for details".into()),
    };
    if !program_arguments.is_empty() && !matches!(command, Command::Run | Command::Interpret) {
        return Err(
            "program arguments are only accepted by `disp run` and `disp interpret`".into(),
        );
    }
    let program_arguments = program_arguments.to_vec();
    let source_path = Path::new(path);

    match command {
        Command::Check => check_path(source_path)
            .map(|_| ())
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path)),
        Command::Run => {
            let (hir, mir) = lower_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let artifacts = backend::build(
                &hir,
                &mir,
                Path::new(path),
                backend::BuildOptions::default(),
            )
            .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let mut native = SandboxedCommand::new(&artifacts.executable);
            native.args(&program_arguments);
            let status = native.status(SandboxProfile::Runtime).map_err(|error| {
                DriverError::driver(format!(
                    "error: could not run `{}`: {error}",
                    artifacts.executable.display()
                ))
            })?;
            if !status.success() {
                return Err(DriverError::driver(format!(
                    "native program exited with status {status}"
                )));
            }
            Ok(())
        }
        Command::Interpret => {
            let path = source_path.to_path_buf();
            let display_path = path.display().to_string();
            let output = std::thread::Builder::new()
                .name("disp-interpreter".into())
                .stack_size(16 * 1024 * 1024)
                .spawn(move || {
                    run_path_with_args(&path, &program_arguments)
                        .map_err(|diagnostic| DriverError::diagnostic(diagnostic, display_path))
                })
                .map_err(|error| {
                    DriverError::driver(format!("could not start the interpreter: {error}"))
                })?
                .join()
                .map_err(|_| DriverError::driver("interpreter terminated unexpectedly"))??;
            for line in output {
                println!("{line}");
            }
            Ok(())
        }
        Command::Build(options) => {
            let (hir, mir) = lower_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let artifacts = backend::build(&hir, &mir, Path::new(path), options)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            println!("{}", artifacts.executable.display());
            if options.library {
                let header = backend::c_header::write(&hir, source_path)
                    .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
                println!("{}", header.display());
            }
            Ok(())
        }
        Command::CHeader => {
            let (hir, _) = lower_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let header = backend::c_header::write(&hir, source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            println!("{}", header.display());
            Ok(())
        }
        Command::BuildFreestanding => {
            let program = check_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let image = disp::freestanding::build_x86_bios(&program, source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            println!("{}", image.display());
            Ok(())
        }
        Command::BuildFreestanding32 => {
            let program = check_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let image = disp::freestanding32::build_x86_protected(&program, source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            println!("{}", image.display());
            Ok(())
        }
        Command::BuildFreestanding64 => {
            let program = check_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let image = disp::freestanding64::build_x86_64(&program, source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            println!("{}", image.display());
            Ok(())
        }
        Command::BuildFreestandingAarch64 => {
            let program = check_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            let image = disp::freestanding_aarch64::build_aarch64_virt(&program, source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            println!("{}", image.display());
            Ok(())
        }
        Command::DumpHir => {
            let (hir, _) = lower_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            print!("{}", disp::hir::dump(&hir));
            Ok(())
        }
        Command::DumpMir => {
            let (_, mir) = lower_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            print!("{}", disp::mir::dump(&mir));
            Ok(())
        }
        Command::DumpEffects => {
            let report = effect_report_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            print!("{}", report.render());
            Ok(())
        }
        Command::DumpConstants => {
            let report = constant_report_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            print!("{}", report.render());
            Ok(())
        }
        Command::DumpExpansions => {
            let report = expansion_report_path(source_path)
                .map_err(|diagnostic| DriverError::diagnostic(diagnostic, path))?;
            print!("{}", report.render());
            Ok(())
        }
    }
}

fn format_path(path: &Path, check: bool) -> Result<(), DriverError> {
    let files = format_files(path)?;
    let mut changed = Vec::new();
    for file in files {
        let metadata = fs::metadata(&file)
            .map_err(|error| format!("error: could not inspect `{}`: {error}", file.display()))?;
        let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        if size > MAX_SOURCE_BYTES {
            return Err(format!(
                "error: `{}` is {size} bytes; the formatter limit is {MAX_SOURCE_BYTES} bytes",
                file.display()
            )
            .into());
        }
        let source = fs::read_to_string(&file).map_err(|error| {
            format!(
                "error: could not read `{}` as UTF-8: {error}",
                file.display()
            )
        })?;
        let formatted = formatter::format_source(&source).map_err(|diagnostic| {
            DriverError::diagnostic(diagnostic, file.display().to_string())
        })?;
        if formatted != source {
            changed.push(file.clone());
            if !check {
                fs::write(&file, formatted).map_err(|error| {
                    format!("error: could not write `{}`: {error}", file.display())
                })?;
            }
        }
    }

    if check && !changed.is_empty() {
        let paths = changed
            .iter()
            .map(|path| format!("  {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "error: DISP formatting differs in:\n{paths}\nrun `disp fmt` to update these files"
        )
        .into());
    }
    if !check {
        for path in changed {
            println!("formatted {}", path.display());
        }
    }
    Ok(())
}

fn format_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        if path.extension().and_then(|extension| extension.to_str()) != Some("disp") {
            return Err(format!(
                "error: source path `{}` must end with `.disp`",
                path.display()
            ));
        }
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        return Err(format!(
            "error: DISP source path `{}` does not exist",
            path.display()
        ));
    }

    let source_root = path.join("src");
    if !source_root.is_dir() {
        return Err(format!(
            "error: DISP project `{}` does not contain a `src` directory",
            path.display()
        ));
    }
    let mut pending = vec![source_root];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("error: could not read `{}`: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| {
                format!(
                    "error: could not read an entry in `{}`: {error}",
                    directory.display()
                )
            })?;
            let kind = entry.file_type().map_err(|error| {
                format!(
                    "error: could not inspect `{}`: {error}",
                    entry.path().display()
                )
            })?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("disp")
            {
                files.push(entry.path());
                if files.len() > 10_000 {
                    return Err("error: formatter project limit is 10,000 DISP source files".into());
                }
            }
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "error: DISP project `{}` contains no source files",
            path.display()
        ));
    }
    Ok(files)
}
