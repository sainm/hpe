use hpa_core::compiler;
use hpa_core::expression::{self, ExpressionScope};
use hpa_core::ir::Workflow;
use hpa_core::validator;
use proptest::prelude::*;
use serde_json::{Value, json};

fn scalar_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        2 => Just(Value::Null),
        2 => any::<bool>().prop_map(Value::Bool),
        3 => any::<i64>().prop_map(Value::from),
        5 => "[ -~]{0,64}".prop_map(Value::String),
    ]
}

proptest! {
    #[test]
    fn yaml_roundtrip_preserves_generated_json_maps(
        entries in prop::collection::btree_map("[A-Za-z][A-Za-z0-9_]{0,12}", scalar_value(), 0..24)
    ) {
        let value = Value::Object(entries.into_iter().collect());
        let yaml = hpa_core::yaml::json_to_yaml(&value).expect("emit yaml");
        let decoded = hpa_core::yaml::yaml_to_json(&yaml).expect("parse yaml");
        prop_assert_eq!(decoded, value);
    }
}

const FIXTURES: &[(&str, &str)] = &[
    (
        "01_sequence.json",
        include_str!("../../../fixtures/01_sequence.json"),
    ),
    (
        "02_condition.json",
        include_str!("../../../fixtures/02_condition.json"),
    ),
    (
        "03_switch.json",
        include_str!("../../../fixtures/03_switch.json"),
    ),
    (
        "04_loop.json",
        include_str!("../../../fixtures/04_loop.json"),
    ),
    (
        "05_parallel.json",
        include_str!("../../../fixtures/05_parallel.json"),
    ),
    (
        "06_mapping.json",
        include_str!("../../../fixtures/06_mapping.json"),
    ),
    (
        "07_retry_error.json",
        include_str!("../../../fixtures/07_retry_error.json"),
    ),
    (
        "08_approval.json",
        include_str!("../../../fixtures/08_approval.json"),
    ),
    (
        "09_subflow.json",
        include_str!("../../../fixtures/09_subflow.json"),
    ),
    (
        "10_layout_unicode.json",
        include_str!("../../../fixtures/10_layout_unicode.json"),
    ),
];

const SEQUENCE_YAML: &str = include_str!("../../../fixtures/01_sequence.yaml");

fn fixture_value(name: &str) -> Value {
    let (_, text) = FIXTURES
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .unwrap_or_else(|| panic!("missing fixture {name}"));
    serde_json::from_str(text).expect("fixture JSON")
}

#[test]
fn all_fixtures_validate() {
    for (name, text) in FIXTURES {
        let compilation = validator::check_json(text)
            .unwrap_or_else(|report| panic!("{name} failed validation: {:?}", report.issues));
        assert!(!compilation.plan.graphs.is_empty(), "{name} has no graphs");
        assert!(
            compilation
                .issues
                .iter()
                .all(|issue| issue.severity == hpa_core::error::Severity::Warning),
            "{name} has unexpected errors"
        );
    }
}

#[test]
fn yaml_and_json_are_semantically_equal() {
    let json_workflow = Workflow::from_json_str(FIXTURES[0].1).expect("JSON workflow");
    let yaml_workflow = Workflow::from_yaml_str(SEQUENCE_YAML).expect("YAML workflow");
    assert_eq!(
        json_workflow.canonical_value().expect("canonical JSON"),
        yaml_workflow.canonical_value().expect("canonical YAML")
    );
}

#[test]
fn json_yaml_json_roundtrip_preserves_semantics() {
    for (name, text) in FIXTURES {
        let workflow = Workflow::from_json_str(text).expect("fixture workflow");
        let yaml = workflow.to_yaml_string().expect("emit YAML");
        let roundtrip = Workflow::from_yaml_str(&yaml)
            .unwrap_or_else(|error| panic!("{name} YAML parse failed: {error}\n{yaml}"));
        assert_eq!(
            workflow.canonical_value().expect("canonical source"),
            roundtrip.canonical_value().expect("canonical roundtrip"),
            "{name} semantic roundtrip changed"
        );
    }
}

#[test]
fn compiled_plan_exposes_container_graphs() {
    let loop_compilation =
        validator::check_json(FIXTURES[3].1).expect("loop fixture should compile");
    assert!(
        loop_compilation
            .plan
            .graphs
            .contains_key(&compiler::loop_graph_id("loop"))
    );

    let parallel_compilation =
        validator::check_json(FIXTURES[4].1).expect("parallel fixture should compile");
    assert!(
        parallel_compilation
            .plan
            .graphs
            .contains_key(&compiler::parallel_graph_id("parallel", "left"))
    );

    let subflow_compilation =
        validator::check_json(FIXTURES[8].1).expect("subflow fixture should compile");
    assert!(
        subflow_compilation
            .plan
            .graph(&compiler::subflow_graph_id("sub_double"))
            .is_some()
    );
}

#[test]
fn expressions_evaluate_context() {
    let context = json!({
        "input": {"value": 41},
        "vars": {"suffix": "!"},
        "nodes": {},
        "env": {},
        "execution": {"id": "exec_test", "now": "2026-11-01T00:00:00Z", "tenantId": "local"},
        "loop": null,
        "parallel": null
    });
    assert_eq!(
        expression::evaluate(".input.value + 1", &context).expect("expression"),
        json!(42)
    );
    assert_eq!(
        expression::render_template("value=${.input.value}${.vars.suffix}", &context)
            .expect("template"),
        "value=41!"
    );
    assert!(expression::evaluate_bool(".input.value > 10", &context).expect("condition"));
}

#[test]
fn expressions_are_checked_for_forbidden_builtins_and_references() {
    let scope = ExpressionScope::new(
        vec!["value".to_string()],
        vec!["suffix".to_string()],
        vec!["map".to_string()],
    );
    assert!(expression::check_expression(".input.value + 1", &scope).is_ok());
    assert!(expression::check_template("x=${.vars.suffix}", &scope).is_ok());
    assert!(expression::check_expression("now", &scope).is_err());
    assert!(expression::check_expression("env", &scope).is_err());
    assert!(expression::check_expression(".nodes.missing.output", &scope).is_err());
    assert!(expression::check_expression(".input.missing", &scope).is_err());
    assert!(expression::check_expression(".loop.item", &scope).is_err());
}

#[test]
fn invalid_graphs_are_rejected() {
    let mut duplicate = fixture_value("01_sequence.json");
    duplicate["nodes"][1]["id"] = json!("start");
    assert!(
        validator::validate_value(&duplicate).has_code("DUP_NODE_ID"),
        "duplicate node id must be rejected"
    );

    let mut dangling = fixture_value("01_sequence.json");
    dangling["edges"][1]["target"] = json!("ghost");
    assert!(
        validator::validate_value(&dangling).has_code("EDGE_DANGLING"),
        "dangling edge must be rejected"
    );

    let mut cycle = fixture_value("01_sequence.json");
    cycle["edges"] = json!([
        {"id": "e1", "source": "start", "target": "map"},
        {"id": "e2", "source": "map", "target": "start"},
        {"id": "e3", "source": "map", "target": "end"}
    ]);
    assert!(
        validator::validate_value(&cycle).has_code("GRAPH_CYCLE"),
        "cycle must be rejected"
    );

    let mut unknown_reference = fixture_value("01_sequence.json");
    unknown_reference["nodes"][1]["config"]["mapping"]["value"] =
        json!({"$expr": ".nodes.ghost.output"});
    assert!(
        validator::validate_value(&unknown_reference).has_code("EXPR_INVALID"),
        "unknown node reference must be rejected"
    );

    let mut missing_subflow = fixture_value("09_subflow.json");
    missing_subflow["nodes"][1]["config"]["subflowId"] = json!("missing");
    assert!(
        validator::validate_value(&missing_subflow).has_code("SUBFLOW_MISSING"),
        "missing subflow must be rejected"
    );

    let mut no_default = fixture_value("03_switch.json");
    no_default["nodes"][1]["config"]
        .as_object_mut()
        .expect("switch config")
        .remove("defaultPort");
    assert!(
        validator::validate_value(&no_default).has_code("SWITCH_DEFAULT"),
        "switch without default must be rejected"
    );
}

#[test]
fn unicode_layout_survives_yaml_roundtrip() {
    let workflow = Workflow::from_json_str(FIXTURES[9].1).expect("unicode workflow");
    let yaml = workflow.to_yaml_string().expect("emit YAML");
    assert!(yaml.contains("挨拶"));
    let roundtrip = Workflow::from_yaml_str(&yaml).expect("parse YAML");
    assert_eq!(
        workflow.canonical_value().expect("canonical"),
        roundtrip.canonical_value().expect("canonical")
    );
}
