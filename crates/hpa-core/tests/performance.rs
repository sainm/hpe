use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use hpa_core::compiler::compile_workflow;
use hpa_core::ir::Workflow;
use hpa_core::runtime::{ExecutionStatus, new_state, step};
use serde_json::{Value, json};

fn build_workflow(node_count: usize) -> Value {
    let mut nodes = Vec::with_capacity(node_count);
    let mut edges = Vec::with_capacity(node_count.saturating_sub(1));
    nodes.push(json!({"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}}));
    let transforms = node_count.saturating_sub(2);
    for index in 0..transforms {
        nodes.push(json!({
            "id": format!("t{index}"),
            "type": "transform",
            "typeVersion": "1.0.0",
            "config": {"mapping": {"value": index}},
        }));
    }
    nodes.push(json!({
        "id": "end",
        "type": "end",
        "typeVersion": "1.0.0",
        "config": {"output": {"$expr": format!(".nodes.t{}", transforms.saturating_sub(1))}},
    }));
    let mut previous = "start".to_string();
    for index in 0..transforms {
        let id = format!("t{index}");
        edges.push(json!({"id": format!("e{index}"), "source": previous, "target": id}));
        previous = id;
    }
    edges.push(json!({
        "id": format!("e{transforms}"),
        "source": previous,
        "target": "end",
    }));
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_perf_100",
        "name": "100 Node Benchmark",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": nodes,
        "edges": edges,
    })
}

#[test]
fn hundred_node_workflow_compiles_and_executes() {
    let value = build_workflow(100);
    let text = serde_json::to_string(&value).expect("serialize workflow");
    let workflow = Workflow::from_json_str(&text).expect("parse workflow");
    assert_eq!(workflow.nodes.len(), 100);

    let compile_started = Instant::now();
    let compilation = compile_workflow(&workflow).expect("compile workflow");
    let compile_elapsed = compile_started.elapsed();
    assert!(compilation.issues.is_empty(), "{:?}", compilation.issues);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let execution_started = Instant::now();
    let (status, steps) = runtime.block_on(async {
        let mut state = new_state(
            &workflow,
            &compilation.plan,
            "exec_perf_100",
            json!({}),
            BTreeMap::new(),
            "local",
        )
        .expect("execution state");
        let mut steps = 0u32;
        for _ in 0..1_000 {
            if state.status.is_terminal() {
                break;
            }
            let _ = step(&workflow, &compilation.plan, &mut state).await;
            steps += 1;
        }
        (state.status, steps)
    });
    let execution_elapsed = execution_started.elapsed();

    assert_eq!(status, ExecutionStatus::Succeeded);
    assert_eq!(steps, 101, "one step per node in a straight-line graph");
    eprintln!(
        "100-node benchmark: compile {compile_elapsed:?}, execute {execution_elapsed:?} ({steps} steps)"
    );
    assert!(
        compile_elapsed < Duration::from_secs(2),
        "compile took {compile_elapsed:?}"
    );
    assert!(
        execution_elapsed < Duration::from_secs(5),
        "execution took {execution_elapsed:?}"
    );
}
