import {
  createRootRoute,
  createRoute,
  createRouter,
} from "@tanstack/react-router";
import { AppShell } from "./components/shell/AppShell";
import { AgentsPage } from "./pages/Agents";
import { ApprovalsPage } from "./pages/Approvals";
import { ArtifactsPage } from "./pages/Artifacts";
import { DashboardPage } from "./pages/Dashboard";
import { DesignerPage } from "./pages/Designer";
import { EnterprisePage } from "./pages/Enterprise";
import { ExecutionDetailPage } from "./pages/ExecutionDetail";
import { ExecutionsPage } from "./pages/Executions";
import { LoginPage } from "./pages/Login";
import { McpCenterPage } from "./pages/McpCenter";
import { ConnectorsPage } from "./pages/Connectors";
import { ConsentPage } from "./pages/Consent";
import { PluginsPage } from "./pages/Plugins";
import { SettingsPage } from "./pages/Settings";
import { SkillsPage } from "./pages/Skills";
import { TriggersPage } from "./pages/Triggers";
import { WorkflowsPage } from "./pages/Workflows";

const rootRoute = createRootRoute({ component: AppShell });

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: DashboardPage,
});

const workflowsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/workflows",
  component: WorkflowsPage,
});

const designerRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/designer",
  component: DesignerPage,
});

const agentsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/agents",
  component: AgentsPage,
});

const skillsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/skills",
  component: SkillsPage,
});

const triggersRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/triggers",
  component: TriggersPage,
});

const pluginsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/plugins",
  component: PluginsPage,
});

const connectorsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/connectors",
  component: ConnectorsPage,
});

const mcpRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/mcp-center",
  component: McpCenterPage,
});

const executionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/executions",
  component: ExecutionsPage,
});

const executionDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/executions/$executionId",
  component: ExecutionDetailPage,
});

const approvalsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/approvals",
  component: ApprovalsPage,
});

const artifactsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/artifacts",
  component: ArtifactsPage,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});

const enterpriseRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/enterprise",
  component: EnterprisePage,
});

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: LoginPage,
});

const consentRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/consent",
  component: ConsentPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  workflowsRoute,
  designerRoute,
  agentsRoute,
  skillsRoute,
  triggersRoute,
  pluginsRoute,
  connectorsRoute,
  mcpRoute,
  executionsRoute,
  executionDetailRoute,
  approvalsRoute,
  artifactsRoute,
  settingsRoute,
  enterpriseRoute,
  loginRoute,
  consentRoute,
]);

export const router = createRouter({
  routeTree,
  defaultPreload: "intent",
  scrollRestoration: true,
});
