import { describe, expect, it } from "vitest";
import {
  createNode,
  defaultInputFor,
  emptyWorkflow,
  nodePorts,
  parseWorkflowText,
  sampleWorkflows,
  uniqueId,
  workflowToText,
} from "./ir";
import type { WorkflowNode } from "./types";

const labels = {
  sequenceName: "Sequence",
  sequenceDescription: "desc",
  approvalName: "Approval",
  approvalDescription: "desc",
  parallelName: "Parallel",
  parallelDescription: "desc",
  approvalTitle: "Approve",
};

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonical);
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return Object.fromEntries(
      Object.keys(record)
        .sort()
        .map((key) => [key, canonical(record[key])]),
    );
  }
  return value;
}

describe("workflow IR text roundtrip", () => {
  it("keeps semantics between JSON and YAML", () => {
    const workflow = emptyWorkflow("Roundtrip");
    const json = parseWorkflowText("json", workflowToText("json", workflow));
    const yaml = parseWorkflowText("yaml", workflowToText("yaml", workflow));
    expect(canonical(yaml)).toEqual(canonical(json));
  });

  it("keeps samples parseable in both formats", () => {
    for (const sample of sampleWorkflows(labels)) {
      const json = parseWorkflowText("json", workflowToText("json", sample));
      const yaml = parseWorkflowText("yaml", workflowToText("yaml", sample));
      expect(canonical(yaml)).toEqual(canonical(json));
      expect(json.nodes.some((node) => node.type === "start")).toBe(true);
      expect(json.nodes.some((node) => node.type === "end")).toBe(true);
    }
  });
});

describe("node helpers", () => {
  it("derives ports by node kind", () => {
    const condition: WorkflowNode = {
      id: "c",
      type: "condition",
      typeVersion: "1.0.0",
      config: {},
    };
    expect(nodePorts(condition, [])).toEqual(["true", "false"]);

    const switchNode: WorkflowNode = {
      id: "s",
      type: "switch",
      typeVersion: "1.0.0",
      config: {
        cases: [
          { port: "case_a", condition: { $expr: "true" } },
          { port: "case_b", condition: { $expr: "false" } },
        ],
        defaultPort: "default",
      },
    };
    expect(nodePorts(switchNode, [])).toEqual(["case_a", "case_b", "default"]);

    const transform: WorkflowNode = {
      id: "t",
      type: "transform",
      typeVersion: "1.0.0",
      config: { mapping: {} },
    };
    expect(nodePorts(transform, [])).toEqual(["out"]);
    expect(nodePorts(transform, ["error"])).toEqual(["out", "error"]);
  });

  it("adds unique ids and container bodies", () => {
    const workflow = emptyWorkflow("Ids");
    const loop = createNode("loop", workflow, { x: 10, y: 10 }, {
      name: "Loop",
      approvalTitle: "Approve",
    });
    expect(loop.node.body?.nodes.length).toBe(1);
    expect(uniqueId(workflow, "node")).toBe("node_1");
    expect(loop.node.id).not.toBe("start");
  });

  it("builds typed default inputs", () => {
    const workflow = emptyWorkflow("Inputs");
    workflow.inputs = [
      { name: "count", type: "integer", required: true },
      { name: "flag", type: "boolean" },
      { name: "label", type: "string", default: "x" },
    ];
    expect(defaultInputFor(workflow)).toEqual({ count: 0, flag: false, label: "x" });
  });
});
