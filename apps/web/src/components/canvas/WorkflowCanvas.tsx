import {
  Background,
  BackgroundVariant,
  Controls,
  MarkerType,
  ReactFlow,
  useNodesState,
  type Connection,
  type Edge,
  type NodeTypes,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { nodePorts, nodeTone } from "../../lib/ir";
import type { WorkflowEdge, WorkflowIR } from "../../lib/types";
import { useUiStore } from "../../stores/ui";
import { WorkflowNodeView, type WorkflowFlowNode } from "./WorkflowNodeView";
import styles from "./canvas.module.css";

const nodeTypes: NodeTypes = { hpa: WorkflowNodeView };

interface Props {
  ir: WorkflowIR;
  selectedId: string | null;
  onSelect: (nodeId: string | null) => void;
  onMove: (nodeId: string, x: number, y: number) => void;
  onConnect: (edge: WorkflowEdge) => void;
  onDeleteNode: (nodeId: string) => void;
  onDeleteEdge: (edgeId: string) => void;
}

export function WorkflowCanvas({
  ir,
  selectedId,
  onSelect,
  onMove,
  onConnect,
  onDeleteNode,
  onDeleteEdge,
}: Props) {
  const { t } = useTranslation();
  const theme = useUiStore((state) => state.theme);

  const generated = useMemo<WorkflowFlowNode[]>(() => {
    return ir.nodes.map((node, index) => {
      const outgoing = ir.edges
        .filter((edge) => edge.source === node.id)
        .map((edge) => edge.sourcePort ?? "out");
      return {
        id: node.id,
        type: "hpa",
        position: {
          x: Number(node.ui?.x ?? index * 280),
          y: Number(node.ui?.y ?? 120),
        },
        selected: node.id === selectedId,
        data: {
          node,
          label: nodeLabel(node.type, t),
          description: nodeDescription(node.type, t),
          ports: nodePorts(node, outgoing),
          tone: nodeTone(node.type),
        },
      };
    });
  }, [ir, selectedId, t]);

  const [nodes, setNodes, onNodesChange] = useNodesState<WorkflowFlowNode>(generated);

  useEffect(() => {
    setNodes(generated);
  }, [generated, setNodes]);

  const edges = useMemo<Edge[]>(() => {
    return ir.edges.map((edge) => {
      const port = edge.sourcePort ?? "out";
      return {
        id: edge.id,
        source: edge.source,
        target: edge.target,
        sourceHandle: port,
        targetHandle: edge.targetPort ?? "in",
        type: "smoothstep",
        markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
        label: port === "out" ? undefined : port,
        labelStyle: {
          fill: "var(--hpa-muted)",
          fontFamily: "var(--hpa-font-mono)",
          fontSize: 9,
        },
        labelBgStyle: { fill: "var(--hpa-panel)" },
        labelBgPadding: [4, 2],
        labelBgBorderRadius: 4,
      };
    });
  }, [ir.edges]);

  const handleConnect = useCallback(
    (connection: Connection) => {
      if (!connection.source || !connection.target) {
        return;
      }
      onConnect({
        id: `e_${connection.source}_${connection.target}_${Date.now().toString(36)}`,
        source: connection.source,
        sourcePort: connection.sourceHandle ?? "out",
        target: connection.target,
        targetPort: connection.targetHandle ?? "in",
      });
    },
    [onConnect],
  );

  return (
    <div className={styles.canvas}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onNodeClick={(_, node) => onSelect(node.id)}
        onPaneClick={() => onSelect(null)}
        onNodeDragStop={(_, node) => onMove(node.id, node.position.x, node.position.y)}
        onNodesDelete={(deleted) => deleted.forEach((node) => onDeleteNode(node.id))}
        onEdgesDelete={(deleted) => deleted.forEach((edge) => onDeleteEdge(edge.id))}
        onConnect={handleConnect}
        colorMode={theme === "light" ? "light" : "dark"}
        fitView
        fitViewOptions={{ padding: 0.1 }}
        minZoom={0.25}
        maxZoom={1.8}
        snapToGrid
        snapGrid={[16, 16]}
        deleteKeyCode={["Backspace", "Delete"]}
        defaultEdgeOptions={{ type: "smoothstep" }}
        proOptions={{ hideAttribution: false }}
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={22}
          size={1}
          color="var(--hpa-canvas-grid)"
        />
        <Controls className={styles.controls} showInteractive={false} />
      </ReactFlow>
      {ir.nodes.length === 0 ? (
        <div className={styles.empty}>{t("designer.emptyCanvas")}</div>
      ) : null}
    </div>
  );
}

function nodeLabel(type: string, t: (key: string) => string): string {
  switch (type) {
    case "start":
      return t("node.start");
    case "end":
      return t("node.end");
    case "transform":
      return t("node.transform");
    case "condition":
      return t("node.condition");
    case "switch":
      return t("node.switch");
    case "loop":
      return t("node.loop");
    case "parallel":
      return t("node.parallel");
    case "join":
      return t("node.join");
    case "http.request":
      return t("node.http");
    case "script":
      return t("node.script");
    case "setVariable":
      return t("node.setVariable");
    case "human.approval":
      return t("node.approval");
    case "subflow":
      return t("node.subflow");
    default:
      return type;
  }
}

function nodeDescription(type: string, t: (key: string) => string): string {
  switch (type) {
    case "start":
      return t("node.desc.start");
    case "end":
      return t("node.desc.end");
    case "transform":
      return t("node.desc.transform");
    case "condition":
      return t("node.desc.condition");
    case "switch":
      return t("node.desc.switch");
    case "loop":
      return t("node.desc.loop");
    case "parallel":
      return t("node.desc.parallel");
    case "join":
      return t("node.desc.join");
    case "http.request":
      return t("node.desc.http");
    case "script":
      return t("node.desc.script");
    case "setVariable":
      return t("node.desc.setVariable");
    case "human.approval":
      return t("node.desc.approval");
    case "subflow":
      return t("node.desc.subflow");
    default:
      return type;
  }
}
