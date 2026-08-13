use disp::{
    backend::{self, BuildOptions, abi, layout::LayoutEngine, target::Target},
    check_source, lower_source, run_source,
};
use std::{fs, process::Command};

fn differential(name: &str, source: &str) {
    let expected = run_source(source).unwrap().join("\n") + "\n";
    let path = std::env::temp_dir().join(format!("disp-database-{name}.disp"));
    fs::write(&path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifact = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap();
    let output = match Command::new(artifact.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => return,
        Err(error) => panic!("native execution failed: {error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        expected
    );
}

#[test]
fn sqlite_bound_parameters_queries_and_transactions_are_differential() {
    let source = r#"
fn inspect() -> Result<bool, DataError> {
    var db=Database.memory()?
    var empty: List<Json> = List.new()
    db.execute("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, active INTEGER)", empty)?
    tricky=match Json.string("Ada'); DROP TABLE users; --") { Ok(value)=>value, Err(error)=>Json.null() }
    first=List.of(tricky, Json.bool(true))
    inserted=db.execute("INSERT INTO users(name,active) VALUES(?,?)", first)?
    db.begin()?
    second=List.of(match Json.string("rolled back") { Ok(value)=>value, Err(error)=>Json.null() }, Json.bool(false))
    changed=db.execute("INSERT INTO users(name,active) VALUES(?,?)", second)?
    db.rollback()?
    db.execute("BEGIN", empty)?
    db.rollback()?
    db.begin()?
    db.execute("COMMIT", empty)?
    raw_closed=match db.rollback() { Ok(value)=>false, Err(error)=>true }
    missing=match db.commit() { Ok(value)=>false, Err(error)=>true }
    db.begin()?
    nested=match db.begin() { Ok(value)=>false, Err(error)=>true }
    durable=List.of(match Json.string("Grace") { Ok(value)=>value, Err(error)=>Json.null() }, Json.bool(true))
    committed=db.execute("INSERT INTO users(name,active) VALUES(?,?)", durable)?
    db.commit()?
    selected=List.of(Json.int(0))
    rows=db.query("SELECT id,name,active FROM users WHERE id>?", selected)?
    valid=inserted==1
    valid=valid && changed==1
    valid=valid && committed==1
    valid=valid && missing
    valid=valid && raw_closed
    valid=valid && nested
    valid=valid && rows.len()==2
    valid=valid && db.changes()==1
    valid=valid && db.last_insert_id()==2
    let readonly=Database.memory()?
    valid=valid && readonly.changes()==0
    return Ok(valid && readonly.last_insert_id()==0)
}

fn main(){ print(match inspect(){Ok(value)=>value,Err(error)=>false}) }
"#;
    differential("sqlite-core", source);
}

#[test]
fn file_database_drop_rolls_back_and_closes_deterministically() {
    let path = std::env::temp_dir().join(format!(
        "disp-database-drop-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_file(&path);
    let database_path = path.to_string_lossy().replace('\\', "/");
    let source = format!(
        r#"
fn abandon() -> Result<bool,DataError> {{
    var db=Database.open(Path("{database_path}"))?
    var empty: List<Json> = List.new()
    db.execute("DROP TABLE IF EXISTS events",empty)?
    db.execute("CREATE TABLE events(value INTEGER)",empty)?
    db.begin()?
    db.execute("INSERT INTO events(value) VALUES(7)",empty)?
    return Ok(true)
}}
fn inspect() -> Result<bool,DataError> {{
    abandoned=abandon()?
    var db=Database.open(Path("{database_path}"))?
    var empty: List<Json> = List.new()
    rows=db.query("SELECT value FROM events",empty)?
    db.close()?
    return Ok(abandoned && rows.is_empty())
}}
fn main(){{ print(match inspect(){{Ok(value)=>value,Err(error)=>false}}) }}
"#
    );
    differential("drop-rollback", &source);
    let _ = fs::remove_file(path);
}

#[test]
fn sqlite_failures_are_typed_bounded_and_do_not_guess() {
    let source = r#"
fn inspect() -> Result<bool, DataError> {
    var db=Database.memory()?
    var empty: List<Json> = List.new()
    multi=match db.execute("CREATE TABLE one(id); CREATE TABLE two(id)", empty) { Ok(value)=>false, Err(error)=>true }
    wrong=match db.execute("CREATE TABLE one(id)", List.of(Json.int(1))) { Ok(value)=>false, Err(error)=>true }
    db.execute("CREATE TABLE blobs(value BLOB)", empty)?
    db.execute("INSERT INTO blobs(value) VALUES(X'00')", empty)?
    blob=match db.query("SELECT value FROM blobs", empty) { Ok(rows)=>false, Err(error)=>true }
    rows=match db.execute("SELECT value FROM blobs", empty) { Ok(value)=>false, Err(error)=>true }
    duplicate=match db.query("SELECT 1 AS value, 2 AS value", empty) { Ok(value)=>false, Err(error)=>true }
    db.execute("CREATE TABLE parents(id INTEGER PRIMARY KEY)", empty)?
    db.execute("CREATE TABLE children(parent_id INTEGER REFERENCES parents(id))", empty)?
    foreign=match db.execute("INSERT INTO children(parent_id) VALUES(1)", empty) { Ok(value)=>false, Err(error)=>true }
    valid=multi && wrong
    valid=valid && blob
    valid=valid && rows
    valid=valid && duplicate
    return Ok(valid && foreign)
}
fn main(){ print(match inspect(){Ok(value)=>value,Err(error)=>false}) }
"#;
    differential("sqlite-errors", source);

    let immutable = check_source(
        "fn inspect()->Result<Unit,DataError>{ let db=Database.memory()?; return db.begin() } fn main(){}",
    )
    .unwrap_err();
    assert!(immutable.message.contains("mutable"), "{immutable}");
    assert_eq!(
        (immutable.span.start.line, immutable.span.start.column),
        (1, 73)
    );

    let parameters = check_source(
        "fn main(){ var db=Database.memory(); match db { Ok(value)=>value.query(\"SELECT ?\",List.of(1)), Err(error)=>Err(error) } }",
    )
    .unwrap_err();
    assert!(parameters.message.contains("List<Json>"), "{parameters}");
    assert_eq!(
        (parameters.span.start.line, parameters.span.start.column),
        (1, 83)
    );

    let moved = check_source(
        "fn use_db()->Result<Unit,DataError>{ var db=Database.memory()?; db.close()?; return db.begin() } fn main(){}",
    )
    .unwrap_err();
    assert!(moved.message.contains("moved"), "{moved}");
    assert_eq!((moved.span.start.line, moved.span.start.column), (1, 85));
}

#[test]
fn database_reaches_hir_mir_layout_and_native_abi() {
    let source = r#"
fn work()->Result<Unit,DataError>{
    var db=Database.memory()?
    var empty: List<Json> = List.new()
    changed=db.execute("CREATE TABLE values_table(value INTEGER)",empty)?
    rows=db.query("SELECT value FROM values_table",empty)?
    return db.close()
}
fn main(){}
"#;
    let (hir, mir) = lower_source(source).unwrap();
    let hir_text = format!("{:?}", hir.functions);
    for name in [
        "Database.memory",
        "Database.execute",
        "Database.query",
        "Database.close",
    ] {
        assert!(hir_text.contains(name), "missing {name}");
    }
    let mir_text = disp::mir::dump(&mir);
    for name in [
        "Database.memory",
        "Database.execute",
        "Database.query",
        "Database.close",
    ] {
        assert!(mir_text.contains(name), "missing {name}");
    }
    let target = Target::host().unwrap();
    let mut layouts = LayoutEngine::new(target, &hir);
    let layout = layouts.layout(&disp::hir::Type::Database).unwrap();
    assert_eq!((layout.size, layout.align), (8, 8));
    assert_eq!(
        abi::classify(&disp::hir::Type::Database, &layout, target),
        abi::PassMode::Direct
    );
}
