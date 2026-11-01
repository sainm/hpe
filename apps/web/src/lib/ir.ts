import YAML from "yaml";
import { z } from "zod";
import type {
  JsonValue,
  WorkflowGraph,
  WorkflowIR,
  WorkflowNode,
  WorkflowSubflow,
} from "./types";

export const NODE_TYPES = [
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
] as const;

export type NodeType = (typeof NODE_TYPES)[number];

const guardedNode = z.looseObject({
  id: z.string().min(1),
  type: z.string().min(1),
  typeVersion: z.string().min(1),
});

const guardedWorkflow = z.looseObject({
  schemaVersion: z.literal("hpa.dev/v1"),
  id: z.string().min(1),
  name: z.string().min(1),
  workflowVersion: z.string().min(1),
  nodes: z.array(guardedNode),
  edges: z.array(
    z.looseObject({
      id: z.string().min(1),
      source: z.string().min(1),
      target: z.string().min(1),
    }),
  ),
});

export function parseWorkflowText(format: "yaml" | "json", text: string): WorkflowIR {
  const value = format === "yaml" ? (YAML.parse(text) as unknown) : (JSON.parse(text) as unknown);
  return normalizedWorkflow(guardedWorkflow.parse(value) as unknown as WorkflowIR);
}

export function workflowToText(format: "yaml" | "json", ir: WorkflowIR): string {
  const value = orderedWorkflow(normalizedWorkflow(ir));
  return format === "yaml"
    ? YAML.stringify(value, { indent: 2, lineWidth: 0 })
    : JSON.stringify(value, null, 2);
}

function orderedWorkflow(ir: WorkflowIR): WorkflowIR {
  const ordered: Record<string, unknown> = {
    $schema: ir.$schema,
    schemaVersion: ir.schemaVersion,
    id: ir.id,
    name: ir.name,
    description: ir.description,
    workflowVersion: ir.workflowVersion,
    inputs: ir.inputs,
    outputs: ir.outputs,
    variables: ir.variables,
    nodes: ir.nodes,
    edges: ir.edges,
  };
  if (ir.triggers) ordered.triggers = ir.triggers;
  if (ir.policies) ordered.policies = ir.policies;
  if (ir.errorHandling) ordered.errorHandling = ir.errorHandling;
  if (ir.retry) ordered.retry = ir.retry;
  if (ir.timeout) ordered.timeout = ir.timeout;
  if (ir.subflows) ordered.subflows = ir.subflows;
  if (ir.layout) ordered.layout = ir.layout;
  if (ir.metadata) ordered.metadata = ir.metadata;
  return ordered as unknown as WorkflowIR;
}

export function normalizedWorkflow(ir: WorkflowIR): WorkflowIR {
  return {
    ...ir,
    $schema: ir.$schema ?? "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
    inputs: ir.inputs ?? [],
    outputs: ir.outputs ?? [],
    variables: ir.variables ?? [],
    nodes: ir.nodes ?? [],
    edges: ir.edges ?? [],
    subflows: ir.subflows ?? [],
  };
}

export function emptyWorkflow(name: string, id = `wf_${Date.now().toString(36)}`): WorkflowIR {
  const start: WorkflowNode = {
    id: "start",
    type: "start",
    typeVersion: "1.0.0",
    name: "Start",
    config: {},
    ui: { x: 0, y: 120 },
  };
  const end: WorkflowNode = {
    id: "end",
    type: "end",
    typeVersion: "1.0.0",
    name: "End",
    config: {},
    ui: { x: 560, y: 120 },
  };
  return {
    $schema: "https://hpa.dev/schema/ir/v0.1/workflow.schema.json",
    schemaVersion: "hpa.dev/v1",
    id,
    name,
    workflowVersion: "1.0.0",
    inputs: [],
    outputs: [],
    variables: [],
    nodes: [start, end],
    edges: [{ id: "e_start_end", source: "start", sourcePort: "out", target: "end", targetPort: "in" }],
    subflows: [],
    layout: { viewport: { x: 0, y: 0, zoom: 1 } },
  };
}

export function collectNodeIds(ir: WorkflowIR): Set<string> {
  const ids = new Set<string>();
  const visit = (graph: WorkflowGraph | undefined) => {
    if (!graph) {
      return;
    }
    for (const node of graph.nodes) {
      ids.add(node.id);
      visit(node.body);
      for (const branch of node.branches ?? []) {
        visit(branch.graph);
      }
    }
  };
  visit({ nodes: ir.nodes, edges: ir.edges });
  for (const subflow of ir.subflows ?? []) {
    ids.add(subflow.id);
    visit(subflow.graph);
  }
  return ids;
}

export function uniqueId(ir: WorkflowIR, prefix: string): string {
  const ids = collectNodeIds(ir);
  let index = 1;
  let candidate = `${prefix}_${index}`;
  while (ids.has(candidate)) {
    index += 1;
    candidate = `${prefix}_${index}`;
  }
  return candidate;
}

interface NodeSeed {
  node: WorkflowNode;
  subflow?: WorkflowSubflow;
}

export function createNode(
  type: NodeType,
  ir: WorkflowIR,
  position: { x: number; y: number },
  labels: { name: string; approvalTitle: string },
): NodeSeed {
  const id = uniqueId(ir, type.split(".")[0] ?? "node");
  const config = defaultConfig(type, id, labels.approvalTitle);
  const node: WorkflowNode = {
    id,
    type,
    typeVersion: "1.0.0",
    name: labels.name,
    config,
    ui: { x: position.x, y: position.y },
    retry: undefined,
    timeoutMs: type === "http.request" ? 10_000 : undefined,
  };
  let subflow: WorkflowSubflow | undefined;
  if (type === "loop") {
    node.body = {
      nodes: [
        {
          id: `${id}_body`,
          type: "transform",
          typeVersion: "1.0.0",
          name: "Loop body",
          config: { mapping: { value: { $expr: ".loop.item" } } },
          ui: { x: position.x, y: position.y + 180 },
        },
      ],
      edges: [],
    };
  }
  if (type === "parallel") {
    node.branches = ["left", "right"].map((branch) => ({
      id: branch,
      graph: {
        nodes: [
          {
            id: `${id}_${branch}`,
            type: "transform",
            typeVersion: "1.0.0",
            name: branch,
            config: { mapping: { branch } },
            ui: { x: position.x + (branch === "left" ? -80 : 80), y: position.y + 180 },
          },
        ],
        edges: [],
      },
    }));
  }
  if (type === "subflow") {
    const subflowId = uniqueId(ir, "sub");
    subflow = {
      id: subflowId,
      name: labels.name,
      inputs: [{ name: "value", type: "object", required: false }],
      outputs: [{ name: "value", type: "object" }],
      graph: {
        nodes: [
          {
            id: `${subflowId}_body`,
            type: "transform",
            typeVersion: "1.0.0",
            name: labels.name,
            config: { mapping: { value: { $expr: ".input.value" } } },
            ui: { x: 0, y: 0 },
          },
        ],
        edges: [],
      },
    };
    node.config = { subflowId };
  }
  return { node, subflow };
}

function defaultConfig(
  type: NodeType,
  id: string,
  approvalTitle: string,
): Record<string, JsonValue> {
  switch (type) {
    case "transform":
      return { mapping: { value: { $expr: ".input.value" } } };
    case "condition":
      return { condition: { $expr: "true" } };
    case "switch":
      return {
        cases: [{ port: "case_a", condition: { $expr: "true" } }],
        defaultPort: "default",
      };
    case "loop":
      return {
        mode: "forEach",
        items: { $expr: "[1, 2, 3]" },
        itemVariable: "item",
        indexVariable: "index",
        maxIterations: 100,
      };
    case "parallel":
      return { join: "all", failFast: false, maxConcurrency: 2 };
    case "http.request":
      return { method: "GET", url: "http://127.0.0.1:8080/" };
    case "script":
      return { language: "jq", source: "." };
    case "setVariable":
      return { name: "value", value: { $expr: ".input" } };
    case "human.approval":
      return {
        title: approvalTitle,
        formSchema: {
          type: "object",
          properties: {
            decision: { type: "string", enum: ["approve", "reject"] },
            comment: { type: "string" },
          },
          required: ["decision"],
        },
        assignees: [],
        onTimeout: "reject",
      };
    case "subflow":
      return { subflowId: "" };
    case "start":
    case "end":
    case "join":
      return {};
    default:
      return { node: id };
  }
}

export function nodePorts(node: WorkflowNode, outgoingPorts: string[]): string[] {
  const ports: string[] = [];
  const add = (port: string | undefined) => {
    if (port && !ports.includes(port)) {
      ports.push(port);
    }
  };
  switch (node.type) {
    case "start":
      add("out");
      break;
    case "end":
      break;
    case "condition":
      add("true");
      add("false");
      break;
    case "switch": {
      const cases = Array.isArray(node.config.cases) ? node.config.cases : [];
      for (const entry of cases) {
        if (entry && typeof entry === "object" && !Array.isArray(entry)) {
          const port = (entry as Record<string, JsonValue>).port;
          if (typeof port === "string") {
            add(port);
          }
        }
      }
      const fallback = node.config.defaultPort;
      add(typeof fallback === "string" ? fallback : "default");
      break;
    }
    case "human.approval":
      add("approved");
      add("rejected");
      add("timeout");
      break;
    case "loop":
      add("out");
      break;
    case "parallel":
      add("out");
      break;
    default:
      add("out");
      break;
  }
  if (node.errorPolicy?.mode === "fallback") {
    add(node.errorPolicy.fallbackPort ?? "error");
  }
  for (const port of outgoingPorts) {
    add(port);
  }
  return ports;
}

export type NodeTone =
  | "neutral"
  | "accent"
  | "success"
  | "danger"
  | "warning"
  | "info"
  | "graph";

export function nodeTone(type: string): NodeTone {
  switch (type) {
    case "start":
    case "end":
      return "neutral";
    case "condition":
    case "switch":
      return "info";
    case "loop":
    case "parallel":
      return "graph";
    case "human.approval":
      return "warning";
    case "http.request":
    case "script":
      return "danger";
    default:
      return "success";
  }
}

export function updateNodeAt(
  ir: WorkflowIR,
  nodeId: string,
  updater: (node: WorkflowNode) => WorkflowNode,
): WorkflowIR {
  return {
    ...ir,
    nodes: ir.nodes.map((node) => (node.id === nodeId ? updater(node) : node)),
  };
}

export function nextNodePosition(ir: WorkflowIR): { x: number; y: number } {
  const nodes = ir.nodes;
  if (nodes.length === 0) {
    return { x: 0, y: 0 };
  }
  const maxX = Math.max(...nodes.map((node) => Number(node.ui?.x ?? 0)));
  const avgY =
    nodes.reduce((sum, node) => sum + Number(node.ui?.y ?? 0), 0) / Math.max(nodes.length, 1);
  return { x: maxX + 280, y: Math.round(avgY) };
}

export function sampleWorkflows(labels: {
  sequenceName: string;
  sequenceDescription: string;
  approvalName: string;
  approvalDescription: string;
  parallelName: string;
  parallelDescription: string;
  approvalTitle: string;
}): WorkflowIR[] {
  const sequence: WorkflowIR = {
    ...emptyWorkflow(labels.sequenceName, "wf_sample_sequence"),
    description: labels.sequenceDescription,
    inputs: [{ name: "value", type: "integer", required: true }],
    nodes: [
      { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {}, ui: { x: 0, y: 120 } },
      {
        id: "map",
        type: "transform",
        typeVersion: "1.0.0",
        name: labels.sequenceName,
        config: { mapping: { value: { $expr: ".input.value + 1" } } },
        ui: { x: 280, y: 120 },
      },
      { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {}, ui: { x: 560, y: 120 } },
    ],
    edges: [
      { id: "e_start_map", source: "start", sourcePort: "out", target: "map", targetPort: "in" },
      { id: "e_map_end", source: "map", sourcePort: "out", target: "end", targetPort: "in" },
    ],
  };
  const approval: WorkflowIR = {
    ...emptyWorkflow(labels.approvalName, "wf_sample_approval"),
    description: labels.approvalDescription,
    nodes: [
      { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {}, ui: { x: 0, y: 120 } },
      {
        id: "approve",
        type: "human.approval",
        typeVersion: "1.0.0",
        name: labels.approvalName,
        config: {
          title: labels.approvalTitle,
          formSchema: {
            type: "object",
            properties: { decision: { type: "string", enum: ["approve", "reject"] }, comment: { type: "string" } },
            required: ["decision"],
          },
          assignees: [],
          onTimeout: "reject",
        },
        ui: { x: 280, y: 120 },
      },
      {
        id: "approved",
        type: "transform",
        typeVersion: "1.0.0",
        name: "Approved",
        config: { mapping: { decision: "approved" } },
        ui: { x: 560, y: 20 },
      },
      {
        id: "rejected",
        type: "transform",
        typeVersion: "1.0.0",
        name: "Rejected",
        config: { mapping: { decision: "rejected" } },
        ui: { x: 560, y: 220 },
      },
      { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {}, ui: { x: 840, y: 120 } },
    ],
    edges: [
      { id: "e_start_approve", source: "start", sourcePort: "out", target: "approve", targetPort: "in" },
      { id: "e_approved", source: "approve", sourcePort: "approved", target: "approved", targetPort: "in" },
      { id: "e_rejected", source: "approve", sourcePort: "rejected", target: "rejected", targetPort: "in" },
      { id: "e_approved_end", source: "approved", sourcePort: "out", target: "end", targetPort: "in" },
      { id: "e_rejected_end", source: "rejected", sourcePort: "out", target: "end", targetPort: "in" },
    ],
  };
  const parallel: WorkflowIR = {
    ...emptyWorkflow(labels.parallelName, "wf_sample_parallel"),
    description: labels.parallelDescription,
    nodes: [
      { id: "start", type: "start", typeVersion: "1.0.0", name: "Start", config: {}, ui: { x: 0, y: 120 } },
      {
        id: "parallel",
        type: "parallel",
        typeVersion: "1.0.0",
        name: labels.parallelName,
        config: { join: "all", failFast: false, maxConcurrency: 2 },
        branches: ["left", "right"].map((branch) => ({
          id: branch,
          graph: {
            nodes: [
              {
                id: `parallel_${branch}`,
                type: "transform",
                typeVersion: "1.0.0",
                name: branch,
                config: { mapping: { branch } },
                ui: { x: branch === "left" ? 200 : 360, y: 280 },
              },
            ],
            edges: [],
          },
        })),
        ui: { x: 280, y: 120 },
      },
      { id: "end", type: "end", typeVersion: "1.0.0", name: "End", config: {}, ui: { x: 600, y: 120 } },
    ],
    edges: [
      { id: "e_start_parallel", source: "start", sourcePort: "out", target: "parallel", targetPort: "in" },
      { id: "e_parallel_end", source: "parallel", sourcePort: "out", target: "end", targetPort: "in" },
    ],
  };
  return [sequence, approval, parallel];
}

export function workflowToGraph(ir: WorkflowIR, selectedId: string | null): WorkflowGraph {
  return {
    nodes: ir.nodes.map((node) => ({
      ...node,
      ui: {
        ...node.ui,
        x: Number(node.ui?.x ?? 0),
        y: Number(node.ui?.y ?? 0),
        selected: node.id === selectedId,
      },
    })),
    edges: ir.edges,
  };
}

export function nodeKey(type: string): string {
  switch (type) {
    case "http.request":
      return "http";
    case "human.approval":
      return "approval";
    case "setVariable":
      return "setVariable";
    case "subflow":
      return "subflow";
    default:
      return NODE_TYPES.includes(type as NodeType) ? type : "transform";
  }
}

export function defaultInputFor(ir: WorkflowIR): Record<string, JsonValue> {
  const input: Record<string, JsonValue> = {};
  for (const io of ir.inputs ?? []) {
    if (io.default !== undefined) {
      input[io.name] = io.default;
      continue;
    }
    switch (io.type) {
      case "integer":
      case "number":
        input[io.name] = 0;
        break;
      case "boolean":
        input[io.name] = false;
        break;
      case "array":
        input[io.name] = [];
        break;
      case "object":
        input[io.name] = {};
        break;
      default:
        input[io.name] = "";
    }
  }
  return input;
}
