use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

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
