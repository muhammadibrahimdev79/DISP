use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn data_path(name: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "disp-owned-data-{}-{nonce}-{name}.dispdb",
        std::process::id()
    ))
}

fn source_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn native_output(name: &str, source: &str) -> Option<String> {
    let path = std::env::temp_dir().join(format!("disp-data-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return None,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
    )
}

#[test]
fn data_schemas_are_nominal_and_reach_hir_mir() {
    let source = r#"
data User {
    id: int primary
    name: String
    active: bool
    bio: Option<String>
}

fn main() {}
"#;
    let (hir, mir) = lower_source(source).unwrap();
    let schema = hir.structs.iter().find(|item| item.name == "User").unwrap();
    assert!(schema.data);
    assert_eq!(
        schema.fields.iter().filter(|field| field.primary).count(),
        1
    );
    assert_eq!(mir.structs.iter().filter(|item| item.data).count(), 1);
}

#[test]
fn disp_data_operations_execute_without_sql_in_source() {
    let source = r#"
data User {
    id: int primary
    name: String
    active: bool
    enabled: Option<bool>
    bio: Option<String>
}

fn work() -> Result<bool, DataError> {
    var store = data memory?
    data add User { id: 1, name: "Ada", active: true, enabled: Some(true), bio: None } in store?
    data add User { id: 2, name: "Grace", active: false, enabled: None, bio: Some("compiler") } in store?
    selected = data find User in store where active == true order id descending limit 10?
    data save User { id: 1, name: "Ada Lovelace", active: true, enabled: Some(true), bio: None } in store?
    removed = data remove User in store where id == 2?
    remaining = data find User in store order id ascending?
    wanted = "Ada Lovelace"
    renamed = data find User in store where name == wanted?
    return Ok(selected.len() == 1 && removed == 1 && remaining.len() == 1 && renamed.len() == 1)
}

fn main() {
    match work() { Ok(value) => print(value), Err(error) => print(error) }
}
"#;
    assert_eq!(run_source(source).unwrap(), ["true"]);
    if let Some(output) = native_output("plans", source) {
        assert_eq!(output, "true\n");
    }

    let (hir, mir) = lower_source(source).unwrap();
    assert_eq!(hir.data_plans.len(), 7);
    assert_eq!(mir.data_plans.len(), 7);
    assert!(
        hir.functions
            .iter()
            .flat_map(|function| &function.body.statements)
            .any(|statement| format!("{statement:?}").contains("Data(DataPlanId"))
    );
}

#[test]
fn native_data_store_enforces_keys_and_typed_plans() {
    let source = r#"
data Record {
    id: int primary
    score: int
    active: bool
    note: Option<String>
}

fn verify() -> Result<bool, DataError> {
    var store = data memory?
    data add Record { id: 1, score: 2, active: true, note: None } in store?
    data add Record { id: 2, score: 5, active: true, note: Some("ready") } in store?
    duplicate = data add Record { id: 1, score: 99, active: false, note: None } in store
    duplicate_rejected = match duplicate { Err(_) => true, Ok(_) => false }
    var wanted: Option<String> = None
    bonus = 1
    rows = data find Record in store
        where note == wanted && active && score + bonus >= 3
        order score descending
        limit 1?
    return Ok(duplicate_rejected && rows.len() == 1)
}

fn main() {
    print(match verify() { Ok(value) => value, Err(_) => false })
}
"#;
    assert_eq!(run_source(source).unwrap(), ["true"]);
    if let Some(output) = native_output("native-typed-plans", source) {
        assert_eq!(output, "true\n");
    }
}

#[test]
fn interpreter_data_open_uses_the_versioned_disp_snapshot_and_reopens() {
    let path = data_path("interpreter");
    let path = source_path(&path);
    let seed = format!(
        r#"
data User {{ id: int primary name: String active: bool note: Option<String> }}
fn seed() -> Result<uint, DataError> {{
    var store = data open Path("{path}")?
    data add User {{ id: 1, name: "Ada", active: true, note: None }} in store?
    return data save User {{ id: 2, name: "Grace", active: false, note: Some("compiler") }} in store
}}
fn main() {{ print(match seed() {{ Ok(value) => value, Err(error) => 0 }}) }}
"#
    );
    assert_eq!(run_source(&seed).unwrap(), ["1"]);

    let bytes = fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"DISPDB\x1a\n");
    assert!(!bytes.starts_with(b"SQLite format 3"));

    let reopen = format!(
        r#"
data User {{ id: int primary name: String active: bool note: Option<String> }}
fn load() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    rows = data find User in store order id ascending?
    ada = data find User in store where name == "Ada"?
    wanted = Some("compiler")
    compiler = data find User in store where note == wanted?
    return Ok(rows.len() == 2 && ada.len() == 1 && compiler.len() == 1)
}}
fn main() {{ print(match load() {{ Ok(value) => value, Err(error) => false }}) }}
"#
    );
    assert_eq!(run_source(&reopen).unwrap(), ["true"]);
}

#[test]
fn interpreter_data_open_rejects_corruption_and_schema_drift() {
    let corrupt = data_path("corrupt");
    fs::write(&corrupt, b"not a database").unwrap();
    let corrupt = source_path(&corrupt);
    let open = format!(
        r#"fn main() {{ print(match data open Path("{corrupt}") {{ Ok(store) => false, Err(error) => true }}) }}"#
    );
    assert_eq!(run_source(&open).unwrap(), ["true"]);

    let path = data_path("schema-drift");
    let path = source_path(&path);
    let seed = format!(
        r#"data Item {{ id: int primary name: String }} fn seed() -> Result<uint, DataError> {{ var store = data open Path("{path}")?; return data add Item {{ id: 1, name: "one" }} in store }} fn main() {{ print(match seed() {{ Ok(value) => value, Err(error) => 0 }}) }}"#
    );
    assert_eq!(run_source(&seed).unwrap(), ["1"]);
    let changed = format!(
        r#"data Item {{ id: int primary score: int }} fn inspect() -> Result<bool, DataError> {{ var store = data open Path("{path}")?; rows = data find Item in store?; return Ok(rows.len() == 1) }} fn main() {{ print(match inspect() {{ Ok(value) => value, Err(error) => false }}) }}"#
    );
    assert_eq!(run_source(&changed).unwrap(), ["false"]);
}

#[test]
fn native_and_interpreter_data_open_share_the_exact_durable_format() {
    let interpreted_path = source_path(&data_path("cross-interpreter"));
    let native_path = source_path(&data_path("cross-native"));
    let seed = |path: &str| {
        format!(
            r#"
data Person {{ id: int primary name: String active: bool note: Option<String> }}
fn seed() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    first = data add Person {{ id: 1, name: "Ada", active: true, note: None }} in store?
    second = data save Person {{ id: 2, name: "Grace", active: false, note: Some("compiler") }} in store?
    return Ok(first == 1 && second == 1)
}}
fn main() {{ print(match seed() {{ Ok(value) => value, Err(_) => false }}) }}
"#
        )
    };

    assert_eq!(run_source(&seed(&interpreted_path)).unwrap(), ["true"]);
    if let Some(output) = native_output("durable-native-seed", &seed(&native_path)) {
        assert_eq!(output, "true\n");
        assert_eq!(
            fs::read(&interpreted_path).unwrap(),
            fs::read(&native_path).unwrap()
        );

        let verify = |path: &str| {
            format!(
                r#"
data Person {{ id: int primary name: String active: bool note: Option<String> }}
fn verify() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    rows = data find Person in store order id ascending?
    wanted_name = "Ada"
    ada = data find Person in store where name == wanted_name?
    wanted = Some("compiler")
    grace = data find Person in store where note == wanted?
    return Ok(rows.len() == 2 && ada.len() == 1 && grace.len() == 1)
}}
fn main() {{ print(match verify() {{ Ok(value) => value, Err(_) => false }}) }}
"#
            )
        };
        if let Some(output) = native_output(
            "durable-native-reads-interpreter",
            &verify(&interpreted_path),
        ) {
            assert_eq!(output, "true\n");
        }
        assert_eq!(run_source(&verify(&native_path)).unwrap(), ["true"]);
    }
}

#[test]
fn native_data_open_returns_typed_errors_for_corruption_and_schema_drift() {
    let corrupt = data_path("native-corrupt");
    fs::write(&corrupt, b"not a database").unwrap();
    let corrupt = source_path(&corrupt);
    let source = format!(
        r#"fn main() {{ print(match data open Path("{corrupt}") {{ Ok(_) => false, Err(_) => true }}) }}"#
    );
    if let Some(output) = native_output("durable-native-corrupt", &source) {
        assert_eq!(output, "true\n");
    }

    let path = source_path(&data_path("native-schema-drift"));
    let seed = format!(
        r#"data Item {{ id: int primary name: String }} fn seed()->Result<uint,DataError>{{ var store=data open Path("{path}")?; return data add Item{{id:1,name:"one"}} in store }} fn main(){{ print(match seed(){{Ok(value)=>value,Err(_)=>0}}) }}"#
    );
    if native_output("durable-native-schema-seed", &seed).is_some() {
        let changed = format!(
            r#"data Item {{ id: int primary score: int }} fn inspect()->Result<bool,DataError>{{ var store=data open Path("{path}")?; rows=data find Item in store?; return Ok(rows.len()==1) }} fn main(){{ print(match inspect(){{Ok(value)=>value,Err(_)=>false}}) }}"#
        );
        if let Some(output) = native_output("durable-native-schema-changed", &changed) {
            assert_eq!(output, "false\n");
        }
    }
}

#[test]
fn invalid_data_schemas_have_source_diagnostics() {
    let missing = check_source("data User { id: int } fn main() {}").unwrap_err();
    assert!(missing.message.contains("primary"), "{missing}");
    assert_eq!((missing.span.start.line, missing.span.start.column), (1, 6));

    let unsupported =
        check_source("data User { id: int primary tags: List<String> } fn main() {}").unwrap_err();
    assert!(
        unsupported.message.contains("cannot be stored"),
        "{unsupported}"
    );
    assert_eq!(
        (unsupported.span.start.line, unsupported.span.start.column),
        (1, 35)
    );

    let unknown_field = check_source(
        "data User { id: int primary } fn main(){ var store=data memory?; values=data find User in store where missing == 1? }",
    )
    .unwrap_err();
    assert!(unknown_field.message.contains("unknown name `missing`"));

    let unsafe_delete = check_source(
        "data User { id: int primary } fn main(){ var store=data memory?; data remove User in store }",
    )
    .unwrap_err();
    assert!(unsafe_delete.message.contains("requires `where`"));

    let non_boolean = check_source(
        "data User { id: int primary } fn run()->Result<List<User>,DataError>{ var store=data memory?; return data find User in store where id + 1 } fn main(){}",
    )
    .unwrap_err();
    assert!(non_boolean.message.contains("condition"), "{non_boolean}");

    let wrong_store = check_source(
        "data User { id: int primary } fn run()->Result<List<User>,DataError>{ store=\"memory\"; return data find User in store } fn main(){}",
    )
    .unwrap_err();
    assert!(wrong_store.message.contains("Data store"), "{wrong_store}");

    let sql_database = check_source(
        "data User { id: int primary } fn run()->Result<List<User>,DataError>{ var db=Database.memory()?; return data find User in db } fn main(){}",
    )
    .unwrap_err();
    assert!(
        sql_database.message.contains("expected DataStore"),
        "{sql_database}"
    );

    let raw_sql = check_source(
        "fn run()->Result<uint,DataError>{ var store=data memory?; var args: List<Json> = List.new(); return store.execute(\"select 1\", args) } fn main(){}",
    )
    .unwrap_err();
    assert!(raw_sql.message.contains("DataStore"), "{raw_sql}");

    let immutable_store = check_source(
        "data User { id: int primary } fn run()->Result<List<User>,DataError>{ let store=data memory?; return data find User in store } fn main(){}",
    )
    .unwrap_err();
    assert!(
        immutable_store.message.contains("mutable"),
        "{immutable_store}"
    );

    let ordinary_struct = check_source(
        "struct User { id: int } fn run()->Result<uint,DataError>{ var store=data memory?; return data add User{id:1} in store } fn main(){}",
    )
    .unwrap_err();
    assert!(
        ordinary_struct.message.contains("data schema"),
        "{ordinary_struct}"
    );
}
