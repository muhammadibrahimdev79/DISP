use disp::{MAX_SOURCE_BYTES, backend, check_source, lower_source, run_source};
use std::{
    env, fs,
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
    if let Err(error) = execute(env::args().skip(1).collect()) {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn execute(arguments: Vec<String>) -> Result<(), String> {
    let (command, path) = match arguments.as_slice() {
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
        _ => return Err("usage: disp <check|build|run|interpret> [--dump-hir|--dump-mir|--release|--emit-c|--emit-obj] <file.disp>".into()),
    };
    validate_path(path)?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("error: could not inspect `{path}`: {error}"))?;
    if metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(format!(
            "error: `{path}` is {} bytes; the current safety limit is {MAX_SOURCE_BYTES} bytes",
            metadata.len()
        ));
    }
    let source = fs::read_to_string(path)
        .map_err(|error| format!("error: could not read `{path}` as UTF-8: {error}"))?;

    match command {
        Command::Check => check_source(&source)
            .map(|_| ())
            .map_err(|diagnostic| diagnostic.render(path)),
        Command::Run => {
            let (hir, mir) = lower_source(&source).map_err(|diagnostic| diagnostic.render(path))?;
            let artifacts = backend::build(
                &hir,
                &mir,
                Path::new(path),
                backend::BuildOptions::default(),
            )
            .map_err(|diagnostic| diagnostic.render(path))?;
            let status = ProcessCommand::new(&artifacts.executable)
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
            let source = source.clone();
            let path = path.to_owned();
            let output = std::thread::Builder::new()
                .name("disp-interpreter".into())
                .stack_size(16 * 1024 * 1024)
                .spawn(move || run_source(&source).map_err(|diagnostic| diagnostic.render(&path)))
                .map_err(|error| format!("error: could not start the interpreter: {error}"))?
                .join()
                .map_err(|_| "error: the interpreter terminated unexpectedly".to_owned())??;
            for line in output {
                println!("{line}");
            }
            Ok(())
        }
        Command::Build(options) => {
            let (hir, mir) = lower_source(&source).map_err(|diagnostic| diagnostic.render(path))?;
            let artifacts = backend::build(&hir, &mir, Path::new(path), options)
                .map_err(|diagnostic| diagnostic.render(path))?;
            println!("{}", artifacts.executable.display());
            Ok(())
        }
        Command::DumpHir => {
            let (hir, _) = lower_source(&source).map_err(|diagnostic| diagnostic.render(path))?;
            print!("{}", disp::hir::dump(&hir));
            Ok(())
        }
        Command::DumpMir => {
            let (_, mir) = lower_source(&source).map_err(|diagnostic| diagnostic.render(path))?;
            print!("{}", disp::mir::dump(&mir));
            Ok(())
        }
    }
}

fn validate_path(path: &str) -> Result<(), String> {
    if Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("disp")
    {
        return Err("error: DISP source files must end with `.disp`".into());
    }
    Ok(())
}
