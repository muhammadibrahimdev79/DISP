use disp::{cfg::ControlFlowGraph, hir, lower_source, mir};

fn lower(source: &str) -> (hir::Program, mir::Program) {
    lower_source(source).unwrap()
}

fn main_mir(program: &mir::Program) -> &mir::Function {
    program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .unwrap()
}

fn statements(function: &mir::Function) -> impl Iterator<Item = &mir::StatementKind> {
    function
        .blocks
        .iter()
        .flat_map(|block| block.statements.iter().map(|statement| &statement.kind))
}

#[test]
fn hir_uses_stable_ids_types_spans_and_resolved_calls() {
    let source = r#"
struct Point { x: int, y: int }
fn add(value: int) -> int { return value + 1 }
fn main() { let point = Point { x: 2, y: 3 } print(add(point.x)) }
"#;
    let (hir, _) = lower(source);
    assert_eq!(hir.structs[0].id, hir::StructId(0));
    assert_eq!(hir.structs[0].fields[0].index, 0);
    assert!(
        hir.functions
            .iter()
            .enumerate()
            .all(|(index, function)| function.id.0 == index)
    );
    assert!(
        hir.functions
            .iter()
            .flat_map(|function| &function.locals)
            .all(|local| local.span.start.line > 0)
    );
    let dump = hir::dump(&hir);
    assert!(dump.contains("fn0 add") && dump.contains("Struct(StructId(0)"));
}

#[test]
fn mir_distinguishes_moves_copies_borrows_and_raw_dereferences() {
    let source = r#"
struct Boxed { text: String }
fn raw(value: ptr<int>) -> int { unsafe { return *value } }
fn main() {
    let number = 4
    let copied = number
    let boxed = Boxed { text: "x" }
    let moved = boxed
    let shared = &copied
    print(*shared)
    print(moved.text)
}
"#;
    let (_, mir) = lower(source);
    let main = main_mir(&mir);
    let dump = mir::dump(&mir);
    assert!(dump.contains("Copy(Place"));
    assert!(dump.contains("Move(Place"));
    assert!(dump.contains("BorrowShared"));
    assert!(
        mir::moved_places(main)
            .iter()
            .any(|place| place.projections.is_empty())
    );
    assert!(dump.contains("Dereference"));
}

#[test]
fn if_else_and_nested_control_flow_form_real_branches() {
    let source = r#"fn main() { var x = 0 if true { if false { x = 1 } else { x = 2 } } else { x = 3 } print(x) }"#;
    let (_, mir) = lower(source);
    let function = main_mir(&mir);
    assert!(
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, mir::Terminator::SwitchBool { .. }))
            .count()
            >= 2
    );
    let cfg = ControlFlowGraph::new(function);
    assert!(cfg.reachable().len() >= 5);
    assert_eq!(cfg.reverse_postorder().first(), Some(&mir::BlockId(0)));
}

#[test]
fn while_loop_for_break_and_continue_create_back_edges_and_targets() {
    let source = r#"
fn main() {
    var n = 0
    while n < 3 { n += 1 if n == 2 { continue } }
    for i in 0..4 { if i == 2 { break } }
    loop { break }
    print(n)
}
"#;
    let (_, mir) = lower(source);
    let function = main_mir(&mir);
    let cfg = ControlFlowGraph::new(function);
    assert!(cfg.has_back_edge());
    assert!(
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, mir::Terminator::Goto(_)))
            .count()
            >= 6
    );
    assert!(!cfg.predecessors(mir::BlockId(1)).is_empty());
}

#[test]
fn match_is_lowered_to_discriminant_switch_and_payload_projection() {
    let source = r#"
enum Choice { Number(int), Empty }
fn main() {
    let choice = Choice.Number(7)
    let value = match choice { Number(number) => number, Empty => 0 }
    print(value)
}
"#;
    let (_, mir) = lower(source);
    let function = main_mir(&mir);
    assert!(
        function
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, mir::Terminator::SwitchEnum { .. }))
    );
    assert!(format!("{:?}", function.blocks).contains("VariantField"));
}

#[test]
fn option_result_and_question_create_explicit_early_return_cfg() {
    let source = r#"
fn required(value: Option<int>) -> Result<int, String> {
    return match value { Some(number) => Ok(number), None => Err("missing") }
}
fn pass(value: Option<int>) -> Result<int, String> { return Ok(required(value)?) }
fn main() { print(pass(Some(3))) }
"#;
    let (_, mir) = lower(source);
    let pass = mir
        .functions
        .iter()
        .find(|function| function.name == "pass")
        .unwrap();
    assert!(
        pass.blocks
            .iter()
            .any(|block| matches!(block.terminator, mir::Terminator::SwitchEnum { .. }))
    );
    assert!(
        pass.blocks
            .iter()
            .filter(|block| matches!(block.terminator, mir::Terminator::Return))
            .count()
            >= 2
    );
}

#[test]
fn generic_and_trait_calls_are_concrete_before_mir() {
    let source = r#"
struct Counter { value: int }
trait Read { fn read(&self) -> int }
impl Read for Counter { fn read(&self) -> int { return self.value } }
fn identity<T>(value: T) -> T { return value }
fn main() { let counter = Counter { value: 5 } print(identity(counter.read())) }
"#;
    let (hir, mir) = lower(source);
    let main = hir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .unwrap();
    let text = format!("{:?}", main.body);
    assert!(text.contains("receiver: Some(Shared)"));
    assert!(text.contains("substitutions: [Int"));
    assert!(
        main_mir(&mir)
            .blocks
            .iter()
            .any(|block| matches!(block.terminator, mir::Terminator::Call { .. }))
    );
}

#[test]
fn drop_flags_cover_partial_moves_reinitialization_and_return_cleanup() {
    let source = r#"
struct Pair { left: String, right: String }
fn take() -> String {
    var pair = Pair { left: "a", right: "b" }
    let left = move pair.left
    pair.left = "c"
    return left
}
fn main() { print(take()) }
"#;
    let (_, mir) = lower(source);
    let take = mir
        .functions
        .iter()
        .find(|function| function.name == "take")
        .unwrap();
    assert!(
        take.locals
            .iter()
            .any(|local| local.kind == mir::LocalKind::DropFlag)
    );
    assert!(statements(take).any(|statement| matches!(
        statement,
        mir::StatementKind::SetDropFlag {
            initialized: false,
            ..
        }
    )));
    let dropped_fields = statements(take)
        .filter_map(|statement| match statement {
            mir::StatementKind::Drop { place, .. } => place.projections.last(),
            _ => None,
        })
        .filter_map(|projection| match projection {
            mir::Projection::Field(index) => Some(*index),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    assert!(dropped_fields.contains(&0) && dropped_fields.contains(&1));
    assert!(
        statements(take)
            .any(|statement| matches!(statement, mir::StatementKind::Drop { flag: Some(_), .. }))
    );
    for block in &take.blocks {
        if matches!(block.terminator, mir::Terminator::Return) {
            let return_position = block.statements.len();
            assert!(
                block.statements[..return_position]
                    .iter()
                    .any(|statement| matches!(statement.kind, mir::StatementKind::Drop { .. }))
            );
        }
    }
}

#[test]
fn cfg_reports_successors_predecessors_reachability_and_unreachable_blocks() {
    let (_, mut mir) = lower("fn main() { if true { print(1) } else { print(2) } }");
    let function = mir
        .functions
        .iter_mut()
        .find(|function| function.name == "main")
        .unwrap();
    function.blocks.push(mir::BasicBlock {
        statements: vec![],
        terminator: mir::Terminator::Return,
    });
    let cfg = ControlFlowGraph::new(function);
    assert_eq!(cfg.successors(mir::BlockId(0)).len(), 2);
    assert!(!cfg.predecessors(mir::BlockId(1)).is_empty());
    assert_eq!(
        cfg.unreachable(),
        vec![mir::BlockId(function.blocks.len() - 1)]
    );
}

#[test]
fn validation_returns_controlled_internal_diagnostics() {
    let (_, mut mir) = lower("fn main() { print(1) }");
    mir.functions[0].blocks[0].terminator = mir::Terminator::Goto(mir::BlockId(999));
    let error = mir::validate(&mir).unwrap_err();
    assert_eq!(error.kind, disp::diagnostics::DiagnosticKind::Internal);
    assert!(error.message.contains("missing bb999"));
}

#[test]
fn dumps_are_deterministic_and_include_source_locations() {
    let (hir, mir) = lower("fn main() { let x = 1 print(x) }");
    assert_eq!(hir::dump(&hir), hir::dump(&hir));
    assert_eq!(mir::dump(&mir), mir::dump(&mir));
    assert!(hir::dump(&hir).contains("@ 1:1"));
    assert!(mir::dump(&mir).contains("bb0:"));
}
