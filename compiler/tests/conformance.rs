use disp::{
    backend::{self, BuildOptions},
    check_path, check_source, lower_source, run_source,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug)]
struct Case {
    rule: String,
    name: String,
    mode: String,
    stage: String,
    expected: String,
    source: PathBuf,
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance")
}

fn cases() -> Vec<Case> {
    let root = fs::canonicalize(corpus_root()).expect("corpus root must be canonicalizable");
    let manifest =
        fs::read_to_string(root.join("manifest.tsv")).expect("manifest must be readable");
    let mut cases = Vec::new();
    for (index, line) in manifest.lines().enumerate() {
        if index == 0 {
            assert_eq!(line, "rule\tcase\tmode\tstage\texpected\tsource");
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            6,
            "manifest line {} must have six fields",
            index + 1
        );
        let source = fs::canonicalize(root.join(fields[5]))
            .unwrap_or_else(|error| panic!("case source {} is invalid: {error}", fields[5]));
        assert!(
            source.starts_with(&root),
            "case path must remain in the corpus"
        );
        cases.push(Case {
            rule: fields[0].into(),
            name: fields[1].into(),
            mode: fields[2].into(),
            stage: fields[3].into(),
            expected: fields[4].replace("\\n", "\n"),
            source,
        });
    }
    cases
}

#[test]
fn manifest_is_complete_unique_and_portable() {
    let cases = cases();
    let mut names = BTreeSet::new();
    let mut rules = BTreeMap::<String, usize>::new();
    for case in &cases {
        assert!(
            names.insert(case.name.clone()),
            "duplicate case {}",
            case.name
        );
        assert!(
            case.name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "case names must be portable ASCII identifiers: {}",
            case.name
        );
        let project_mode = case.mode.starts_with("project-");
        assert!(
            (project_mode && case.source.is_dir()) || (!project_mode && case.source.is_file()),
            "case source has the wrong kind: {}",
            case.source.display()
        );
        assert!(matches!(
            case.mode.as_str(),
            "check" | "reject" | "run" | "diagnostic" | "project-check" | "project-reject"
        ));
        *rules.entry(case.rule.clone()).or_default() += 1;
    }
    let expected = (1..=34)
        .map(|number| format!("DISP-CORE-{number:04}"))
        .collect::<BTreeSet<_>>();
    assert_eq!(rules.keys().cloned().collect::<BTreeSet<_>>(), expected);
}

#[test]
fn static_and_interpreter_conformance() {
    for case in cases() {
        match case.mode.as_str() {
            "check" => {
                let source = fs::read_to_string(&case.source).expect("case source must be UTF-8");
                check_source(&source).unwrap_or_else(|error| {
                    panic!("{} ({}) should pass: {error}", case.name, case.rule)
                });
            }
            "reject" => {
                let source = fs::read_to_string(&case.source).expect("case source must be UTF-8");
                let error = check_source(&source).unwrap_err();
                assert_eq!(
                    error.kind.to_string(),
                    case.stage,
                    "case {} returned: {}",
                    case.name,
                    error.message
                );
                assert!(
                    error.message.contains(&case.expected),
                    "case {} expected {:?} in {:?}",
                    case.name,
                    case.expected,
                    error.message
                );
            }
            "diagnostic" => {
                let source = fs::read_to_string(&case.source).expect("case source must be UTF-8");
                let error = check_source(&source).unwrap_err();
                assert_eq!(error.kind.to_string(), case.stage, "case {}", case.name);
                assert_eq!(error.kind.code(), case.expected, "case {}", case.name);
            }
            "run" => {
                let source = fs::read_to_string(&case.source).expect("case source must be UTF-8");
                let output = run_source(&source)
                    .unwrap_or_else(|error| panic!("{} should run: {error}", case.name))
                    .join("\n");
                assert_eq!(output, case.expected, "case {}", case.name);
            }
            "project-check" => {
                check_path(&case.source).unwrap_or_else(|error| {
                    panic!("{} ({}) should pass: {error}", case.name, case.rule)
                });
            }
            "project-reject" => {
                let error = check_path(&case.source).unwrap_err();
                assert_eq!(error.kind.to_string(), case.stage, "case {}", case.name);
                assert!(
                    error.message.contains(&case.expected),
                    "case {} expected {:?} in {:?}",
                    case.name,
                    case.expected,
                    error.message
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn run_cases_match_the_native_backend_when_host_policy_allows_launch() {
    let require_native = std::env::var_os("DISP_REQUIRE_NATIVE_CONFORMANCE").is_some();
    let mut executed = 0usize;
    let mut policy_blocked = Vec::new();
    let run_cases = cases()
        .into_iter()
        .filter(|case| case.mode == "run")
        .collect::<Vec<_>>();
    for case in &run_cases {
        let source = fs::read_to_string(&case.source).unwrap();
        let (hir, mir) = lower_source(&source)
            .unwrap_or_else(|error| panic!("{} should lower: {error}", case.name));
        let temp = std::env::temp_dir().join(format!(
            "disp-conformance-{}-{}-{}.disp",
            std::process::id(),
            case.rule,
            case.name
        ));
        fs::write(&temp, &source).unwrap();
        let artifact = backend::build(&hir, &mir, &temp, BuildOptions::default())
            .unwrap_or_else(|error| panic!("{} should build: {error}", case.name));
        let output = match Command::new(&artifact.executable).output() {
            Ok(output) => output,
            Err(error) if error.raw_os_error() == Some(4551) && !require_native => {
                policy_blocked.push(case.name.clone());
                continue;
            }
            Err(error) => panic!("{} native launch failed: {error}", case.name),
        };
        assert!(
            output.status.success(),
            "{} native process failed",
            case.name
        );
        executed += 1;
        assert_eq!(
            String::from_utf8(output.stdout)
                .expect("DISP print output must be UTF-8")
                .replace("\r\n", "\n")
                .trim_end(),
            case.expected,
            "case {}",
            case.name
        );
    }
    assert_eq!(executed + policy_blocked.len(), run_cases.len());
    if !policy_blocked.is_empty() {
        eprintln!(
            "native conformance launch blocked by host policy for: {}",
            policy_blocked.join(", ")
        );
    }
}
