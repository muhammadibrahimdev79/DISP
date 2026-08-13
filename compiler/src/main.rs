use disp::{backend, check_path, lower_path, run_path_with_args};
use std::{
    env,
    path::Path,
    process::{self, Command as ProcessCommand},
};

enum Command {
    Check,
    Run,
    DumpHir,
    DumpMir,
    Build(backend::BuildOptions),
    Interpret,
}

fn main() {
    let arguments = env::args().skip(1).collect();
    let result = std::thread::Builder::new()
        .name("disp-driver".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || execute(arguments))
        .and_then(|driver| {
            driver.join().map_err(|_| {
                std::io::Error::other("the DISP compiler driver terminated unexpectedly")
            })
        });
    let result = match result {
        Ok(result) => result,
        Err(error) => Err(format!("error: could not run the compiler driver: {error}")),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn execute(arguments: Vec<String>) -> Result<(), String> {
    if let [command, path] = arguments.as_slice()
        && command == "new"
    {
        disp::project::create(Path::new(path)).map_err(|diagnostic| diagnostic.render(path))?;
        println!("created DISP project `{path}`");
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "lock"
    {
        let lock = disp::package::write_lock(Path::new(path))
            .map_err(|diagnostic| diagnostic.render(path))?;
        println!("{}", lock.display());
        return Ok(());
    }
    if let [command, path] = arguments.as_slice()
        && command == "tree"
    {
        let graph =
            disp::package::verify(Path::new(path)).map_err(|diagnostic| diagnostic.render(path))?;
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
        [command, path] if command == "build" => (Command::Build(backend::BuildOptions::default()), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--release" => (Command::Build(backend::BuildOptions { optimized: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--emit-c" => (Command::Build(backend::BuildOptions { emit_c: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "build" && flag == "--emit-obj" => (Command::Build(backend::BuildOptions { emit_object: true, ..Default::default() }), path.as_str()),
        [command, flag, path] if command == "check" && flag == "--dump-hir" => {
            (Command::DumpHir, path.as_str())
        }
        [command, flag, path] if command == "check" && flag == "--dump-mir" => {
            (Command::DumpMir, path.as_str())
        }
        _ => return Err("usage: disp <new|lock|tree> <directory> | disp <check|build|run|interpret> [--dump-hir|--dump-mir|--release|--emit-c|--emit-obj] <file.disp|project-directory> [-- program-arguments...]".into()),
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
            .map_err(|diagnostic| diagnostic.render(path)),
        Command::Run => {
            let (hir, mir) =
                lower_path(source_path).map_err(|diagnostic| diagnostic.render(path))?;
            let artifacts = backend::build(
                &hir,
                &mir,
                Path::new(path),
                backend::BuildOptions::default(),
            )
            .map_err(|diagnostic| diagnostic.render(path))?;
            let status = ProcessCommand::new(&artifacts.executable)
                .args(&program_arguments)
                .status()
                .map_err(|error| {
                    format!(
                        "error: could not run `{}`: {error}",
                        artifacts.executable.display()
                    )
                })?;
            if !status.success() {
                return Err(format!("native program exited with status {status}"));
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
                        .map_err(|diagnostic| diagnostic.render(&display_path))
                })
                .map_err(|error| format!("error: could not start the interpreter: {error}"))?
                .join()
                .map_err(|_| "error: the interpreter terminated unexpectedly".to_owned())??;
            for line in output {
                println!("{line}");
            }
            Ok(())
        }
        Command::Build(options) => {
            let (hir, mir) =
                lower_path(source_path).map_err(|diagnostic| diagnostic.render(path))?;
            let artifacts = backend::build(&hir, &mir, Path::new(path), options)
                .map_err(|diagnostic| diagnostic.render(path))?;
            println!("{}", artifacts.executable.display());
            Ok(())
        }
        Command::DumpHir => {
            let (hir, _) = lower_path(source_path).map_err(|diagnostic| diagnostic.render(path))?;
            print!("{}", disp::hir::dump(&hir));
            Ok(())
        }
        Command::DumpMir => {
            let (_, mir) = lower_path(source_path).map_err(|diagnostic| diagnostic.render(path))?;
            print!("{}", disp::mir::dump(&mir));
            Ok(())
        }
    }
}
