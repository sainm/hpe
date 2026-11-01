import { create } from "zustand";
import type {
  Issue,
  JsonValue,
  WorkflowEdge,
  WorkflowIR,
  WorkflowNode,
  WorkflowSubflow,
} from "../lib/types";
import { updateNodeAt } from "../lib/ir";

type Panel = "yaml" | "json" | "debug";

interface EditorState {
  workflowId: string | null;
  ir: WorkflowIR | null;
  selectedNodeId: string | null;
  dirty: boolean;
  panel: Panel;
  issues: Issue[];
  runningExecutionId: string | null;
  load: (ir: WorkflowIR) => void;
  replaceIr: (ir: WorkflowIR) => void;
  patchWorkflow: (patch: Partial<WorkflowIR>) => void;
  clear: () => void;
  markSaved: (workflowId?: string) => void;
  selectNode: (nodeId: string | null) => void;
  updateNode: (nodeId: string, updater: (node: WorkflowNode) => WorkflowNode) => void;
  patchNode: (nodeId: string, patch: Partial<WorkflowNode>) => void;
  setConfig: (nodeId: string, config: Record<string, JsonValue>) => void;
  addNode: (node: WorkflowNode, subflow?: WorkflowSubflow) => void;
  removeNode: (nodeId: string) => void;
  moveNode: (nodeId: string, x: number, y: number) => void;
  addEdge: (edge: WorkflowEdge) => void;
  removeEdge: (edgeId: string) => void;
  setPanel: (panel: Panel) => void;
  setIssues: (issues: Issue[]) => void;
  setRunningExecution: (executionId: string | null) => void;
}

export const useEditorStore = create<EditorState>((set) => ({
  workflowId: null,
  ir: null,
  selectedNodeId: null,
  dirty: false,
  panel: "yaml",
  issues: [],
  runningExecutionId: null,

  load: (ir) =>
    set({
      ir,
      workflowId: ir.id,
      selectedNodeId: null,
      dirty: false,
      issues: [],
      runningExecutionId: null,
    }),

  replaceIr: (ir) =>
    set({
      ir,
      workflowId: ir.id,
      selectedNodeId: null,
      dirty: true,
      issues: [],
    }),

  patchWorkflow: (patch) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return { ir: { ...state.ir, ...patch }, dirty: true };
    }),

  clear: () =>
    set({
      workflowId: null,
      ir: null,
      selectedNodeId: null,
      dirty: false,
      issues: [],
      runningExecutionId: null,
    }),

  markSaved: (workflowId) =>
    set((state) => ({
      workflowId: workflowId ?? state.workflowId,
      dirty: false,
    })),

  selectNode: (selectedNodeId) => set({ selectedNodeId }),

  updateNode: (nodeId, updater) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return { ir: updateNodeAt(state.ir, nodeId, updater), dirty: true };
    }),

  patchNode: (nodeId, patch) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: updateNodeAt(state.ir, nodeId, (node) => ({ ...node, ...patch })),
        dirty: true,
      };
    }),

  setConfig: (nodeId, config) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: updateNodeAt(state.ir, nodeId, (node) => ({
          ...node,
          config,
        })),
        dirty: true,
      };
    }),

  addNode: (node, subflow) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: {
          ...state.ir,
          nodes: [...state.ir.nodes, node],
          subflows: subflow ? [...(state.ir.subflows ?? []), subflow] : state.ir.subflows,
        },
        selectedNodeId: node.id,
        dirty: true,
      };
    }),

  removeNode: (nodeId) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: {
          ...state.ir,
          nodes: state.ir.nodes.filter((node) => node.id !== nodeId),
          edges: state.ir.edges.filter(
            (edge) => edge.source !== nodeId && edge.target !== nodeId,
          ),
        },
        selectedNodeId: state.selectedNodeId === nodeId ? null : state.selectedNodeId,
        dirty: true,
      };
    }),

  moveNode: (nodeId, x, y) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: updateNodeAt(state.ir, nodeId, (node) => ({
          ...node,
          ui: { ...node.ui, x, y },
        })),
        dirty: true,
      };
    }),

  addEdge: (edge) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: { ...state.ir, edges: [...state.ir.edges, edge] },
        dirty: true,
      };
    }),

  removeEdge: (edgeId) =>
    set((state) => {
      if (!state.ir) {
        return state;
      }
      return {
        ir: {
          ...state.ir,
          edges: state.ir.edges.filter((edge) => edge.id !== edgeId),
        },
        dirty: true,
      };
    }),

  setPanel: (panel) => set({ panel }),
  setIssues: (issues) => set({ issues }),
  setRunningExecution: (runningExecutionId) => set({ runningExecutionId }),
}));
