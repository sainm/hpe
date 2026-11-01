use hpa_core::ir::{Node, Workflow};
use hpa_core::registry::{self, NetworkPolicy};
use hpa_core::runtime::ApprovalRequest;
use hpa_core::validator;
use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn http_workflow(url: &str, permissions: Option<serde_json::Value>) -> serde_json::Value {
    let mut node = json!({
        "id": "fetch",
        "type": "http.request",
        "typeVersion": "1.0.0",
        "config": { "method": "GET", "url": url }
    });
    if let Some(permissions) = permissions {
        node["permissions"] = permissions;
    }
    json!({
        "$schema": "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
        "schemaVersion": "hpa.dev/v1",
        "id": "wf_phase1_http",
        "name": "Phase 1 HTTP",
        "workflowVersion": "1.0.0",
        "inputs": [],
        "outputs": [],
        "variables": [],
        "nodes": [
            {"id": "start", "type": "start", "typeVersion": "1.0.0", "config": {}},
            node,
            {"id": "end", "type": "end", "typeVersion": "1.0.0", "config": {}}
        ],
        "edges": [
            {"id": "e1", "source": "start", "target": "fetch"},
            {"id": "e2", "source": "fetch", "target": "end"}
        ]
    })
}

#[test]
fn registry_normalizes_versions() {
    let resolved = registry::resolve_version("transform", "1.0").expect("version");
    assert_eq!(resolved.version, "1.0.0");
    assert!(!resolved.deprecated);
    assert!(registry::resolve_version("transform", "9.0.0").is_err());
    assert!(registry::resolve_version("unknown.node", "1.0.0").is_err());
}

#[test]
fn registry_migrates_legacy_http_node() {
    let node: Node = serde_json::from_value(json!({
        "id": "legacy_fetch",
        "type": "http.request",
        "typeVersion": "0.9.0",
        "config": {"uri": "https://api.example.com/orders", "verb": "POST"}
    }))
    .expect("legacy node");
    let migrated = registry::migrate_node(&node).expect("migration");
    assert_eq!(migrated.type_version, "1.0.0");
    assert_eq!(
        migrated.config["url"],
        json!("https://api.example.com/orders")
    );
    assert_eq!(migrated.config["method"], json!("POST"));
    assert!(migrated.config.get("uri").is_none());
}

#[test]
fn network_policy_defaults_to_loopback() {
    let policy = NetworkPolicy::from_permissions(None).expect("policy");
    assert!(policy.allows("127.0.0.1", 8080));
    assert!(policy.allows("localhost", 80));
    assert!(!policy.allows("api.example.com", 443));
    assert!(policy.allows_private_resolution("127.0.0.1", 8080));
    assert!(!policy.allows_private_resolution("localhost.example.com", 80));

    let explicit = NetworkPolicy::from_permissions(Some(&json!({
        "network": ["api.example.com:443", "*.trusted.example", "127.0.0.1:9000"]
    })))
    .expect("policy");
    assert!(explicit.allows("api.example.com", 443));
    assert!(!explicit.allows("api.example.com", 80));
    assert!(explicit.allows("v1.trusted.example", 8443));
    assert!(!explicit.allows("trusted.example", 443));
    assert!(explicit.allows("127.0.0.1", 9000));
    assert!(explicit.allows_private_resolution("127.0.0.1", 9000));
}

#[test]
fn invalid_network_rules_are_rejected() {
    assert!(registry::parse_network_rule("api.example.com:abc").is_err());
    assert!(registry::parse_network_rule("").is_err());
    assert!(registry::parse_network_rule("[::1]:8080").is_ok());
}

#[test]
fn external_literal_url_requires_network_permission() {
    let denied = validator::validate_value(&http_workflow("https://api.example.com/orders", None));
    assert!(denied.has_code("PERMISSION_NETWORK"), "{:?}", denied.issues);

    let allowed = validator::validate_value(&http_workflow(
        "https://api.example.com/orders",
        Some(json!({"network": ["api.example.com"]})),
    ));
    assert!(allowed.is_valid(), "{:?}", allowed.issues);
}

#[test]
fn unsupported_permissions_and_versions_are_rejected() {
    let filesystem = validator::validate_value(&http_workflow(
        "https://api.example.com/orders",
        Some(json!({"network": ["api.example.com"], "filesystem": {"read": ["./x"]}})),
    ));
    assert!(
        filesystem.has_code("PERMISSION_UNSUPPORTED"),
        "{:?}",
        filesystem.issues
    );

    let mut workflow = http_workflow("http://127.0.0.1:8080/", None);
    workflow["nodes"][1]["typeVersion"] = json!("9.9.9");
    let version = validator::validate_value(&workflow);
    assert!(version.has_code("NODE_VERSION"), "{:?}", version.issues);

    workflow["nodes"][1]["typeVersion"] = json!("1.0.0");
    workflow["nodes"][1]["config"] = json!({"method": "GET"});
    let missing_url = validator::validate_value(&workflow);
    assert!(
        missing_url.has_code("NODE_CONFIG_SCHEMA"),
        "{:?}",
        missing_url.issues
    );
}

#[test]
fn approval_sla_is_computed_from_request_time() {
    let now = OffsetDateTime::parse("2026-11-01T00:00:10Z", &Rfc3339).expect("now");
    let request = ApprovalRequest {
        id: "appr_1".to_string(),
        node_id: "approve".to_string(),
        graph_id: "root".to_string(),
        title: "Approve".to_string(),
        form_schema: None,
        assignees: Vec::new(),
        sla_ms: Some(5_000),
        on_timeout: "reject".to_string(),
        requested_at: "2026-11-01T00:00:00Z".to_string(),
    };
    assert!(request.is_overdue(now));
    let fresh = ApprovalRequest {
        requested_at: "2026-11-01T00:00:08Z".to_string(),
        ..request
    };
    assert!(!fresh.is_overdue(now));
}

#[test]
fn execution_policies_are_validated() {
    fn policy_codes(value: &serde_json::Value) -> Vec<String> {
        let workflow = Workflow::from_value(value).expect("workflow should parse");
        match hpa_core::compiler::compile_workflow(&workflow) {
            Ok(_) => Vec::new(),
            Err(issues) => issues.into_iter().map(|issue| issue.code).collect(),
        }
    }

    let mut workflow = http_workflow("http://127.0.0.1:8080/", None);
    workflow["nodes"][1]["timeoutMs"] = json!(0);
    let report = validator::validate_value(&workflow);
    assert!(report.has_code("SCHEMA_INVALID"), "{:?}", report.issues);
    assert!(
        policy_codes(&workflow)
            .iter()
            .any(|code| code == "POLICY_TIMEOUT")
    );

    let mut workflow = http_workflow("http://127.0.0.1:8080/", None);
    workflow["nodes"][1]["retry"] =
        json!({"maxAttempts": 0, "backoff": {"baseMs": 900, "maxMs": 100}});
    let codes = policy_codes(&workflow);
    assert!(codes.iter().any(|code| code == "POLICY_RETRY"), "{codes:?}");

    let mut workflow = http_workflow("http://127.0.0.1:8080/", None);
    workflow["nodes"][1]["errorPolicy"] = json!({"mode": "compensate"});
    assert!(
        policy_codes(&workflow)
            .iter()
            .any(|code| code == "POLICY_UNSUPPORTED")
    );

    let mut workflow = http_workflow("http://127.0.0.1:8080/", None);
    workflow["nodes"][1]["errorPolicy"] = json!({"mode": "fallback"});
    assert!(
        policy_codes(&workflow)
            .iter()
            .any(|code| code == "POLICY_ERROR_HANDLING")
    );
    workflow["nodes"][1]["errorPolicy"] = json!({"mode": "fallback", "fallbackPort": "error"});
    assert!(policy_codes(&workflow).is_empty());

    let mut workflow = http_workflow("http://127.0.0.1:8080/", None);
    workflow["timeout"] = json!({"executionMs": 0});
    workflow["policies"] = json!({"concurrency": {"limit": 0}});
    let codes = policy_codes(&workflow);
    assert!(
        codes.iter().any(|code| code == "POLICY_TIMEOUT"),
        "{codes:?}"
    );
    assert!(codes.iter().any(|code| code == "POLICY_LIMIT"), "{codes:?}");
}

#[test]
fn registry_definitions_cover_supported_nodes() {
    let definitions = registry::definitions();
    for node_type in [
        "start",
        "end",
        "transform",
        "condition",
        "switch",
        "loop",
        "parallel",
        "join",
        "http.request",
        "script",
        "setVariable",
        "human.approval",
        "subflow",
    ] {
        assert!(
            registry::definition(node_type).is_some(),
            "missing {node_type}"
        );
    }
    assert_eq!(definitions.len(), 13);
}

#[test]
fn workflow_parses_for_registry_test() {
    let workflow =
        Workflow::from_value(&http_workflow("http://127.0.0.1:8080/", None)).expect("workflow");
    assert_eq!(workflow.nodes[1].node_type, "http.request");
}
