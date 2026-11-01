import { Handle, Position, type NodeProps, type Node } from "@xyflow/react";
import {
  BadgeCheck,
  Box,
  Braces,
  CirclePlay,
  CircleStop,
  GitBranch,
  Globe,
  Layers,
  Merge,
  Repeat,
  Split,
  Terminal,
  Variable,
  Workflow,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { WorkflowNode } from "../../lib/types";
import { cn } from "../ui";
import styles from "./canvas.module.css";

export interface WorkflowNodeData extends Record<string, unknown> {
  node: WorkflowNode;
  label: string;
  description: string;
  ports: string[];
  tone: "neutral" | "accent" | "success" | "danger" | "warning" | "info" | "graph";
}

export type WorkflowFlowNode = Node<WorkflowNodeData, "hpa">;

const icons: Record<string, LucideIcon> = {
  start: CirclePlay,
  end: CircleStop,
  transform: Braces,
  condition: GitBranch,
  switch: Split,
  loop: Repeat,
  parallel: Layers,
  join: Merge,
  "http.request": Globe,
  script: Terminal,
  setVariable: Variable,
  "human.approval": BadgeCheck,
  subflow: Workflow,
};

const toneClass: Record<WorkflowNodeData["tone"], string> = {
  neutral: styles.toneNeutral!,
  accent: styles.toneAccent!,
  success: styles.toneSuccess!,
  danger: styles.toneDanger!,
  warning: styles.toneWarning!,
  info: styles.toneInfo!,
  graph: styles.toneGraph!,
};

export function WorkflowNodeView({ data, selected }: NodeProps<WorkflowFlowNode>) {
  const { t } = useTranslation();
  const node = data.node;
  const Icon = icons[node.type] ?? Box;
  const isContainer = node.type === "loop" || node.type === "parallel";
  const children =
    node.type === "loop"
      ? node.body?.nodes.length ?? 0
      : node.type === "parallel"
        ? node.branches?.length ?? 0
        : 0;

  return (
    <div
      className={cn(
        styles.node,
        toneClass[data.tone],
        selected && styles.nodeSelected,
      )}
    >
      {node.type !== "start" ? (
        <Handle
          type="target"
          position={Position.Left}
          id="in"
          className={styles.handle}
          aria-label="in"
        />
      ) : null}
      <div className={styles.nodeAccent} />
      <div className={styles.nodeHeader}>
        <span className={styles.nodeIcon}>
          <Icon className="h-3.5 w-3.5" />
        </span>
        <span className={styles.nodeKind}>{data.label}</span>
        {isContainer ? (
          <span className={styles.nodeMeta}>{children}</span>
        ) : null}
      </div>
      <div className={styles.nodeName}>{node.name || node.id}</div>
      <div className={styles.nodeId}>{node.id}</div>
      <div className={styles.nodeDescription}>{data.description}</div>
      {data.ports.map((port, index) => (
        <div
          key={port}
          className={styles.port}
          style={{ top: `${((index + 1) / (data.ports.length + 1)) * 100}%` }}
        >
          <span className={styles.portLabel}>{port === "out" ? t("common.output") : port}</span>
          <Handle
            type="source"
            position={Position.Right}
            id={port}
            className={styles.handle}
            aria-label={port}
          />
        </div>
      ))}
    </div>
  );
}
