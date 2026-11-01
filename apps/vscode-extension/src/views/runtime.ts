import type { HpaServices } from "../contracts";
import { actionItem, fieldItem, ViewProvider, type HpaTreeItem } from "./common";

export class RuntimeProvider extends ViewProvider {
  constructor(services: HpaServices) {
    super(services);
  }

  protected async load(): Promise<HpaTreeItem[]> {
    const health = await this.services.client.health();
    const i18n = this.services.i18n;
    const connectLabel = i18n.t("runtime.reconnect");
    return [
      fieldItem(this.services, i18n.t("common.status"), health.status, "pulse"),
      fieldItem(this.services, i18n.t("runtime.version"), health.version, "versions"),
      fieldItem(this.services, i18n.t("runtime.platform"), health.runtime, "server"),
      fieldItem(this.services, i18n.t("runtime.endpoint"), this.services.client.endpoint, "globe"),
      actionItem(connectLabel, "plug", "hpa.runtime.connect", connectLabel),
    ];
  }
}
