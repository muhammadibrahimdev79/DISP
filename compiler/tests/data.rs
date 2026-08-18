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

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

fn legacy_empty_snapshot() -> Vec<u8> {
    let payload = 0_u32.to_le_bytes();
    let mut bytes = Vec::with_capacity(32 + payload.len());
    bytes.extend_from_slice(b"DISPDB\x1a\n");
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&fnv1a(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

fn committed_wal(old: &[u8], new: &[u8]) -> Vec<u8> {
    const PAGE_SIZE: usize = 4096;
    const HEADER_SIZE: usize = 64;
    const RECORD_SIZE: usize = 8 + PAGE_SIZE;

    assert_eq!(new.len() % PAGE_SIZE, 0);
    let pages = new.len() / PAGE_SIZE;
    let changed = new
        .chunks_exact(PAGE_SIZE)
        .enumerate()
        .filter(|(page, bytes)| {
            let start = page * PAGE_SIZE;
            old.get(start..start + PAGE_SIZE) != Some(*bytes)
        })
        .collect::<Vec<_>>();
    assert!(changed.iter().any(|(page, _)| *page == 0));

    let mut wal = vec![0_u8; HEADER_SIZE + changed.len() * RECORD_SIZE];
    wal[..8].copy_from_slice(b"DISPWAL\n");
    wal[8..12].copy_from_slice(&1_u32.to_le_bytes());
    wal[12..16].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    wal[16..24].copy_from_slice(&new[16..24]);
    wal[24..32].copy_from_slice(&(pages as u64).to_le_bytes());
    wal[32..40].copy_from_slice(&(changed.len() as u64).to_le_bytes());
    for (record, (page, bytes)) in changed.into_iter().enumerate() {
        let start = HEADER_SIZE + record * RECORD_SIZE;
        wal[start..start + 8].copy_from_slice(&(page as u64).to_le_bytes());
        wal[start + 8..start + RECORD_SIZE].copy_from_slice(bytes);
    }
    let records_checksum = fnv1a(&wal[HEADER_SIZE..]);
    wal[40..48].copy_from_slice(&records_checksum.to_le_bytes());
    let header_checksum = fnv1a(&wal[..56]);
    wal[56..64].copy_from_slice(&header_checksum.to_le_bytes());
    wal
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
    name: String unique
    group: String index
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
    assert!(
        schema
            .fields
            .iter()
            .any(|field| field.name == "name" && field.unique)
    );
    assert_eq!(mir.structs.iter().filter(|item| item.data).count(), 1);
    assert!(mir.structs[0].fields.iter().any(|field| field.unique));
    assert!(mir.structs[0].fields.iter().any(|field| field.indexed));
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
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 4);
    assert_eq!(bytes.len() % 4096, 0);
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
fn legacy_snapshots_migrate_to_fixed_pages_in_both_engines() {
    let interpreted = data_path("legacy-interpreter");
    let native = data_path("legacy-native");
    fs::write(&interpreted, legacy_empty_snapshot()).unwrap();
    fs::write(&native, legacy_empty_snapshot()).unwrap();

    let migrate = |path: &std::path::Path| {
        let path = source_path(path);
        format!(
            r#"
data Item {{ id: int primary name: String }}
fn migrate() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    changed = data add Item {{ id: 1, name: "migrated" }} in store?
    return Ok(changed == 1)
}}
fn main() {{ match migrate() {{ Ok(value) => print(value), Err(error) => print(error) }} }}
"#
        )
    };

    assert_eq!(run_source(&migrate(&interpreted)).unwrap(), ["true"]);
    let interpreted_bytes = fs::read(&interpreted).unwrap();
    assert_eq!(
        u32::from_le_bytes(interpreted_bytes[8..12].try_into().unwrap()),
        4
    );
    assert_eq!(interpreted_bytes.len() % 4096, 0);

    if let Some(output) = native_output("legacy-migration", &migrate(&native)) {
        assert_eq!(output, "true\n");
        assert_eq!(interpreted_bytes, fs::read(&native).unwrap());
    }
}

#[test]
fn committed_wal_recovers_identically_in_interpreter_and_native() {
    let template = data_path("wal-template");
    let template_source = source_path(&template);
    let seed = format!(
        r#"
data Item {{ id: int primary name: String }}
fn seed() -> Result<bool, DataError> {{
    var store = data open Path("{template_source}")?
    changed = data add Item {{ id: 1, name: "before" }} in store?
    return Ok(changed == 1)
}}
fn main() {{ print(match seed() {{ Ok(value) => value, Err(_) => false }}) }}
"#
    );
    assert_eq!(run_source(&seed).unwrap(), ["true"]);
    let before = fs::read(&template).unwrap();

    let update = format!(
        r#"
data Item {{ id: int primary name: String }}
fn update() -> Result<bool, DataError> {{
    var store = data open Path("{template_source}")?
    changed = data save Item {{ id: 1, name: "after" }} in store?
    return Ok(changed == 1)
}}
fn main() {{ print(match update() {{ Ok(value) => value, Err(_) => false }}) }}
"#
    );
    assert_eq!(run_source(&update).unwrap(), ["true"]);
    let after = fs::read(&template).unwrap();
    let wal = committed_wal(&before, &after);

    let verify = |path: &std::path::Path| {
        let path = source_path(path);
        format!(
            r#"
data Item {{ id: int primary name: String }}
fn verify() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    rows = data find Item in store where name == "after"?
    return Ok(rows.len() == 1)
}}
fn main() {{ match verify() {{ Ok(value) => print(value), Err(error) => print(error) }} }}
"#
        )
    };

    let interpreted = data_path("wal-interpreter");
    fs::write(&interpreted, &before).unwrap();
    fs::write(format!("{}.wal", interpreted.display()), &wal).unwrap();
    assert_eq!(run_source(&verify(&interpreted)).unwrap(), ["true"]);
    assert_eq!(fs::read(&interpreted).unwrap(), after);
    assert!(!std::path::Path::new(&format!("{}.wal", interpreted.display())).exists());

    let native = data_path("wal-native");
    fs::write(&native, &before).unwrap();
    fs::write(format!("{}.wal", native.display()), &wal).unwrap();
    if let Some(output) = native_output("wal-recovery", &verify(&native)) {
        assert_eq!(output, "true\n");
        assert_eq!(fs::read(&native).unwrap(), after);
        assert!(!std::path::Path::new(&format!("{}.wal", native.display())).exists());
    }
}

#[test]
fn a_second_open_is_rejected_while_the_first_store_owns_the_lock() {
    let interpreted = source_path(&data_path("lock-interpreter"));
    let native = source_path(&data_path("lock-native"));
    let source = |path: &str| {
        format!(
            r#"
fn check() -> Result<bool, DataError> {{
    var first = data open Path("{path}")?
    second = data open Path("{path}")
    return Ok(match second {{ Err(_) => true, Ok(_) => false }})
}}
fn main() {{ print(match check() {{ Ok(value) => value, Err(_) => false }}) }}
"#
        )
    };

    assert_eq!(run_source(&source(&interpreted)).unwrap(), ["true"]);
    if let Some(output) = native_output("exclusive-data-lock", &source(&native)) {
        assert_eq!(output, "true\n");
    }
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

#[test]
fn unique_constraints_are_typed_durable_and_native() {
    let path = data_path("unique");
    let path = source_path(&path);
    let source = format!(
        r#"
data Account {{
    id: int primary
    email: String unique
}}

fn verify() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    data add Account {{ id: 1, email: "ada@disp.dev" }} in store?
    duplicate = data add Account {{ id: 2, email: "ada@disp.dev" }} in store
    data add Account {{ id: 2, email: "grace@disp.dev" }} in store?
    collision = data save Account {{ id: 2, email: "ada@disp.dev" }} in store
    rows = data find Account in store order id ascending?
    wanted = "grace@disp.dev"
    indexed = data find Account in store where email == wanted?
    removed = data remove Account in store where id == 1?
    data add Account {{ id: 3, email: "ada@disp.dev" }} in store?
    reused = data find Account in store where email == "ada@disp.dev"?
    return Ok(match duplicate {{ Err(_) => true, Ok(_) => false }}
        && match collision {{ Err(_) => true, Ok(_) => false }}
        && rows.len() == 2 && indexed.len() == 1
        && removed == 1 && reused.len() == 1)
}}

fn main() {{ print(match verify() {{ Ok(value) => value, Err(_) => false }}) }}
"#
    );
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(format!("{path}.lock"));
    if let Some(output) = native_output("unique-constraints", &source) {
        assert_eq!(output, "true\n");
    }
    let native_source = std::env::temp_dir().join(format!(
        "disp-data-index-structure-{}.disp",
        std::process::id()
    ));
    fs::write(&native_source, &source).unwrap();
    let (hir, mir) = lower_source(&source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &native_source,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    assert!(generated.contains("disp_data_indexes_rebuild"));
    assert!(generated.contains("disp_data_index_find"));
    assert!(generated.matches("disp_data_native_lookup(").count() >= 2);

    let optional =
        check_source("data Account { id: int primary email: Option<String> unique } fn main() {}")
            .unwrap_err();
    assert!(
        optional.message.contains("cannot be optional"),
        "{optional}"
    );
    assert_eq!(
        (optional.span.start.line, optional.span.start.column),
        (1, 39)
    );

    let duplicate =
        check_source("data Account { id: int primary email: String unique unique } fn main() {}")
            .unwrap_err();
    assert!(
        duplicate.message.contains("duplicate `unique`"),
        "{duplicate}"
    );
    assert_eq!(
        (duplicate.span.start.line, duplicate.span.start.column),
        (1, 53)
    );
}

#[test]
fn secondary_indexes_allow_duplicate_keys_and_track_mutations() {
    let path = data_path("secondary-index");
    let path = source_path(&path);
    let source = format!(
        r#"
data Event {{
    id: int primary
    category: String index
    message: String
}}

fn seed() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    data add Event {{ id: 1, category: "system", message: "boot" }} in store?
    data add Event {{ id: 2, category: "system", message: "ready" }} in store?
    data add Event {{ id: 3, category: "user", message: "login" }} in store?
    wanted = "system"
    before = data find Event in store where category == wanted order id ascending?
    data remove Event in store where id == 1?
    data save Event {{ id: 3, category: "system", message: "login" }} in store?
    after = data find Event in store where category == wanted order id ascending?
    literal = data find Event in store where category == "system" order id ascending?
    return Ok(before.len() == 2 && after.len() == 2 && literal.len() == 2)
}}

fn reopen() -> Result<bool, DataError> {{
    var store = data open Path("{path}")?
    wanted = "system"
    rows = data find Event in store where category == wanted order id ascending?
    return Ok(rows.len() == 2)
}}

fn main() {{
    seeded = match seed() {{ Ok(value) => value, Err(_) => false }}
    reopened = match reopen() {{ Ok(value) => value, Err(_) => false }}
    print(seeded && reopened)
}}
"#
    );
    assert_eq!(run_source(&source).unwrap(), ["true"]);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 4);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(format!("{path}.lock"));
    if let Some(output) = native_output("secondary-index", &source) {
        assert_eq!(output, "true\n");
    }
    let plan_source = std::env::temp_dir().join(format!(
        "disp-data-secondary-index-plan-{}.disp",
        std::process::id()
    ));
    fs::write(&plan_source, &source).unwrap();
    let (hir, mir) = lower_source(&source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &plan_source,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    assert!(generated.matches("disp_data_native_lookup(").count() >= 4);

    let duplicate =
        check_source("data Event { id: int primary category: String index index } fn main() {}")
            .unwrap_err();
    assert!(
        duplicate.message.contains("duplicate `index`"),
        "{duplicate}"
    );
    assert_eq!(
        (duplicate.span.start.line, duplicate.span.start.column),
        (1, 53)
    );
}
