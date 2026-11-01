import type { HpaServices } from "../contracts";
import {
  actionItem,
  fieldItem,
  groupItem,
  runtimeStatusLabel,
  ViewProvider,
  type HpaTreeItem,
} from "./common";

export class OverviewProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const [health, stats, approvals, skills, agents] = await Promise.all([
      this.services.client.health(),
      this.services.client.stats(),
      this.services.client.listApprovals(),
      this.services.client.listSkills(),
      this.services.client.listAgents(),
    ]);
    const status = runtimeStatusLabel(this.services);
    const runtime = [
      fieldItem(this.services, this.services.i18n.t("common.status"), status, "pulse"),
      fieldItem(this.services, this.services.i18n.t("runtime.version"), health.version, "versions"),
      fieldItem(this.services, this.services.i18n.t("runtime.platform"), health.runtime, "server"),
      fieldItem(
        this.services,
        this.services.i18n.t("runtime.endpoint"),
        this.services.client.endpoint,
        "globe",
      ),
      fieldItem(
        this.services,
        this.services.i18n.t("common.time"),
        this.services.runtime().checkedAt,
        "clock",
      ),
    ];
    const pending = fieldItem(
      this.services,
      this.services.i18n.t("overview.pendingApprovals"),
      String(stats.waitingApproval),
      "bell",
    );
    pending.tooltip = `${this.services.i18n.t("overview.pendingApprovals")}: ${approvals.length}`;
    const counts = [
      fieldItem(
        this.services,
        this.services.i18n.t("overview.workflows"),
        String(stats.workflows),
        "symbol-class",
      ),
      fieldItem(
        this.services,
        this.services.i18n.t("overview.executions"),
        String(stats.executions),
        "run",
      ),
      pending,
      fieldItem(
        this.services,
        this.services.i18n.t("overview.skills"),
        String(skills.length),
        "lightbulb",
      ),
      fieldItem(
        this.services,
        this.services.i18n.t("overview.agents"),
        String(agents.length),
        "robot",
      ),
    ];
    const quickActions = [
      actionItem(
        this.services.i18n.t("status.connecting"),
        "plug",
        "hpa.runtime.connect",
        this.services.i18n.t("status.connecting"),
      ),
      actionItem(
        this.services.i18n.t("workflows.open"),
        "link-external",
        "hpa.web.open",
        this.services.i18n.t("workflows.open"),
        ["/"],
      ),
      actionItem(
        this.services.i18n.t("skills.install"),
        "cloud-download",
        "hpa.skill.install",
        this.services.i18n.t("skills.install"),
      ),
      actionItem(
        this.services.i18n.t("mcp.register"),
        "server",
        "hpa.mcp.register",
        this.services.i18n.t("mcp.register"),
      ),
    ];
    return [
      groupItem(
        this.services.i18n.t("overview.runtime"),
        "server-process",
        runtime,
        status,
      ),
      groupItem(this.services.i18n.t("overview.counts"), "graph", counts),
      groupItem(
        this.services.i18n.t("overview.quickActions"),
        "zap",
        quickActions,
      ),
    ];
  }
}
