import { describe, expect, it, vi } from "vitest";
import type { HpaServices } from "../../src/contracts";
import { I18n } from "../../src/i18n";
import type { RuntimeClient } from "../../src/runtime/client";
import { PluginsProvider } from "../../src/views/plugins";

describe("PluginsProvider remote marketplace tree", () => {
  it("groups sources, packages, publishers and signing keys", async () => {
    const client = {
      listPlugins: vi.fn(async () => []),
      listMarketplaceSources: vi.fn(async () => [
        {
          id: "source-one",
          label: "Source One",
          indexUri: "https://market.example/index.json",
          enabled: true,
          lastSequence: 4,
          createdAt: "2026-11-02T00:00:00Z",
          updatedAt: "2026-11-02T00:00:00Z",
          keys: [{ keyId: "source-key", label: "primary" }],
          packageCount: 1,
          approvedCount: 1,
          withdrawnCount: 0,
        },
      ]),
      listMarketplacePublishers: vi.fn(async () => [
        {
          id: "publisher-one",
          name: "Publisher One",
          status: "active",
          createdAt: "2026-11-02T00:00:00Z",
          updatedAt: "2026-11-02T00:00:00Z",
          keys: [
            {
              keyId: "publisher-key",
              label: "primary",
              createdAt: "2026-11-02T00:00:00Z",
            },
          ],
        },
      ]),
      listRemoteMarketplace: vi.fn(async () => [
        {
          sourceId: "source-one",
          pluginId: "plugin-one",
          version: "1.0.0",
          publisherId: "publisher-one",
          signerKeyId: "publisher-key",
          packageSha256: "a".repeat(64),
          reviewStatus: "approved",
          sequence: 4,
          provenance: {
            kind: "cosign-v1",
            statementPath: "provenance/statement.json",
            statementSha256: "b".repeat(64),
            bundlePath: "provenance/bundle.json",
            bundleSha256: "c".repeat(64),
          },
          oci: {
            repository: "registry.example/plugin-one",
            digest: `sha256:${"d".repeat(64)}`,
          },
        },
      ]),
    } as unknown as RuntimeClient;
    const i18n = new I18n("en");
    const services = {
      client,
      i18n,
      runtime: () => ({
        status: "online",
        checkedAt: "2026-11-02T00:00:00Z",
      }),
      onDidChangeRuntime: () => ({ dispose: vi.fn() }),
      dataEvents: {
        onDidChange: () => ({ dispose: vi.fn() }),
        fire: vi.fn(),
      },
      output: { appendLine: vi.fn() },
    } as unknown as HpaServices;
    const provider = new PluginsProvider(services);

    const root = await provider.getChildren();
    const sources = root.find((item) => item.label === i18n.t("plugins.remoteSources"));
    const publishers = root.find((item) => item.label === i18n.t("plugins.publishers"));
    expect(sources?.children).toHaveLength(1);
    expect(publishers?.children).toHaveLength(1);

    const source = sources?.children?.[0];
    expect(source?.label).toBe("Source One");
    expect(source?.children).toHaveLength(2);
    const remotePackage = source?.children?.find(
      (item) => item.remotePackage?.pluginId === "plugin-one",
    );
    expect(remotePackage?.description).toContain("approved");
    expect(remotePackage?.description).toContain("Sigstore");
    expect(remotePackage?.description).toContain("OCI");
    expect(remotePackage?.command?.command).toBe("hpa.marketplace.scan");

    const publisher = publishers?.children?.[0];
    expect(publisher?.children?.[0]?.label).toBe("publisher-key");
    provider.dispose();
  });
});
