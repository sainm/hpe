import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Boxes,
  CheckCircle2,
  FileJson2,
  Fingerprint,
  KeyRound,
  Package,
  Plug,
  Power,
  PowerOff,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  Sparkles,
  Star,
  Store,
  Trash2,
  Upload,
  XCircle,
} from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  JsonView,
  PageHeader,
  Panel,
  Timestamp,
  cn,
  inputClass,
  textareaClass,
  type BadgeTone,
} from "../components/ui";
import { api, ApiError } from "../lib/api";
import type {
  MarketDetail,
  MarketSummary,
  PluginInspection,
  PluginSbom,
} from "../lib/types";
import { useUiStore } from "../stores/ui";

type PluginTab = "installed" | "marketplace";

function trustTone(trust: string | null | undefined): BadgeTone {
  if (trust === "verified") return "success";
  if (trust === "community") return "warning";
  if (trust === "untrusted") return "danger";
  return "neutral";
}

function statusTone(status: string): BadgeTone {
  if (status === "enabled") return "success";
  if (status === "installed") return "info";
  if (status === "disabled") return "warning";
  return "neutral";
}

function reviewTone(status: string): BadgeTone {
  if (status === "approved") return "success";
  if (status === "rejected") return "danger";
  if (status === "pending") return "warning";
  return "neutral";
}

export function PluginsPage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [tab, setTab] = useState<PluginTab>("installed");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [installOpen, setInstallOpen] = useState(false);
  const [installPath, setInstallPath] = useState("");
  const [allowUnsigned, setAllowUnsigned] = useState(false);
  const [allowUntrusted, setAllowUntrusted] = useState(false);
  const [force, setForce] = useState(false);
  const [inspection, setInspection] = useState<PluginInspection | null>(null);
  const [publishPath, setPublishPath] = useState("");
  const [marketPluginId, setMarketPluginId] = useState<string | null>(null);
  const [marketVersion, setMarketVersion] = useState<string | null>(null);
  const [reviewNote, setReviewNote] = useState("");
  const [rating, setRating] = useState("5");
  const [ratingComment, setRatingComment] = useState("");
  const [keyId, setKeyId] = useState("");
  const [publicKey, setPublicKey] = useState("");
  const [keyLabel, setKeyLabel] = useState("");
  const [keyTrust, setKeyTrust] = useState<"verified" | "community">("verified");

  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.listPlugins, retry: 1 });
  const detail = useQuery({
    queryKey: ["plugin", selectedId],
    queryFn: () => api.getPlugin(selectedId ?? ""),
    enabled: Boolean(selectedId),
    retry: 1,
  });
  const versionDetail = useQuery({
    queryKey: ["plugin-version", selectedId, version],
    queryFn: () => api.getPluginVersion(selectedId ?? "", version ?? ""),
    enabled: Boolean(selectedId && version),
    retry: 1,
  });
  const keyInfo = useQuery({ queryKey: ["plugin-key"], queryFn: api.pluginKey, retry: 1 });
  const trustedKeys = useQuery({ queryKey: ["plugin-keys"], queryFn: api.pluginKeys, retry: 1 });
  const marketplace = useQuery({
    queryKey: ["marketplace"],
    queryFn: api.listMarketplace,
    retry: 1,
  });
  const marketDetail = useQuery({
    queryKey: ["market", marketPluginId, marketVersion],
    queryFn: () => api.getMarketVersion(marketPluginId ?? "", marketVersion ?? ""),
    enabled: Boolean(marketPluginId && marketVersion),
    retry: 1,
  });

  const pluginList = plugins.data?.plugins ?? [];
  const marketList = marketplace.data?.plugins ?? [];
  const installedCount = pluginList.filter((plugin) => plugin.installedVersion).length;
  const enabledCount = pluginList.filter((plugin) => plugin.status === "enabled").length;
  const approvedCount = marketList.filter((entry) => entry.reviewStatus === "approved").length;
  const pendingCount = marketList.filter((entry) => entry.reviewStatus === "pending").length;

  useEffect(() => {
    if (!selectedId && pluginList.length > 0) {
      setSelectedId(pluginList[0]!.id);
    }
  }, [pluginList, selectedId]);

  useEffect(() => {
    if (!detail.data) return;
    setVersion((current) => {
      if (current && detail.data?.packages.some((entry) => entry.version === current)) {
        return current;
      }
      return (
        detail.data?.plugin.installedVersion ??
        detail.data?.plugin.latestVersion ??
        detail.data?.packages[0]?.version ??
        null
      );
    });
  }, [detail.data]);

  useEffect(() => {
    if (marketPluginId || marketList.length === 0) return;
    setMarketPluginId(marketList[0]!.pluginId);
    setMarketVersion(marketList[0]!.version);
  }, [marketList, marketPluginId]);

  function invalidatePluginQueries(id?: string | null) {
    void queryClient.invalidateQueries({ queryKey: ["plugins"] });
    if (id) void queryClient.invalidateQueries({ queryKey: ["plugin", id] });
    void queryClient.invalidateQueries({ queryKey: ["plugin-version"] });
    void queryClient.invalidateQueries({ queryKey: ["connectors"] });
    void queryClient.invalidateQueries({ queryKey: ["connections"] });
  }

  const installPlugin = useMutation({
    mutationFn: () => api.installPlugin(installPath.trim(), allowUnsigned, allowUntrusted),
    onSuccess: (result) => {
      pushToast(t("plugins.installDone"), "success");
      setInspection(null);
      setSelectedId(result.plugin.id);
      setVersion(result.plugin.installedVersion ?? result.plugin.latestVersion);
      invalidatePluginQueries(result.plugin.id);
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.installFailed")), "error"),
  });

  const inspectPlugin = useMutation({
    mutationFn: () => api.inspectPlugin(installPath.trim(), allowUnsigned, allowUntrusted),
    onSuccess: (result) => {
      setInspection(result);
      pushToast(t("plugins.inspectDone"), "success");
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.inspectFailed")), "error"),
  });

  const samplePlugin = useMutation({
    mutationFn: api.samplePlugin,
    onSuccess: (result) => {
      pushToast(t("plugins.sampleAdded"), "success");
      setSelectedId(result.plugin.id);
      setVersion(result.plugin.installedVersion ?? result.plugin.latestVersion);
      setTab("installed");
      invalidatePluginQueries(result.plugin.id);
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.sampleFailed")), "error"),
  });

  const togglePlugin = useMutation({
    mutationFn: ({ id, action }: { id: string; action: "enable" | "disable" | "rollback" | "uninstall" }) => {
      if (action === "enable") return api.enablePlugin(id);
      if (action === "disable") return api.disablePlugin(id);
      if (action === "rollback") return api.rollbackPlugin(id);
      return api.uninstallPlugin(id, force);
    },
    onSuccess: (result, variables) => {
      const key =
        variables.action === "enable"
          ? "plugins.enabledToast"
          : variables.action === "disable"
            ? "plugins.disabledToast"
            : variables.action === "rollback"
              ? "plugins.rollbackToast"
              : "plugins.uninstallToast";
      pushToast(t(key), "success");
      setVersion(result.plugin.installedVersion ?? result.plugin.latestVersion);
      invalidatePluginQueries(result.plugin.id);
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.actionFailed")), "error"),
  });

  const trustKey = useMutation({
    mutationFn: () =>
      api.trustPluginKey({
        keyId: keyId.trim(),
        publicKey: publicKey.trim(),
        label: keyLabel.trim() || undefined,
        trust: keyTrust,
      }),
    onSuccess: () => {
      pushToast(t("plugins.trustAdded"), "success");
      setKeyId("");
      setPublicKey("");
      setKeyLabel("");
      void queryClient.invalidateQueries({ queryKey: ["plugin-keys"] });
      void queryClient.invalidateQueries({ queryKey: ["plugins"] });
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.trustFailed")), "error"),
  });

  const publishMarket = useMutation({
    mutationFn: () => api.publishMarketplace(publishPath.trim()),
    onSuccess: (result) => {
      pushToast(t("plugins.published"), "success");
      setMarketPluginId(result.pluginId);
      setMarketVersion(result.version);
      setReviewNote("");
      void queryClient.invalidateQueries({ queryKey: ["marketplace"] });
      void queryClient.invalidateQueries({ queryKey: ["market", result.pluginId, result.version] });
      invalidatePluginQueries();
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.publishFailed")), "error"),
  });

  const reviewMarket = useMutation({
    mutationFn: (status: "approved" | "rejected") =>
      api.reviewMarketVersion(
        marketPluginId ?? "",
        marketVersion ?? "",
        status,
        reviewNote.trim() || undefined,
      ),
    onSuccess: () => {
      pushToast(t("plugins.reviewed"), "success");
      setReviewNote("");
      void queryClient.invalidateQueries({ queryKey: ["marketplace"] });
      void queryClient.invalidateQueries({ queryKey: ["market", marketPluginId, marketVersion] });
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.reviewFailed")), "error"),
  });

  const withdrawMarket = useMutation({
    mutationFn: () => api.withdrawMarketVersion(marketPluginId ?? "", marketVersion ?? ""),
    onSuccess: () => {
      pushToast(t("plugins.withdrawnToast"), "success");
      void queryClient.invalidateQueries({ queryKey: ["marketplace"] });
      void queryClient.invalidateQueries({ queryKey: ["market", marketPluginId, marketVersion] });
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.withdrawFailed")), "error"),
  });

  const rateMarket = useMutation({
    mutationFn: () =>
      api.rateMarketVersion(
        marketPluginId ?? "",
        marketVersion ?? "",
        Number(rating),
        ratingComment.trim() || undefined,
      ),
    onSuccess: () => {
      pushToast(t("plugins.rated"), "success");
      setRatingComment("");
      void queryClient.invalidateQueries({ queryKey: ["marketplace"] });
      void queryClient.invalidateQueries({ queryKey: ["market", marketPluginId, marketVersion] });
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.rateFailed")), "error"),
  });

  const installMarket = useMutation({
    mutationFn: () => api.installMarketVersion(marketPluginId ?? "", marketVersion ?? ""),
    onSuccess: (result) => {
      pushToast(t("plugins.installVersionDone"), "success");
      setSelectedId(result.plugin.id);
      setVersion(result.plugin.installedVersion ?? result.plugin.latestVersion);
      setTab("installed");
      void queryClient.invalidateQueries({ queryKey: ["marketplace"] });
      void queryClient.invalidateQueries({ queryKey: ["market", marketPluginId, marketVersion] });
      invalidatePluginQueries(result.plugin.id);
    },
    onError: (error) => pushToast(errorMessage(error, t("plugins.installVersionFailed")), "error"),
  });

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase();
    if (!query) return pluginList;
    return pluginList.filter(
      (plugin) =>
        plugin.id.toLowerCase().includes(query) || plugin.name.toLowerCase().includes(query),
    );
  }, [pluginList, search]);

  const selected = detail.data;
  const selectedPackage = versionDetail.data?.package ?? null;
  const sbom = versionDetail.data?.sbom ?? selected?.sbom ?? null;
  const manifest = versionDetail.data?.manifest ?? null;

  function selectPlugin(id: string) {
    if (id === selectedId) return;
    setSelectedId(id);
    setVersion(null);
    setInspection(null);
  }

  function selectMarket(entry: MarketSummary) {
    setMarketPluginId(entry.pluginId);
    setMarketVersion(entry.version);
    setReviewNote("");
    setRatingComment("");
  }

  return (
    <div className="mx-auto grid max-w-[1560px] gap-3">
      <PageHeader
        title={t("plugins.title")}
        subtitle={t("plugins.subtitle")}
        actions={
          <>
            <Button variant="secondary" onClick={() => void queryClient.invalidateQueries()}>
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
            <Button
              variant="secondary"
              loading={samplePlugin.isPending}
              onClick={() => samplePlugin.mutate()}
            >
              <Sparkles className="h-3.5 w-3.5" />
              {t("plugins.addSample")}
            </Button>
            {tab === "installed" ? (
              <Button
                variant="primary"
                onClick={() => {
                  setInstallOpen((value) => !value);
                  setInspection(null);
                }}
              >
                <Package className="h-3.5 w-3.5" />
                {t("plugins.installOpen")}
              </Button>
            ) : null}
          </>
        }
      />

      <div className="flex flex-wrap gap-1">
        {(
          [
            ["installed", t("plugins.tabInstalled"), <Plug key="i" className="h-3 w-3" />],
            ["marketplace", t("plugins.tabMarketplace"), <Store key="m" className="h-3 w-3" />],
          ] as [PluginTab, string, ReactNode][]
        ).map(([key, label, icon]) => (
          <button
            key={key}
            type="button"
            onClick={() => setTab(key)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-sm border px-2.5 text-[11.5px] font-medium",
              tab === key
                ? "border-accent/40 bg-accent/10 text-accent"
                : "border-line bg-panel text-muted hover:text-ink",
            )}
          >
            {icon}
            {label}
          </button>
        ))}
        <span className="ml-auto self-center text-[10px] text-faint">{t("plugins.localOnly")}</span>
      </div>

      <div className="grid grid-cols-2 gap-2 min-[560px]:grid-cols-4">
        <Stat label={t("plugins.statInstalled")} value={installedCount} />
        <Stat label={t("plugins.statEnabled")} value={enabledCount} />
        <Stat label={t("plugins.statApproved")} value={approvedCount} />
        <Stat label={t("plugins.statPending")} value={pendingCount} />
      </div>

      {installOpen && tab === "installed" ? (
        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <Upload className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("plugins.installTitle")}</h2>
            <span className="text-[10.5px] text-faint">{t("plugins.installHint")}</span>
          </div>
          <div className="mt-2 grid gap-2 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-end">
            <Field label={t("plugins.installPath")}>
              <input
                className={inputClass}
                value={installPath}
                onChange={(event) => setInstallPath(event.target.value)}
                placeholder="/path/to/plugin"
              />
            </Field>
            <div className="flex flex-wrap items-center gap-1.5">
              <label className="inline-flex items-center gap-1.5 text-[11px] text-muted">
                <input
                  type="checkbox"
                  className="h-3.5 w-3.5 accent-accent"
                  checked={allowUnsigned}
                  onChange={(event) => setAllowUnsigned(event.target.checked)}
                />
                {t("plugins.allowUnsigned")}
              </label>
              <label className="inline-flex items-center gap-1.5 text-[11px] text-muted">
                <input
                  type="checkbox"
                  className="h-3.5 w-3.5 accent-accent"
                  checked={allowUntrusted}
                  onChange={(event) => setAllowUntrusted(event.target.checked)}
                />
                {t("plugins.allowUntrusted")}
              </label>
              <Button
                size="sm"
                variant="secondary"
                loading={inspectPlugin.isPending}
                disabled={!installPath.trim()}
                onClick={() => inspectPlugin.mutate()}
              >
                <ShieldCheck className="h-3.5 w-3.5" />
                {t("plugins.inspect")}
              </Button>
              <Button
                size="sm"
                variant="primary"
                loading={installPlugin.isPending}
                disabled={!installPath.trim()}
                onClick={() => installPlugin.mutate()}
              >
                {t("plugins.install")}
              </Button>
            </div>
          </div>
          {inspection ? (
            <div className="mt-3 grid gap-2 border-t border-line pt-3">
              <div className="flex flex-wrap items-center gap-1.5">
                <Badge tone={inspection.valid ? "success" : "danger"}>
                  {inspection.valid ? t("plugins.inspectValid") : t("plugins.inspectInvalid")}
                </Badge>
                <Badge tone={trustTone(inspection.trust)}>
                  {t(`plugins.trust.${inspection.trust}`, { defaultValue: inspection.trust })}
                </Badge>
                <span className="font-mono text-[9.5px] text-faint">
                  {inspection.contentHash.slice(0, 24)}
                </span>
                <span className="ml-auto text-[10px] text-faint">{t("plugins.inspectTitle")}</span>
              </div>
              <div className="grid gap-2 lg:grid-cols-2">
                <div>
                  <p className="mb-1 text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
                    {t("plugins.manifest")}
                  </p>
                  <JsonView value={inspection.manifest} maxHeight="180px" />
                </div>
                <SbomPanel sbom={inspection.sbom} />
              </div>
            </div>
          ) : null}
        </Panel>
      ) : null}

      {tab === "installed" ? (
        <div className="grid gap-3 xl:grid-cols-[260px_minmax(0,1fr)_320px]">
          <Panel className="p-3">
            <div className="flex items-center gap-2">
              <Plug className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("plugins.registry")}</h2>
              <span className="ml-auto font-mono text-[10px] text-faint">{pluginList.length}</span>
            </div>
            <input
              className={cn(inputClass, "mt-2")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("plugins.searchPlaceholder")}
              aria-label={t("common.search")}
            />
            <div className="mt-2 grid gap-1.5">
              {plugins.isLoading ? (
                <p className="px-1 py-2 text-xs text-muted">{t("common.loading")}</p>
              ) : plugins.isError ? (
                <p className="px-1 py-2 text-xs text-danger">{t("errors.loadFailed")}</p>
              ) : filtered.length === 0 ? (
                <EmptyState
                  icon={<Plug className="h-4 w-4" />}
                  title={t("plugins.empty")}
                  hint={t("plugins.emptyHint")}
                />
              ) : (
                filtered.map((plugin) => (
                  <button
                    key={plugin.id}
                    type="button"
                    onClick={() => selectPlugin(plugin.id)}
                    className={cn(
                      "rounded-sm border px-2.5 py-2 text-left transition-colors",
                      selectedId === plugin.id
                        ? "border-accent/45 bg-accent/10"
                        : "border-line bg-panel hover:border-line-strong",
                    )}
                  >
                    <div className="flex items-center gap-1.5">
                      <span className="min-w-0 flex-1 truncate text-[12px] font-semibold text-ink">
                        {plugin.name}
                      </span>
                      <Badge tone={statusTone(plugin.status)}>
                        {t(`plugins.status.${plugin.status}`, { defaultValue: plugin.status })}
                      </Badge>
                    </div>
                    <div className="mt-0.5 flex items-center gap-1.5 font-mono text-[9.5px] text-faint">
                      <span className="min-w-0 truncate">{plugin.id}</span>
                      <span>·</span>
                      <span>{plugin.installedVersion ?? plugin.latestVersion}</span>
                    </div>
                  </button>
                ))
              )}
            </div>
          </Panel>

          <div className="grid min-w-0 content-start gap-3">
            {detail.isLoading ? (
              <Panel className="p-4 text-xs text-muted">{t("common.loading")}</Panel>
            ) : detail.isError ? (
              <Panel className="p-4 text-xs text-danger">{t("errors.loadFailed")}</Panel>
            ) : !selected ? (
              <Panel className="p-4">
                <EmptyState
                  icon={<Plug className="h-4 w-4" />}
                  title={t("plugins.selectPlugin")}
                  hint={t("plugins.selectPluginHint")}
                />
              </Panel>
            ) : (
              <>
                <Panel className="p-4">
                  <div className="flex flex-wrap items-start gap-2">
                    <div className="min-w-0">
                      <h2 className="truncate text-[15px] font-semibold text-ink-strong">
                        {selected.plugin.name}
                      </h2>
                      <p className="mt-0.5 break-all font-mono text-[10px] text-faint">
                        {selected.plugin.id} · {selected.plugin.latestVersion}
                      </p>
                    </div>
                    <Badge tone={statusTone(selected.plugin.status)}>
                      {t(`plugins.status.${selected.plugin.status}`, {
                        defaultValue: selected.plugin.status,
                      })}
                    </Badge>
                    <Badge tone={trustTone(selected.plugin.trust)}>
                      {t(`plugins.trust.${selected.plugin.trust}`, {
                        defaultValue: selected.plugin.trust,
                      })}
                    </Badge>
                    <span className="ml-auto font-mono text-[9.5px] text-faint">
                      {selectedPackage?.contentHash.slice(0, 12) ?? "—"}
                    </span>
                  </div>
                  <dl className="mt-3 grid grid-cols-2 gap-2 xl:grid-cols-4">
                    <Info label={t("plugins.trustLabel")} value={selected.plugin.trust} mono />
                    <Info label={t("plugins.license")} value={selected.plugin.license} mono />
                    <Info
                      label={t("plugins.installedVersion")}
                      value={selected.plugin.installedVersion ?? t("common.none")}
                      mono
                    />
                    <Info
                      label={t("plugins.previousVersion")}
                      value={selected.plugin.previousVersion ?? t("common.none")}
                      mono
                    />
                  </dl>
                  <div className="mt-3 flex flex-wrap items-center gap-1.5">
                    <Button
                      size="sm"
                      variant="secondary"
                      loading={togglePlugin.isPending && togglePlugin.variables?.action === "enable"}
                      disabled={selected.plugin.status === "enabled" || !selected.plugin.installedVersion}
                      onClick={() =>
                        togglePlugin.mutate({ id: selected.plugin.id, action: "enable" })
                      }
                    >
                      <Power className="h-3.5 w-3.5" />
                      {t("plugins.enable")}
                    </Button>
                    <Button
                      size="sm"
                      variant="secondary"
                      loading={togglePlugin.isPending && togglePlugin.variables?.action === "disable"}
                      disabled={selected.plugin.status !== "enabled"}
                      onClick={() =>
                        togglePlugin.mutate({ id: selected.plugin.id, action: "disable" })
                      }
                    >
                      <PowerOff className="h-3.5 w-3.5" />
                      {t("plugins.disable")}
                    </Button>
                    <Button
                      size="sm"
                      variant="secondary"
                      loading={togglePlugin.isPending && togglePlugin.variables?.action === "rollback"}
                      disabled={!selected.plugin.previousVersion}
                      onClick={() =>
                        togglePlugin.mutate({ id: selected.plugin.id, action: "rollback" })
                      }
                    >
                      <RotateCcw className="h-3.5 w-3.5" />
                      {t("plugins.rollback")}
                    </Button>
                    <label className="ml-auto inline-flex items-center gap-1.5 text-[10.5px] text-muted">
                      <input
                        type="checkbox"
                        className="h-3.5 w-3.5 accent-accent"
                        checked={force}
                        onChange={(event) => setForce(event.target.checked)}
                      />
                      {t("plugins.forceUninstall")}
                    </label>
                    <Button
                      size="sm"
                      variant="danger"
                      loading={togglePlugin.isPending && togglePlugin.variables?.action === "uninstall"}
                      disabled={!selected.plugin.installedVersion && selected.plugin.status === "uninstalled"}
                      onClick={() =>
                        togglePlugin.mutate({ id: selected.plugin.id, action: "uninstall" })
                      }
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      {t("plugins.uninstall")}
                    </Button>
                  </div>
                </Panel>

                <Panel className="p-4">
                  <div className="flex items-center gap-2">
                    <Boxes className="h-4 w-4 text-accent" />
                    <h2 className="text-sm font-semibold text-ink">{t("plugins.capabilities")}</h2>
                    <span className="ml-auto font-mono text-[10px] text-faint">
                      {selected.capabilities.length}
                    </span>
                  </div>
                  {selected.capabilities.length === 0 ? (
                    <p className="mt-2 text-xs text-muted">{t("plugins.noCapabilities")}</p>
                  ) : (
                    <div className="mt-2 overflow-x-auto">
                      <table className="w-full min-w-[500px] text-left text-[11px]">
                        <thead className="text-[9.5px] uppercase tracking-[0.12em] text-faint">
                          <tr>
                            <th className="px-2 py-1 font-medium">{t("plugins.capabilityKind")}</th>
                            <th className="px-2 py-1 font-medium">{t("plugins.capabilityId")}</th>
                            <th className="px-2 py-1 font-medium">
                              {t("plugins.capabilityVersion")}
                            </th>
                            <th className="px-2 py-1 font-medium">{t("plugins.capabilityPath")}</th>
                            <th className="px-2 py-1 font-medium">{t("common.status")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {selected.capabilities.map((capability) => (
                            <tr
                              key={`${capability.kind}-${capability.capabilityId}`}
                              className="border-t border-line"
                            >
                              <td className="px-2 py-1.5">
                                <Badge tone="accent">{capability.kind}</Badge>
                              </td>
                              <td className="px-2 py-1.5 font-mono text-ink">
                                {capability.capabilityId}
                              </td>
                              <td className="px-2 py-1.5 font-mono text-muted">
                                {capability.capabilityVersion}
                              </td>
                              <td className="max-w-[120px] truncate px-2 py-1.5 font-mono text-[9.5px] text-faint">
                                {capability.path}
                              </td>
                              <td className="px-2 py-1.5">
                                <Badge
                                  tone={
                                    capability.status === "enabled"
                                      ? "success"
                                      : capability.status === "disabled"
                                        ? "warning"
                                        : "neutral"
                                  }
                                >
                                  {t(`plugins.status.${capability.status}`, {
                                    defaultValue: capability.status,
                                  })}
                                </Badge>
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                </Panel>

                <Panel className="p-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <Package className="h-4 w-4 text-accent" />
                    <h2 className="text-sm font-semibold text-ink">{t("plugins.packages")}</h2>
                    <select
                      className={cn(inputClass, "ml-auto h-7 w-auto font-mono text-[11px]")}
                      value={version ?? ""}
                      onChange={(event) => setVersion(event.target.value || null)}
                      aria-label={t("common.version")}
                    >
                      {selected.packages.map((entry) => (
                        <option key={entry.version} value={entry.version}>
                          {entry.version}
                        </option>
                      ))}
                    </select>
                  </div>
                  {selected.packages.length === 0 ? (
                    <p className="mt-2 text-xs text-muted">{t("plugins.noPackages")}</p>
                  ) : (
                    <div className="mt-2 overflow-x-auto">
                      <table className="w-full min-w-[560px] text-left text-[11px]">
                        <thead className="text-[9.5px] uppercase tracking-[0.12em] text-faint">
                          <tr>
                            <th className="px-2 py-1 font-medium">{t("common.version")}</th>
                            <th className="px-2 py-1 font-medium">{t("plugins.packageTrust")}</th>
                            <th className="px-2 py-1 font-medium">{t("plugins.packageSource")}</th>
                            <th className="px-2 py-1 font-medium">{t("plugins.packageHash")}</th>
                            <th className="px-2 py-1 font-medium">{t("common.createdAt")}</th>
                            <th className="px-2 py-1 font-medium" />
                          </tr>
                        </thead>
                        <tbody>
                          {selected.packages.map((entry) => (
                            <tr key={entry.version} className="border-t border-line">
                              <td className="px-2 py-1.5 font-mono text-ink">
                                {entry.version}
                                {selected.plugin.installedVersion === entry.version ? (
                                  <span className="ml-1 text-[9px] text-accent">
                                    {t("plugins.installedTag")}
                                  </span>
                                ) : null}
                              </td>
                              <td className="px-2 py-1.5">
                                <Badge tone={trustTone(entry.trust)}>
                                  {t(`plugins.trust.${entry.trust}`, {
                                    defaultValue: entry.trust,
                                  })}
                                </Badge>
                              </td>
                              <td className="px-2 py-1.5 font-mono text-muted">{entry.source}</td>
                              <td className="px-2 py-1.5 font-mono text-[9.5px] text-faint">
                                {entry.contentHash.slice(0, 16)}
                              </td>
                              <td className="px-2 py-1.5 text-faint">
                                <Timestamp value={entry.createdAt} />
                              </td>
                              <td className="px-2 py-1.5 text-right">
                                <Button
                                  size="xs"
                                  variant="ghost"
                                  onClick={() => setVersion(entry.version)}
                                >
                                  {t("common.open")}
                                </Button>
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                  {versionDetail.isError ? (
                    <p className="mt-2 text-xs text-danger">{t("errors.loadFailed")}</p>
                  ) : null}
                </Panel>

                {versionDetail.data ? (
                  <>
                    <Panel className="p-4">
                      <div className="flex flex-wrap items-center gap-2">
                        <FileJson2 className="h-4 w-4 text-accent" />
                        <h2 className="text-sm font-semibold text-ink">{t("plugins.packageDetail")}</h2>
                        <span className="font-mono text-[10px] text-faint">
                          {versionDetail.data.package.version}
                        </span>
                        <span className="ml-auto font-mono text-[9.5px] text-faint">
                          {versionDetail.data.package.contentHash.slice(0, 24)}
                        </span>
                      </div>
                      <div className="mt-2 grid gap-2 md:grid-cols-2">
                        <div>
                          <SubTitle>{t("plugins.manifest")}</SubTitle>
                          <JsonView value={manifest} maxHeight="220px" />
                        </div>
                        <div className="grid gap-2">
                          <div>
                            <SubTitle>{t("plugins.permissions")}</SubTitle>
                            <JsonView
                              value={(manifest as { permissions?: unknown } | null)?.permissions ?? []}
                              maxHeight="90px"
                            />
                          </div>
                          <div>
                            <SubTitle>{t("plugins.dependencies")}</SubTitle>
                            <JsonView
                              value={(manifest as { dependencies?: unknown } | null)?.dependencies ?? []}
                              maxHeight="90px"
                            />
                          </div>
                        </div>
                      </div>
                      <div className="mt-2 rounded-sm border border-line bg-panel-2 p-2">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <Fingerprint className="h-3.5 w-3.5 text-accent" />
                          <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
                            {t("plugins.signature")}
                          </span>
                          {versionDetail.data.package.signerKeyId ? (
                            <Badge tone="success">{t("plugins.signed")}</Badge>
                          ) : (
                            <Badge tone="danger">{t("plugins.unsigned")}</Badge>
                          )}
                          <span className="ml-auto font-mono text-[9.5px] text-faint">
                            {versionDetail.data.package.signerKeyId ?? "—"}
                          </span>
                        </div>
                        {versionDetail.data.package.signerPublicKey ? (
                          <p className="mt-1 break-all font-mono text-[9px] text-muted">
                            {versionDetail.data.package.signerPublicKey}
                          </p>
                        ) : null}
                        {versionDetail.data.package.signature ? (
                          <JsonView
                            value={versionDetail.data.package.signature}
                            maxHeight="90px"
                            className="mt-1"
                          />
                        ) : null}
                      </div>
                    </Panel>

                    <Panel className="p-4">
                      <SbomPanel sbom={sbom} />
                    </Panel>
                  </>
                ) : null}

                <Panel className="p-4">
                  <div className="flex items-center gap-2">
                    <ShieldCheck className="h-4 w-4 text-accent" />
                    <h2 className="text-sm font-semibold text-ink">{t("plugins.events")}</h2>
                    <span className="ml-auto font-mono text-[10px] text-faint">
                      {selected.events.length}
                    </span>
                  </div>
                  {selected.events.length === 0 ? (
                    <p className="mt-2 text-xs text-muted">{t("plugins.noEvents")}</p>
                  ) : (
                    <div className="mt-2 grid gap-1">
                      {selected.events.slice(0, 10).map((event) => (
                        <div
                          key={event.id}
                          className="flex flex-wrap items-center gap-2 rounded-sm border border-line px-2 py-1.5"
                        >
                          <span className="font-mono text-[10px] text-accent">{event.kind}</span>
                          <span className="font-mono text-[9.5px] text-faint">
                            {event.version ?? "—"}
                          </span>
                          <span className="ml-auto text-[9.5px] text-faint">
                            <Timestamp value={event.at} />
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                </Panel>
              </>
            )}
          </div>

          <div className="grid content-start gap-3">
            <KeyPanels
              signing={keyInfo.data}
              trusted={trustedKeys.data?.keys ?? []}
              loading={trustedKeys.isLoading}
              keyId={keyId}
              publicKey={publicKey}
              keyLabel={keyLabel}
              keyTrust={keyTrust}
              onKeyId={setKeyId}
              onPublicKey={setPublicKey}
              onKeyLabel={setKeyLabel}
              onKeyTrust={setKeyTrust}
              busy={trustKey.isPending}
              onSubmit={() => trustKey.mutate()}
            />
          </div>
        </div>
      ) : (
        <div className="grid gap-3 xl:grid-cols-[300px_minmax(0,1fr)_320px]">
          <div className="grid content-start gap-3">
            <Panel className="p-4">
              <div className="flex flex-wrap items-center gap-2">
                <Upload className="h-4 w-4 text-accent" />
                <h2 className="text-sm font-semibold text-ink">{t("plugins.publishTitle")}</h2>
              </div>
              <p className="mt-1 text-[10.5px] text-faint">{t("plugins.publishHint")}</p>
              <input
                className={cn(inputClass, "mt-2 font-mono text-[11px]")}
                value={publishPath}
                onChange={(event) => setPublishPath(event.target.value)}
                placeholder="/path/to/signed-plugin"
                aria-label={t("plugins.publishPath")}
              />
              <Button
                size="sm"
                variant="primary"
                className="mt-2"
                loading={publishMarket.isPending}
                disabled={!publishPath.trim()}
                onClick={() => publishMarket.mutate()}
              >
                <Store className="h-3.5 w-3.5" />
                {t("plugins.publish")}
              </Button>
            </Panel>

            <Panel className="p-3">
              <div className="flex items-center gap-2">
                <Store className="h-4 w-4 text-accent" />
                <h2 className="text-sm font-semibold text-ink">{t("plugins.listings")}</h2>
                <span className="ml-auto font-mono text-[10px] text-faint">{marketList.length}</span>
              </div>
              {marketplace.isLoading ? (
                <p className="px-1 py-2 text-xs text-muted">{t("common.loading")}</p>
              ) : marketplace.isError ? (
                <p className="px-1 py-2 text-xs text-danger">{t("errors.loadFailed")}</p>
              ) : marketList.length === 0 ? (
                <EmptyState
                  icon={<Store className="h-4 w-4" />}
                  title={t("plugins.noListings")}
                  hint={t("plugins.noListingsHint")}
                />
              ) : (
                <div className="mt-2 grid gap-1.5">
                  {marketList.map((entry) => (
                    <button
                      key={`${entry.pluginId}@${entry.version}`}
                      type="button"
                      onClick={() => selectMarket(entry)}
                      className={cn(
                        "rounded-sm border px-2.5 py-2 text-left transition-colors",
                        marketPluginId === entry.pluginId && marketVersion === entry.version
                          ? "border-accent/45 bg-accent/10"
                          : "border-line bg-panel hover:border-line-strong",
                      )}
                    >
                      <div className="flex items-center gap-1.5">
                        <span className="min-w-0 flex-1 truncate text-[12px] font-semibold text-ink">
                          {entry.name}
                        </span>
                        <Badge tone={reviewTone(entry.reviewStatus)}>
                          {t(`plugins.reviewStatus.${entry.reviewStatus}`, {
                            defaultValue: entry.reviewStatus,
                          })}
                        </Badge>
                      </div>
                      <div className="mt-0.5 flex items-center gap-1.5 font-mono text-[9.5px] text-faint">
                        <span className="min-w-0 truncate">
                          {entry.pluginId} · {entry.version}
                        </span>
                      </div>
                      <div className="mt-1 flex flex-wrap items-center gap-1">
                        <Badge tone={trustTone(entry.trust)}>
                          {t(`plugins.trust.${entry.trust}`, { defaultValue: entry.trust })}
                        </Badge>
                        <span className="font-mono text-[9px] text-faint">{entry.license}</span>
                        <span className="ml-auto inline-flex items-center gap-1 font-mono text-[9.5px] text-faint">
                          <Star className="h-2.5 w-2.5" />
                          {(entry.rating.average ?? 0).toFixed(1)}
                          <span>·</span>
                          {entry.downloads}
                        </span>
                      </div>
                    </button>
                  ))}
                </div>
              )}
            </Panel>
          </div>

          <div className="grid min-w-0 content-start gap-3">
            {marketDetail.isLoading ? (
              <Panel className="p-4 text-xs text-muted">{t("common.loading")}</Panel>
            ) : marketDetail.isError ? (
              <Panel className="p-4 text-xs text-danger">{t("errors.loadFailed")}</Panel>
            ) : !marketDetail.data ? (
              <Panel className="p-4">
                <EmptyState
                  icon={<Store className="h-4 w-4" />}
                  title={t("plugins.selectListing")}
                  hint={t("plugins.selectListingHint")}
                />
              </Panel>
            ) : (
              <MarketDetailPanel
                detail={marketDetail.data}
                note={reviewNote}
                onNote={setReviewNote}
                rating={rating}
                onRating={setRating}
                ratingComment={ratingComment}
                onRatingComment={setRatingComment}
                reviewBusy={reviewMarket.isPending}
                withdrawBusy={withdrawMarket.isPending}
                rateBusy={rateMarket.isPending}
                installBusy={installMarket.isPending}
                onReview={(status) => reviewMarket.mutate(status)}
                onWithdraw={() => withdrawMarket.mutate()}
                onRate={() => rateMarket.mutate()}
                onInstall={() => installMarket.mutate()}
              />
            )}
          </div>

          <div className="grid content-start gap-3">
            <KeyPanels
              signing={keyInfo.data}
              trusted={trustedKeys.data?.keys ?? []}
              loading={trustedKeys.isLoading}
              keyId={keyId}
              publicKey={publicKey}
              keyLabel={keyLabel}
              keyTrust={keyTrust}
              onKeyId={setKeyId}
              onPublicKey={setPublicKey}
              onKeyLabel={setKeyLabel}
              onKeyTrust={setKeyTrust}
              busy={trustKey.isPending}
              onSubmit={() => trustKey.mutate()}
            />
          </div>
        </div>
      )}
    </div>
  );
}

function MarketDetailPanel({
  detail,
  note,
  onNote,
  rating,
  onRating,
  ratingComment,
  onRatingComment,
  reviewBusy,
  withdrawBusy,
  rateBusy,
  installBusy,
  onReview,
  onWithdraw,
  onRate,
  onInstall,
}: {
  detail: MarketDetail;
  note: string;
  onNote: (value: string) => void;
  rating: string;
  onRating: (value: string) => void;
  ratingComment: string;
  onRatingComment: (value: string) => void;
  reviewBusy: boolean;
  withdrawBusy: boolean;
  rateBusy: boolean;
  installBusy: boolean;
  onReview: (status: "approved" | "rejected") => void;
  onWithdraw: () => void;
  onRate: () => void;
  onInstall: () => void;
}) {
  const { t } = useTranslation();
  const manifest = detail.manifest as {
    permissions?: unknown;
    dependencies?: unknown;
    signature?: unknown;
  } | null;
  const pending = detail.reviewStatus === "pending";
  const withdrawn = Boolean(detail.withdrawnAt) || detail.reviewStatus === "withdrawn";
  const approved = detail.reviewStatus === "approved" && !withdrawn;
  return (
    <>
      <Panel className="p-4">
        <div className="flex flex-wrap items-start gap-2">
          <div className="min-w-0">
            <h2 className="truncate text-[15px] font-semibold text-ink-strong">
              {detail.package.name}
            </h2>
            <p className="mt-0.5 break-all font-mono text-[10px] text-faint">
              {detail.pluginId} · {detail.version} · {detail.package.contentHash.slice(0, 20)}
            </p>
          </div>
          <Badge tone={reviewTone(detail.reviewStatus)}>
            {t(`plugins.reviewStatus.${detail.reviewStatus}`, { defaultValue: detail.reviewStatus })}
          </Badge>
          <Badge tone={trustTone(detail.package.trust)}>
            {t(`plugins.trust.${detail.package.trust}`, { defaultValue: detail.package.trust })}
          </Badge>
          <div className="ml-auto flex flex-wrap gap-1.5">
            <Button
              size="sm"
              variant="primary"
              loading={installBusy}
              disabled={!approved}
              onClick={onInstall}
            >
              <Package className="h-3.5 w-3.5" />
              {t("plugins.installVersion")}
            </Button>
            <Button
              size="sm"
              variant="secondary"
              loading={withdrawBusy}
              disabled={withdrawn}
              onClick={onWithdraw}
            >
              {t("plugins.withdraw")}
            </Button>
          </div>
        </div>

        <dl className="mt-3 grid grid-cols-2 gap-2 xl:grid-cols-4">
          <Info label={t("plugins.publisher")} value={detail.package.publisher ?? "—"} />
          <Info label={t("plugins.license")} value={detail.package.license} mono />
          <Info
            label={t("plugins.rating")}
            value={`${(detail.rating.average ?? 0).toFixed(1)} (${detail.rating.count})`}
            mono
          />
          <Info label={t("plugins.downloads")} value={String(detail.downloads)} mono />
        </dl>

        {pending ? (
          <p className="mt-3 rounded-sm border border-warning/30 bg-warning/10 px-2 py-1.5 text-[10.5px] text-warning">
            {t("plugins.pendingReviewHint")}
          </p>
        ) : null}
        {detail.reviewNote ? (
          <p className="mt-2 rounded-sm border border-line bg-panel-2 px-2 py-1.5 text-[10.5px] text-muted">
            {t("plugins.reviewNoteLabel")}: {detail.reviewNote}
          </p>
        ) : null}
        {!approved && !pending ? (
          <p className="mt-2 text-[10.5px] text-faint">{t("plugins.approvedOnly")}</p>
        ) : null}

        <div className="mt-3 grid gap-2 md:grid-cols-2">
          <div>
            <SubTitle>{t("plugins.manifest")}</SubTitle>
            <JsonView value={detail.manifest} maxHeight="200px" />
          </div>
          <div className="grid gap-2">
            <div>
              <SubTitle>{t("plugins.permissions")}</SubTitle>
              <JsonView value={manifest?.permissions ?? []} maxHeight="80px" />
            </div>
            <div>
              <SubTitle>{t("plugins.dependencies")}</SubTitle>
              <JsonView value={manifest?.dependencies ?? []} maxHeight="80px" />
            </div>
            <div>
              <SubTitle>{t("plugins.signature")}</SubTitle>
              <div className="flex flex-wrap items-center gap-1.5 rounded-sm border border-line bg-panel-2 px-2 py-1.5">
                <Badge tone={detail.package.signerKeyId ? "success" : "danger"}>
                  {detail.package.signerKeyId ? t("plugins.signed") : t("plugins.unsigned")}
                </Badge>
                <span className="min-w-0 flex-1 truncate font-mono text-[9.5px] text-faint">
                  {detail.package.signerKeyId ?? "—"}
                </span>
              </div>
              {detail.package.signerPublicKey ? (
                <p className="mt-1 break-all font-mono text-[9px] text-faint">
                  {detail.package.signerPublicKey}
                </p>
              ) : null}
              {detail.package.signature ? (
                <JsonView value={detail.package.signature} maxHeight="80px" className="mt-1" />
              ) : null}
            </div>
          </div>
        </div>
      </Panel>

      <Panel className="p-4">
        <SbomPanel sbom={detail.sbom ?? null} />
      </Panel>

      <div className="grid gap-3 md:grid-cols-2">
        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("plugins.reviewTitle")}</h2>
            <Badge tone={reviewTone(detail.reviewStatus)}>
              {t(`plugins.reviewStatus.${detail.reviewStatus}`, {
                defaultValue: detail.reviewStatus,
              })}
            </Badge>
          </div>
          <textarea
            className={cn(textareaClass, "mt-2 min-h-[70px]")}
            value={note}
            onChange={(event) => onNote(event.target.value)}
            placeholder={t("plugins.reviewNotePlaceholder")}
            aria-label={t("plugins.reviewNote")}
          />
          <div className="mt-2 flex flex-wrap gap-1.5">
            <Button
              size="sm"
              variant="primary"
              loading={reviewBusy}
              disabled={!pending}
              onClick={() => onReview("approved")}
            >
              <CheckCircle2 className="h-3.5 w-3.5" />
              {t("common.approve")}
            </Button>
            <Button
              size="sm"
              variant="danger"
              loading={reviewBusy}
              disabled={!pending}
              onClick={() => onReview("rejected")}
            >
              <XCircle className="h-3.5 w-3.5" />
              {t("common.reject")}
            </Button>
          </div>
        </Panel>

        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <Star className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("plugins.rateTitle")}</h2>
          </div>
          <div className="mt-2 grid grid-cols-[72px_minmax(0,1fr)] gap-2">
            <Field label={t("plugins.rating")}>
              <select
                className={inputClass}
                value={rating}
                onChange={(event) => onRating(event.target.value)}
              >
                {[5, 4, 3, 2, 1].map((value) => (
                  <option key={value} value={String(value)}>
                    {value}
                  </option>
                ))}
              </select>
            </Field>
            <Field label={t("plugins.ratingComment")}>
              <input
                className={inputClass}
                value={ratingComment}
                onChange={(event) => onRatingComment(event.target.value)}
              />
            </Field>
          </div>
          <Button
            size="sm"
            variant="secondary"
            className="mt-2"
            loading={rateBusy}
            onClick={onRate}
          >
            {t("plugins.rate")}
          </Button>
        </Panel>
      </div>

      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Star className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("plugins.reviews")}</h2>
          <span className="ml-auto font-mono text-[10px] text-faint">
            {detail.reviews.length}
          </span>
        </div>
        {detail.reviews.length === 0 ? (
          <p className="mt-2 text-xs text-muted">{t("plugins.noReviews")}</p>
        ) : (
          <div className="mt-2 grid gap-1.5">
            {detail.reviews.map((review) => (
              <div key={review.id} className="rounded-sm border border-line px-2.5 py-2">
                <div className="flex flex-wrap items-center gap-1.5">
                  <Badge tone="accent">{review.rating}/5</Badge>
                  <span className="text-[11px] text-ink">{review.comment ?? ""}</span>
                  <span className="ml-auto text-[9.5px] text-faint">
                    <Timestamp value={review.createdAt} />
                  </span>
                </div>
              </div>
            ))}
          </div>
        )}
      </Panel>
    </>
  );
}

function KeyPanels({
  signing,
  trusted,
  loading,
  keyId,
  publicKey,
  keyLabel,
  keyTrust,
  onKeyId,
  onPublicKey,
  onKeyLabel,
  onKeyTrust,
  busy,
  onSubmit,
}: {
  signing?: { keyId: string; publicKey: string; algorithm: string; trust: string };
  trusted: { keyId: string; publicKey: string; label: string; trust: string; createdAt: string }[];
  loading: boolean;
  keyId: string;
  publicKey: string;
  keyLabel: string;
  keyTrust: "verified" | "community";
  onKeyId: (value: string) => void;
  onPublicKey: (value: string) => void;
  onKeyLabel: (value: string) => void;
  onKeyTrust: (value: "verified" | "community") => void;
  busy: boolean;
  onSubmit: () => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <KeyRound className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("plugins.signingKey")}</h2>
        </div>
        {signing ? (
          <div className="mt-2 grid gap-1.5">
            <div className="flex flex-wrap items-center gap-1.5">
              <Badge tone={trustTone(signing.trust)}>{signing.algorithm}</Badge>
              <span className="font-mono text-[10px] text-ink">{signing.keyId}</span>
            </div>
            <p className="break-all font-mono text-[9px] text-faint">{signing.publicKey}</p>
            <p className="text-[10px] text-faint">{t("plugins.signingKeyHint")}</p>
          </div>
        ) : (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        )}
      </Panel>

      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Fingerprint className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("plugins.trustedKeys")}</h2>
          <span className="ml-auto font-mono text-[10px] text-faint">{trusted.length}</span>
        </div>
        {loading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : trusted.length === 0 ? (
          <p className="mt-2 text-xs text-muted">{t("plugins.noTrustedKeys")}</p>
        ) : (
          <div className="mt-2 grid gap-1">
            {trusted.map((key) => (
              <div key={key.keyId} className="rounded-sm border border-line px-2 py-1.5">
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink">
                    {key.keyId}
                  </span>
                  <Badge tone={trustTone(key.trust)}>
                    {t(`plugins.trust.${key.trust}`, { defaultValue: key.trust })}
                  </Badge>
                </div>
                <p className="truncate text-[9.5px] text-faint">{key.label}</p>
              </div>
            ))}
          </div>
        )}

        <div className="mt-3 grid gap-1.5 border-t border-line pt-2">
          <p className="text-[10.5px] font-semibold text-muted">{t("plugins.addTrustedKey")}</p>
          <input
            className={inputClass}
            value={keyId}
            onChange={(event) => onKeyId(event.target.value)}
            placeholder={t("plugins.keyId")}
            aria-label={t("plugins.keyId")}
          />
          <textarea
            className={cn(textareaClass, "min-h-[54px] text-[10px]")}
            value={publicKey}
            onChange={(event) => onPublicKey(event.target.value)}
            placeholder={t("plugins.publicKey")}
            aria-label={t("plugins.publicKey")}
          />
          <div className="grid grid-cols-[minmax(0,1fr)_96px] gap-1.5">
            <input
              className={inputClass}
              value={keyLabel}
              onChange={(event) => onKeyLabel(event.target.value)}
              placeholder={t("plugins.keyLabel")}
              aria-label={t("plugins.keyLabel")}
            />
            <select
              className={inputClass}
              value={keyTrust}
              onChange={(event) => onKeyTrust(event.target.value as "verified" | "community")}
              aria-label={t("plugins.keyTrust")}
            >
              <option value="verified">{t("plugins.trust.verified")}</option>
              <option value="community">{t("plugins.trust.community")}</option>
            </select>
          </div>
          <Button
            size="sm"
            variant="secondary"
            loading={busy}
            disabled={!keyId.trim() || !publicKey.trim()}
            onClick={onSubmit}
          >
            <KeyRound className="h-3.5 w-3.5" />
            {t("plugins.trustKey")}
          </Button>
        </div>
      </Panel>
    </>
  );
}

function SbomPanel({ sbom }: { sbom: PluginSbom | null }) {
  const { t } = useTranslation();
  if (!sbom) {
    return (
      <div>
        <SubTitle>{t("plugins.sbom")}</SubTitle>
        <p className="text-xs text-muted">{t("plugins.noSbom")}</p>
      </div>
    );
  }
  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <SubTitle>{t("plugins.sbom")}</SubTitle>
        <span className="ml-auto font-mono text-[9.5px] text-faint">{sbom.schemaVersion}</span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        <Badge tone={sbom.gates.license ? "success" : "danger"}>
          {t("plugins.gateLicense")}
        </Badge>
        <Badge tone={sbom.gates.signature ? "success" : "danger"}>
          {t("plugins.gateSignature")}
        </Badge>
        <Badge tone={sbom.gates.staticScan === "pass" ? "success" : "warning"}>
          {t("plugins.gateScan")}: {sbom.gates.staticScan}
        </Badge>
        <Badge tone={trustTone(sbom.trust)}>
          {t(`plugins.trust.${sbom.trust}`, { defaultValue: sbom.trust })}
        </Badge>
      </div>
      <div>
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
            {t("plugins.sbomFiles")}
          </span>
          <span className="ml-auto font-mono text-[9px] text-faint">{sbom.files.length}</span>
        </div>
        <div className="hpa-scroll mt-1 max-h-[130px] overflow-auto rounded-sm border border-line bg-panel-2">
          {sbom.files.map((file) => (
            <div
              key={file.path}
              className="flex items-center gap-2 border-b border-line px-2 py-1 last:border-b-0"
            >
              <span className="min-w-0 flex-1 truncate font-mono text-[9.5px] text-ink">
                {file.path}
              </span>
              <span className="shrink-0 font-mono text-[9px] text-faint">{file.size} B</span>
              <span className="hidden shrink-0 font-mono text-[9px] text-faint min-[420px]:inline">
                {file.sha256.slice(0, 10)}
              </span>
            </div>
          ))}
        </div>
      </div>
      {sbom.warnings.length > 0 ? (
        <div>
          <span className="text-[10px] font-semibold uppercase tracking-[0.12em] text-warning">
            {t("plugins.sbomWarnings")}
          </span>
          <div className="mt-1 grid gap-1">
            {sbom.warnings.map((warning) => (
              <p
                key={`${warning.path}-${warning.pattern}`}
                className="break-all rounded-sm border border-warning/25 bg-warning/5 px-2 py-1 font-mono text-[9.5px] text-warning"
              >
                {warning.pattern} · {warning.path}
              </p>
            ))}
          </div>
        </div>
      ) : (
        <p className="text-[10px] text-success">{t("plugins.noWarnings")}</p>
      )}
      <div className="grid gap-1 text-[10px] text-faint">
        <span className="font-mono">
          {t("plugins.signerKeyId")}: {sbom.signerKeyId ?? "—"}
        </span>
        <span className="break-all font-mono">
          {t("plugins.signature")}: {sbom.publicKey ?? "—"}
        </span>
      </div>
    </div>
  );
}

function SubTitle({ children }: { children: ReactNode }) {
  return (
    <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">{children}</p>
  );
}

function Info({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-1.5">
      <dt className="text-[8.5px] uppercase tracking-[0.12em] text-faint">{label}</dt>
      <dd className={cn("mt-0.5 truncate text-[11px] text-ink", mono && "font-mono")}>{value}</dd>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-sm border border-line bg-panel px-3 py-2">
      <p className="text-[9px] uppercase tracking-[0.14em] text-faint">{label}</p>
      <p className="mt-0.5 font-mono text-sm text-ink">{value}</p>
    </div>
  );
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) return error.message || fallback;
  return error instanceof Error ? error.message : fallback;
}
