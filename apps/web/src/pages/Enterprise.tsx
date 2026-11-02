import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  Activity,
  Ban,
  Building2,
  DatabaseBackup,
  Download,
  Gauge,
  HardDrive,
  KeyRound,
  ListChecks,
  LockKeyhole,
  LogIn,
  LogOut,
  Plus,
  PowerOff,
  RefreshCw,
  RotateCw,
  ScrollText,
  Send,
  Server,
  ShieldCheck,
  Trash2,
  Users,
  Webhook,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  CopyButton,
  EmptyState,
  Field,
  PageHeader,
  Panel,
  Timestamp,
  cn,
  inputClass,
  textareaClass,
  type BadgeTone,
} from "../components/ui";
import {
  ApiError,
  api,
  clearEnterpriseToken,
  readEnterpriseToken,
  writeEnterpriseToken,
} from "../lib/api";
import { useAuth } from "../lib/session";
import type {
  AuthSession,
  EnterpriseAuditEntry,
  EnterpriseBackup,
  EnterpriseMember,
  EnterpriseQuota,
  EnterpriseRole,
  EnterpriseTenant,
  EnterpriseToken,
  JsonValue,
} from "../lib/types";
import { useUiStore } from "../stores/ui";
import { OAuthSection } from "./EnterpriseOAuth";

type EnterpriseTab =
  | "overview"
  | "access"
  | "tenants"
  | "tokens"
  | "members"
  | "quotas"
  | "workers"
  | "backups"
  | "audit"
  | "oidc"
  | "oauth";

const roles: EnterpriseRole[] = [
  "owner",
  "admin",
  "developer",
  "publisher",
  "operator",
  "approver",
  "auditor",
  "viewer",
];

const quotaFields = [
  "maxWorkflows",
  "maxExecutionsPerDay",
  "maxSkills",
  "maxAgents",
  "maxPlugins",
  "maxConnections",
  "maxConcurrentRuns",
] as const;

type QuotaField = (typeof quotaFields)[number];
type QuotaDraft = Record<QuotaField, string>;

function emptyQuotaDraft(): QuotaDraft {
  return {
    maxWorkflows: "0",
    maxExecutionsPerDay: "0",
    maxSkills: "0",
    maxAgents: "0",
    maxPlugins: "0",
    maxConnections: "0",
    maxConcurrentRuns: "0",
  };
}

function quotaDraftFrom(quota: EnterpriseQuota): QuotaDraft {
  return {
    maxWorkflows: String(quota.maxWorkflows),
    maxExecutionsPerDay: String(quota.maxExecutionsPerDay),
    maxSkills: String(quota.maxSkills),
    maxAgents: String(quota.maxAgents),
    maxPlugins: String(quota.maxPlugins),
    maxConnections: String(quota.maxConnections),
    maxConcurrentRuns: String(quota.maxConcurrentRuns),
  };
}

function quotaBody(draft: QuotaDraft): EnterpriseQuota {
  return {
    maxWorkflows: Number(draft.maxWorkflows),
    maxExecutionsPerDay: Number(draft.maxExecutionsPerDay),
    maxSkills: Number(draft.maxSkills),
    maxAgents: Number(draft.maxAgents),
    maxPlugins: Number(draft.maxPlugins),
    maxConnections: Number(draft.maxConnections),
    maxConcurrentRuns: Number(draft.maxConcurrentRuns),
  };
}

function quotaDraftValid(draft: QuotaDraft): boolean {
  return quotaFields.every((field) => {
    const value = draft[field].trim();
    if (!value) return false;
    const parsed = Number(value);
    return Number.isFinite(parsed) && parsed >= 0 && Number.isInteger(parsed);
  });
}

function maskToken(value: string): string {
  if (!value) return "";
  if (value.length <= 8) return "•".repeat(value.length);
  return `${value.slice(0, 4)}••••••${value.slice(-4)}`;
}

function formatBytes(size: number): string {
  if (!Number.isFinite(size) || size < 0) return "—";
  if (size < 1024) return `${size} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = size / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[index] ?? "B"}`;
}

function errorText(error: unknown, fallback: string): string {
  if (error instanceof ApiError) {
    return error.message || error.code || fallback;
  }
  return error instanceof Error ? error.message : fallback;
}

function isAuthError(error: unknown): boolean {
  return error instanceof ApiError && (error.status === 401 || error.status === 403);
}

function parseMetadata(text: string): JsonValue | undefined {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  return JSON.parse(trimmed) as JsonValue;
}

function tenantTone(status: string): BadgeTone {
  if (status === "active") return "success";
  if (status === "suspended" || status === "disabled") return "warning";
  if (status === "archived") return "neutral";
  return "neutral";
}

function workerTone(status: string): BadgeTone {
  if (status === "online") return "success";
  if (status === "offline") return "warning";
  if (status === "disabled") return "danger";
  return "neutral";
}

function workerJobTone(status: string): BadgeTone {
  if (status === "queued") return "neutral";
  if (status === "leased" || status === "running") return "info";
  if (status === "waiting_approval") return "warning";
  if (status === "succeeded") return "success";
  if (status === "failed") return "danger";
  return "neutral";
}

function isTerminalWorkerJob(status: string): boolean {
  return status === "succeeded" || status === "failed" || status === "cancelled";
}

function epochTimestamp(value: number | null): string | undefined {
  if (value === null || !Number.isFinite(value)) return undefined;
  return new Date(value).toISOString();
}

function splitCapabilities(value: string): string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function backupTone(status: string): BadgeTone {
  if (status === "ready" || status === "completed" || status === "succeeded") return "success";
  if (status === "failed") return "danger";
  if (status === "creating" || status === "pending") return "warning";
  return "neutral";
}

function decisionTone(decision: string): BadgeTone {
  if (decision === "allow" || decision === "allowed" || decision === "permitted") return "success";
  if (decision === "deny" || decision === "denied" || decision === "rejected") return "danger";
  return "neutral";
}

function tokenState(token: EnterpriseToken): "revoked" | "expired" | "active" {
  if (token.revokedAt) return "revoked";
  if (token.expiresAt && Date.parse(token.expiresAt) <= Date.now()) return "expired";
  return "active";
}

export function EnterprisePage() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [tab, setTab] = useState<EnterpriseTab>("overview");
  const [token, setToken] = useState(readEnterpriseToken);
  const [localMode, setLocalMode] = useState(false);
  const [tokenDraft, setTokenDraft] = useState("");
  const [selectedTenantId, setSelectedTenantId] = useState<string | null>(null);
  const { session, logout } = useAuth();

  const sessionAuthenticated = Boolean(session?.authenticated);
  const sessionAdmin = Boolean(session?.principal?.platformAdmin);
  const canRequest = token.length > 0 || localMode || (sessionAuthenticated && sessionAdmin);
  const controlAllowed = token.length > 0 || localMode || (sessionAuthenticated && sessionAdmin);
  const modeLabel = token
    ? t("enterprise.modeToken")
    : localMode
      ? t("enterprise.modeLocal")
      : sessionAuthenticated
        ? t("enterprise.modeSession")
        : t("enterprise.modeLocked");
  const tenantsQuery = useQuery({
    queryKey: ["enterprise", "tenants"],
    queryFn: api.enterpriseTenants,
    enabled: controlAllowed,
    retry: false,
  });
  const tenants = tenantsQuery.data?.tenants ?? [];
  const authRejected = isAuthError(tenantsQuery.error);

  useEffect(() => {
    if (tenants.length === 0) {
      if (selectedTenantId) setSelectedTenantId(null);
      return;
    }
    if (!selectedTenantId || !tenants.some((tenant) => tenant.id === selectedTenantId)) {
      setSelectedTenantId(tenants[0]?.id ?? null);
    }
  }, [tenants, selectedTenantId]);

  const tabs: [EnterpriseTab, string, ReactNode][] = [
    ["overview", t("enterprise.tabs.overview"), <Gauge key="o" className="h-3 w-3" />],
    ["access", t("enterprise.tabs.access"), <KeyRound key="a" className="h-3 w-3" />],
    ["tenants", t("enterprise.tabs.tenants"), <Building2 key="t" className="h-3 w-3" />],
    ["tokens", t("enterprise.tabs.tokens"), <ShieldCheck key="k" className="h-3 w-3" />],
    ["members", t("enterprise.tabs.members"), <Users key="m" className="h-3 w-3" />],
    ["quotas", t("enterprise.tabs.quotas"), <Gauge key="q" className="h-3 w-3" />],
    ["workers", t("enterprise.tabs.workers"), <Server key="w" className="h-3 w-3" />],
    ["backups", t("enterprise.tabs.backups"), <DatabaseBackup key="b" className="h-3 w-3" />],
    ["audit", t("enterprise.tabs.audit"), <ScrollText key="u" className="h-3 w-3" />],
    ["oidc", t("enterprise.tabs.oidc"), <LockKeyhole key="i" className="h-3 w-3" />],
    ["oauth", t("enterprise.tabs.oauth"), <Webhook key="x" className="h-3 w-3" />],
  ];

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
  }

  function saveToken() {
    const value = tokenDraft.trim();
    if (!value) return;
    writeEnterpriseToken(value);
    setToken(value);
    setTokenDraft("");
    setLocalMode(false);
    pushToast(t("enterprise.auth.saved"), "success");
    refresh();
  }

  function clearToken() {
    clearEnterpriseToken();
    setToken("");
    setTokenDraft("");
    setLocalMode(false);
    queryClient.removeQueries({ queryKey: ["enterprise"] });
    pushToast(t("enterprise.auth.cleared"), "warning");
  }

  function enterLocalMode() {
    setLocalMode(true);
    pushToast(t("enterprise.auth.localBadge"), "info");
  }

  function exitLocalMode() {
    setLocalMode(false);
    queryClient.removeQueries({ queryKey: ["enterprise"] });
  }

  async function signOutSession() {
    await logout();
    queryClient.removeQueries({ queryKey: ["enterprise"] });
    pushToast(t("enterprise.auth.sessionLoggedOut"), "info");
  }

  const accessPanel = (
    <AccessPanel
      token={token}
      localMode={localMode}
      session={session}
      draft={tokenDraft}
      onDraft={setTokenDraft}
      onSave={saveToken}
      onClear={clearToken}
      onEnterLocal={enterLocalMode}
      onExitLocal={exitLocalMode}
      onLogout={() => void signOutSession()}
      rejected={authRejected}
    />
  );

  return (
    <div className="mx-auto grid min-w-0 max-w-[1560px] gap-3">
      <PageHeader
        title={t("enterprise.title")}
        subtitle={t("enterprise.subtitle")}
        actions={
          <>
            <Badge
              tone={token ? "success" : localMode ? "warning" : sessionAuthenticated ? "accent" : "neutral"}
              dot
            >
              {modeLabel}
            </Badge>
            <Button
              variant="secondary"
              disabled={!controlAllowed}
              onClick={refresh}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("common.refresh")}
            </Button>
          </>
        }
      />

      <div className="flex flex-wrap gap-1">
        {tabs.map(([key, label, icon]) => (
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
        <span className="ml-auto self-center text-[10px] text-faint">
          {t("enterprise.localPlane")}
        </span>
      </div>

      {sessionAuthenticated && !sessionAdmin && !token && !localMode ? (
        <div className="grid min-w-0 gap-3 xl:grid-cols-[minmax(0,1fr)_340px]">
          {accessPanel}
          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <LockKeyhole className="h-4 w-4 text-warning" />
              <h2 className="text-sm font-semibold text-ink">
                {t("enterprise.auth.sessionNoAdminTitle")}
              </h2>
            </div>
            <p className="mt-2 text-[11.5px] text-muted">
              {t("enterprise.auth.sessionNoAdminText")}
            </p>
            <p className="mt-2 text-[11px] text-faint">
              {t("enterprise.auth.sessionNoAdminHint")}
            </p>
            <Link
              to="/login"
              className="mt-2 inline-flex items-center gap-1.5 text-[11px] text-accent hover:underline"
            >
              <LogIn className="h-3.5 w-3.5" />
              {t("enterprise.auth.sessionSignIn")}
            </Link>
          </Panel>
        </div>
      ) : !canRequest ? (
        <div className="grid min-w-0 gap-3 xl:grid-cols-[minmax(0,1fr)_340px]">
          {accessPanel}
          <Panel className="p-4">
            <div className="flex items-center gap-2">
              <Building2 className="h-4 w-4 text-accent" />
              <h2 className="text-sm font-semibold text-ink">{t("enterprise.auth.gateTitle")}</h2>
            </div>
            <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.auth.gateText")}</p>
            <ul className="mt-2 grid gap-1 text-[11px] text-faint">
              <li>{t("enterprise.auth.gatePointApi")}</li>
              <li>{t("enterprise.auth.gatePointLocal")}</li>
              <li>{t("enterprise.auth.gatePointScope")}</li>
            </ul>
          </Panel>
        </div>
      ) : (
        <>
          {authRejected ? (
            <Panel className="border-danger/30 bg-danger/5 p-3">
              <div className="flex flex-wrap items-center gap-2">
                <LockKeyhole className="h-4 w-4 text-danger" />
                <div className="min-w-0">
                  <p className="text-[12px] font-semibold text-danger">
                    {token
                      ? t("enterprise.auth.rejected")
                      : t("enterprise.auth.localDenied")}
                  </p>
                  <p className="mt-0.5 text-[11px] text-muted">
                    {t("enterprise.auth.rejectedHint")}
                  </p>
                </div>
                <span className="font-mono text-[10px] text-faint">{maskToken(token)}</span>
                <Button
                  size="sm"
                  variant="secondary"
                  className="ml-auto"
                  onClick={() => setTab("access")}
                >
                  <KeyRound className="h-3.5 w-3.5" />
                  {t("enterprise.auth.openAccess")}
                </Button>
              </div>
            </Panel>
          ) : null}

          {tab === "overview" ? (
            <OverviewSection
              enabled={controlAllowed}
              tenantCount={tenants.length}
              tenantsLoading={tenantsQuery.isLoading}
              modeLabel={modeLabel}
            />
          ) : null}
          {tab === "access" ? accessPanel : null}
          {tab === "tenants" ? (
            <TenantsSection
              enabled={controlAllowed}
              tenants={tenants}
              loading={tenantsQuery.isLoading}
              error={tenantsQuery.error}
              selectedId={selectedTenantId}
              onSelect={setSelectedTenantId}
            />
          ) : null}
          {tab === "tokens" ? (
            <TokensSection
              enabled={controlAllowed}
              tenants={tenants}
              selectedTenantId={selectedTenantId}
            />
          ) : null}
          {tab === "members" ? (
            <MembersSection
              enabled={controlAllowed}
              tenants={tenants}
              selectedTenantId={selectedTenantId}
            />
          ) : null}
          {tab === "quotas" ? (
            <QuotasSection
              enabled={controlAllowed}
              tenants={tenants}
              tenantId={selectedTenantId}
              onTenant={setSelectedTenantId}
            />
          ) : null}
          {tab === "workers" ? (
            <WorkersSection
              enabled={controlAllowed}
              tenants={tenants}
              tenantId={selectedTenantId}
              onTenant={setSelectedTenantId}
            />
          ) : null}
          {tab === "backups" ? (
            <BackupsSection
              enabled={controlAllowed}
              tenants={tenants}
              tenantId={selectedTenantId}
              onTenant={setSelectedTenantId}
            />
          ) : null}
          {tab === "audit" ? <AuditSection enabled={controlAllowed} tenants={tenants} /> : null}
          {tab === "oidc" ? <OidcSection enabled={controlAllowed} /> : null}
          {tab === "oauth" ? <OAuthSection enabled={controlAllowed} /> : null}
        </>
      )}
    </div>
  );
}

function AccessPanel({
  token,
  localMode,
  session,
  draft,
  onDraft,
  onSave,
  onClear,
  onEnterLocal,
  onExitLocal,
  onLogout,
  rejected,
}: {
  token: string;
  localMode: boolean;
  session: AuthSession | null;
  draft: string;
  onDraft: (value: string) => void;
  onSave: () => void;
  onClear: () => void;
  onEnterLocal: () => void;
  onExitLocal: () => void;
  onLogout: () => void;
  rejected: boolean;
}) {
  const { t } = useTranslation();
  const principal = session?.authenticated ? session.principal : undefined;
  if (!token && principal) {
    return (
      <Panel className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <ShieldCheck className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">
            {t("enterprise.auth.sessionTitle")}
          </h2>
          <Badge tone="accent" dot className="ml-auto">
            {t("enterprise.auth.sessionBadge")}
          </Badge>
        </div>
        <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.auth.sessionHint")}</p>
        <dl className="mt-3 grid gap-1.5 rounded-sm border border-line bg-panel-2 px-2.5 py-2 text-[11px]">
          <div className="flex min-w-0 items-center gap-2">
            <dt className="w-[92px] shrink-0 text-faint">{t("enterprise.auth.sessionIdentity")}</dt>
            <dd className="min-w-0 truncate text-ink">
              {principal.displayName || principal.email || principal.subject}
            </dd>
          </div>
          {principal.email ? (
            <div className="flex min-w-0 items-center gap-2">
              <dt className="w-[92px] shrink-0 text-faint">{t("enterprise.auth.sessionEmail")}</dt>
              <dd className="min-w-0 truncate font-mono text-[10.5px] text-muted">
                {principal.email}
              </dd>
            </div>
          ) : null}
          <div className="flex min-w-0 items-center gap-2">
            <dt className="w-[92px] shrink-0 text-faint">{t("enterprise.auth.sessionTenant")}</dt>
            <dd className="min-w-0 truncate text-ink">{principal.tenantId}</dd>
          </div>
          <div className="flex min-w-0 items-center gap-2">
            <dt className="w-[92px] shrink-0 text-faint">{t("enterprise.auth.sessionRole")}</dt>
            <dd className="min-w-0 truncate text-ink">
              {t(`enterprise.roles.${principal.role}`, { defaultValue: principal.role })}
            </dd>
          </div>
          <div className="flex min-w-0 items-center gap-2">
            <dt className="w-[92px] shrink-0 text-faint">{t("enterprise.auth.sessionAdmin")}</dt>
            <dd className="min-w-0 truncate">
              <Badge tone={principal.platformAdmin ? "success" : "neutral"} dot>
                {principal.platformAdmin
                  ? t("enterprise.auth.sessionAdminYes")
                  : t("enterprise.auth.sessionAdminNo")}
              </Badge>
            </dd>
          </div>
          <div className="flex min-w-0 items-center gap-2">
            <dt className="w-[92px] shrink-0 text-faint">{t("enterprise.auth.sessionExpires")}</dt>
            <dd className="min-w-0 truncate text-muted">
              {session?.expiresAt ? (
                <Timestamp value={new Date(session.expiresAt).toISOString()} />
              ) : (
                "—"
              )}
            </dd>
          </div>
        </dl>
        <div className="mt-3 flex flex-wrap items-center gap-1.5">
          <Button variant="secondary" size="sm" onClick={onLogout}>
            <LogOut className="h-3.5 w-3.5" />
            {t("enterprise.auth.sessionLogout")}
          </Button>
          <Link
            to="/login"
            className="inline-flex h-6 items-center gap-1.5 rounded-sm border border-line px-2 text-[11.5px] text-muted hover:text-ink"
          >
            <LogIn className="h-3.5 w-3.5" />
            {t("enterprise.auth.sessionSwitch")}
          </Link>
        </div>
        <p className="mt-3 rounded-sm border border-line bg-panel-2 px-2 py-1.5 text-[10.5px] text-faint">
          {t("enterprise.auth.sessionCsrfNote")}
        </p>
      </Panel>
    );
  }
  const active = token.length > 0 || localMode;
  return (
    <Panel className="p-4">
      <div className="flex flex-wrap items-center gap-2">
        <KeyRound className="h-4 w-4 text-accent" />
        <h2 className="text-sm font-semibold text-ink">{t("enterprise.auth.title")}</h2>
        <Badge
          tone={token ? "success" : localMode ? "warning" : "neutral"}
          dot
          className="ml-auto"
        >
          {token
            ? t("enterprise.auth.tokenSet")
            : localMode
              ? t("enterprise.auth.localBadge")
              : t("enterprise.auth.noToken")}
        </Badge>
      </div>
      <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.auth.notPassword")}</p>
      <p className="mt-1 text-[11px] text-faint">{t("enterprise.auth.storedLocally")}</p>

      {token ? (
        <div className="mt-3 flex flex-wrap items-center gap-2 rounded-sm border border-line bg-panel-2 px-2.5 py-1.5">
          <ShieldCheck className="h-3.5 w-3.5 text-success" />
          <span className="text-[10px] uppercase tracking-[0.12em] text-faint">
            {t("enterprise.auth.activeToken")}
          </span>
          <span className="font-mono text-[11px] text-ink">{maskToken(token)}</span>
        </div>
      ) : null}

      {rejected ? (
        <p className="mt-2 rounded-sm border border-danger/30 bg-danger/5 px-2 py-1.5 text-[10.5px] text-danger">
          {t("enterprise.auth.rejectedHint")}
        </p>
      ) : null}

      <div className="mt-3 grid gap-2">
        <Field label={t("enterprise.auth.tokenLabel")} hint={t("enterprise.auth.tokenHint")}>
          <input
            className={cn(inputClass, "font-mono text-[11px]")}
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={draft}
            onChange={(event) => onDraft(event.target.value)}
            placeholder={t("enterprise.auth.placeholder")}
          />
        </Field>
        <div className="flex flex-wrap items-center gap-1.5">
          <Button variant="primary" size="sm" disabled={!draft.trim()} onClick={onSave}>
            <KeyRound className="h-3.5 w-3.5" />
            {t("enterprise.auth.save")}
          </Button>
          <Button
            variant="danger"
            size="sm"
            disabled={!token}
            onClick={onClear}
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t("enterprise.auth.clear")}
          </Button>
          {!token && !localMode ? (
            <Button variant="secondary" size="sm" onClick={onEnterLocal}>
              <Server className="h-3.5 w-3.5" />
              {t("enterprise.auth.useLocal")}
            </Button>
          ) : null}
          {!token && localMode ? (
            <Button variant="secondary" size="sm" onClick={onExitLocal}>
              {t("enterprise.auth.exitLocal")}
            </Button>
          ) : null}
        </div>
      </div>

      <p className="mt-3 rounded-sm border border-line bg-panel-2 px-2 py-1.5 text-[10.5px] text-faint">
        {t("enterprise.auth.apiNote")}
      </p>
      {!active ? (
        <p className="mt-2 text-[10.5px] text-muted">{t("enterprise.auth.localHint")}</p>
      ) : null}
    </Panel>
  );
}

function OverviewSection({
  enabled,
  tenantCount,
  tenantsLoading,
  modeLabel,
}: {
  enabled: boolean;
  tenantCount: number;
  tenantsLoading: boolean;
  modeLabel: string;
}) {
  const { t } = useTranslation();
  const tokensQuery = useQuery({
    queryKey: ["enterprise", "tokens", "all"],
    queryFn: () => api.enterpriseTokens(),
    enabled,
    retry: false,
  });
  const workersQuery = useQuery({
    queryKey: ["enterprise", "workers", "all"],
    queryFn: () => api.enterpriseWorkers(),
    enabled,
    retry: false,
  });
  const backupsQuery = useQuery({
    queryKey: ["enterprise", "backups", "all"],
    queryFn: () => api.enterpriseBackups(),
    enabled,
    retry: false,
  });
  const auditQuery = useQuery({
    queryKey: ["enterprise", "audit", "all", 200],
    queryFn: () => api.enterpriseAudit(undefined, 200),
    enabled,
    retry: false,
  });

  const now = Date.now();
  const activeTokens = (tokensQuery.data?.tokens ?? []).filter(
    (entry) => !entry.revokedAt && (!entry.expiresAt || Date.parse(entry.expiresAt) > now),
  ).length;
  const workers = workersQuery.data?.workers ?? [];
  const online = workers.filter((worker) => worker.status === "online").length;
  const offline = workers.filter((worker) => worker.status === "offline").length;
  const backups = backupsQuery.data?.backups.length ?? 0;
  const auditEvents = auditQuery.data?.entries.length ?? 0;

  return (
    <>
      <div className="grid grid-cols-2 gap-2 min-[560px]:grid-cols-3 xl:grid-cols-6">
        <StatCard
          label={t("enterprise.overview.statTenants")}
          value={tenantsLoading ? "—" : tenantCount}
        />
        <StatCard
          label={t("enterprise.overview.statTokens")}
          value={tokensQuery.isLoading ? "—" : activeTokens}
        />
        <StatCard
          label={t("enterprise.overview.statWorkersOnline")}
          value={workersQuery.isLoading ? "—" : online}
        />
        <StatCard
          label={t("enterprise.overview.statWorkersOffline")}
          value={workersQuery.isLoading ? "—" : offline}
        />
        <StatCard
          label={t("enterprise.overview.statBackups")}
          value={backupsQuery.isLoading ? "—" : backups}
        />
        <StatCard
          label={t("enterprise.overview.statAudit")}
          value={auditQuery.isLoading ? "—" : auditEvents}
        />
      </div>

      <div className="grid min-w-0 gap-3 lg:grid-cols-2">
        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.overview.planeTitle")}</h2>
            <Badge tone="accent" className="ml-auto">
              {modeLabel}
            </Badge>
          </div>
          <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.overview.planeText")}</p>
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <span className="rounded-sm border border-line bg-panel-2 px-2 py-1 font-mono text-[10px] text-accent">
              /api/enterprise
            </span>
            <span className="text-[10px] text-faint">{t("enterprise.overview.planeHint")}</span>
          </div>
        </Panel>

        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <Building2 className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">
              {t("enterprise.overview.scopeTitle")}
            </h2>
          </div>
          <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.overview.scopeText")}</p>
          <div className="mt-2 grid grid-cols-2 gap-2">
            <MiniStat label={t("enterprise.overview.statTenants")} value={String(tenantCount)} />
            <MiniStat
              label={t("enterprise.overview.statTokens")}
              value={String(activeTokens)}
            />
          </div>
        </Panel>
      </div>

      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Activity className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.overview.recentAudit")}</h2>
          <span className="ml-auto font-mono text-[10px] text-faint">{auditEvents}</span>
        </div>
        {auditQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : auditQuery.isError ? (
          <p className="mt-2 text-xs text-danger">
            {errorText(auditQuery.error, t("errors.loadFailed"))}
          </p>
        ) : (auditQuery.data?.entries ?? []).length === 0 ? (
          <p className="mt-2 text-xs text-faint">{t("enterprise.audit.empty")}</p>
        ) : (
          <div className="mt-2 grid gap-1">
            {(auditQuery.data?.entries ?? []).slice(0, 5).map((entry) => (
              <div
                key={entry.id}
                className="flex flex-wrap items-center gap-2 rounded-sm border border-line px-2 py-1.5"
              >
                <Badge tone={decisionTone(entry.decision)}>
                  {t(`enterprise.decision.${entry.decision}`, { defaultValue: entry.decision })}
                </Badge>
                <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink">
                  {entry.action}
                </span>
                <span className="text-[10px] text-faint">{entry.tenantId}</span>
                <Timestamp value={entry.at} />
              </div>
            ))}
          </div>
        )}
      </Panel>
    </>
  );
}

function TenantsSection({
  enabled,
  tenants,
  loading,
  error,
  selectedId,
  onSelect,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
  loading: boolean;
  error: unknown;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [name, setName] = useState("");
  const [confirmId, setConfirmId] = useState<string | null>(null);
  const [confirmText, setConfirmText] = useState("");

  const detail = useQuery({
    queryKey: ["enterprise", "tenant", selectedId],
    queryFn: () => api.enterpriseTenant(selectedId ?? ""),
    enabled: enabled && Boolean(selectedId),
    retry: false,
  });

  const createTenant = useMutation({
    mutationFn: () => api.enterpriseCreateTenant(name.trim()),
    onSuccess: (tenant) => {
      setName("");
      onSelect(tenant.id);
      pushToast(t("enterprise.tenants.created"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.tenants.createFailed")), "error"),
  });

  const deleteTenant = useMutation({
    mutationFn: (id: string) => api.enterpriseDeleteTenant(id),
    onSuccess: () => {
      setConfirmId(null);
      setConfirmText("");
      onSelect("");
      pushToast(t("enterprise.tenants.deleted"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.tenants.deleteFailed")), "error"),
  });

  return (
    <>
      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Building2 className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.tenants.createTitle")}</h2>
        </div>
        <div className="mt-2 grid gap-2 min-[560px]:grid-cols-[minmax(0,320px)_auto] min-[560px]:items-end">
          <Field label={t("enterprise.tenants.name")}>
            <input
              className={inputClass}
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder={t("enterprise.tenants.namePlaceholder")}
            />
          </Field>
          <Button
            variant="primary"
            size="sm"
            className="w-fit"
            loading={createTenant.isPending}
            disabled={!name.trim()}
            onClick={() => createTenant.mutate()}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("enterprise.tenants.create")}
          </Button>
        </div>
      </Panel>

      <Panel className="p-4">
        <div className="flex items-center gap-2">
          <Users className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.tenants.listTitle")}</h2>
          <span className="ml-auto font-mono text-[10px] text-faint">{tenants.length}</span>
        </div>
        {loading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : error ? (
          <p className="mt-2 text-xs text-danger">{errorText(error, t("errors.loadFailed"))}</p>
        ) : tenants.length === 0 ? (
          <EmptyState
            icon={<Building2 className="h-4 w-4" />}
            title={t("enterprise.tenants.empty")}
            hint={t("enterprise.tenants.emptyHint")}
          />
        ) : (
          <div className="mt-2 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[640px] border-collapse text-left text-[11px]">
              <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                <tr>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.tenants.name")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.createdAt")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.updatedAt")}</th>
                  <th className="px-3 py-1.5 font-medium" />
                </tr>
              </thead>
              <tbody>
                {tenants.map((tenant) => (
                  <tr
                    key={tenant.id}
                    className={cn(
                      "border-t border-line",
                      selectedId === tenant.id && "bg-accent/5",
                    )}
                  >
                    <td className="px-3 py-1.5">
                      <div className="text-[11.5px] font-medium text-ink">{tenant.name}</div>
                      <div className="font-mono text-[9.5px] text-faint">{tenant.id}</div>
                    </td>
                    <td className="px-3 py-1.5">
                      <Badge tone={tenantTone(tenant.status)} dot>
                        {t(`enterprise.status.${tenant.status}`, {
                          defaultValue: tenant.status,
                        })}
                      </Badge>
                    </td>
                    <td className="px-3 py-1.5">
                      <Timestamp value={tenant.createdAt} />
                    </td>
                    <td className="px-3 py-1.5">
                      <Timestamp value={tenant.updatedAt} />
                    </td>
                    <td className="px-3 py-1.5 text-right">
                      <Button
                        size="xs"
                        variant={selectedId === tenant.id ? "primary" : "ghost"}
                        onClick={() => {
                          onSelect(tenant.id);
                          setConfirmId(null);
                          setConfirmText("");
                        }}
                      >
                        {selectedId === tenant.id
                          ? t("enterprise.selected")
                          : t("enterprise.select")}
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>

      {selectedId ? (
        <Panel className="p-4">
          {detail.isLoading ? (
            <p className="text-xs text-muted">{t("common.loading")}</p>
          ) : detail.isError ? (
            <p className="text-xs text-danger">
              {errorText(detail.error, t("errors.loadFailed"))}
            </p>
          ) : !detail.data ? (
            <EmptyState
              icon={<Building2 className="h-4 w-4" />}
              title={t("enterprise.tenants.selectHint")}
            />
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-2">
                <Building2 className="h-4 w-4 text-accent" />
                <h2 className="text-sm font-semibold text-ink">{detail.data.tenant.name}</h2>
                <Badge tone={tenantTone(detail.data.tenant.status)} dot>
                  {t(`enterprise.status.${detail.data.tenant.status}`, {
                    defaultValue: detail.data.tenant.status,
                  })}
                </Badge>
                <span className="font-mono text-[9.5px] text-faint">{detail.data.tenant.id}</span>
              </div>
              <dl className="mt-2 grid grid-cols-2 gap-2 min-[560px]:grid-cols-4">
                <MiniStat label={t("common.createdAt")} value="" stamp={detail.data.tenant.createdAt} />
                <MiniStat label={t("common.updatedAt")} value="" stamp={detail.data.tenant.updatedAt} />
                <MiniStat
                  label={t("enterprise.tenants.workersCount")}
                  value={String(detail.data.workers.length)}
                />
                <MiniStat
                  label={t("enterprise.tenants.backupsCount")}
                  value={String(detail.data.backups.length)}
                />
              </dl>

              <h3 className="mt-3 text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
                {t("enterprise.tenants.quotasTitle")}
              </h3>
              <div className="mt-1 grid grid-cols-2 gap-2 min-[560px]:grid-cols-4">
                {quotaFields.map((field) => (
                  <MiniStat
                    key={field}
                    label={t(`enterprise.quotas.${field}`)}
                    value={String(detail.data.quotas[field])}
                  />
                ))}
              </div>

              <div className="mt-3 border-t border-line pt-3">
                {confirmId === detail.data.tenant.id ? (
                  <div className="rounded-sm border border-danger/30 bg-danger/5 p-2.5">
                    <p className="text-[11px] font-semibold text-danger">
                      {t("enterprise.tenants.deleteTitle")}
                    </p>
                    <p className="mt-1 text-[10.5px] text-muted">
                      {t("enterprise.tenants.deleteHint")}
                    </p>
                    <Field
                      className="mt-2"
                      label={t("enterprise.tenants.confirmLabel", {
                        name: detail.data.tenant.name,
                      })}
                    >
                      <input
                        className={inputClass}
                        value={confirmText}
                        onChange={(event) => setConfirmText(event.target.value)}
                        placeholder={detail.data.tenant.name}
                      />
                    </Field>
                    <div className="mt-2 flex flex-wrap gap-1.5">
                      <Button
                        size="sm"
                        variant="danger"
                        loading={deleteTenant.isPending}
                        disabled={confirmText !== detail.data.tenant.name}
                        onClick={() => deleteTenant.mutate(detail.data.tenant.id)}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        {t("enterprise.tenants.delete")}
                      </Button>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => {
                          setConfirmId(null);
                          setConfirmText("");
                        }}
                      >
                        {t("common.cancel")}
                      </Button>
                    </div>
                  </div>
                ) : (
                  <div className="flex flex-wrap items-center gap-2">
                    <Button
                      size="sm"
                      variant="danger"
                      onClick={() => {
                        setConfirmId(detail.data?.tenant.id ?? null);
                        setConfirmText("");
                      }}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      {t("enterprise.tenants.delete")}
                    </Button>
                    <span className="text-[10px] text-faint">
                      {t("enterprise.tenants.deleteShortHint")}
                    </span>
                  </div>
                )}
              </div>
            </>
          )}
        </Panel>
      ) : null}
    </>
  );
}

function TokensSection({
  enabled,
  tenants,
  selectedTenantId,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
  selectedTenantId: string | null;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [scope, setScope] = useState<string>("all");
  const [name, setName] = useState("");
  const [tenantId, setTenantId] = useState("");
  const [role, setRole] = useState<EnterpriseRole>("viewer");
  const [platformAdmin, setPlatformAdmin] = useState(false);
  const [expires, setExpires] = useState("");
  const [issued, setIssued] = useState<{ token: string; info: EnterpriseToken } | null>(null);
  const [revokeId, setRevokeId] = useState<string | null>(null);

  useEffect(() => {
    if (tenants.length === 0) {
      setTenantId("");
      return;
    }
    if (!tenants.some((tenant) => tenant.id === tenantId)) {
      setTenantId(selectedTenantId ?? tenants[0]?.id ?? "");
    }
  }, [tenants, tenantId, selectedTenantId]);

  useEffect(() => {
    if (scope !== "all" && !tenants.some((tenant) => tenant.id === scope)) {
      setScope("all");
    }
  }, [tenants, scope]);

  const tenantParam = scope === "all" ? undefined : scope;
  const tokensQuery = useQuery({
    queryKey: ["enterprise", "tokens", tenantParam ?? "all"],
    queryFn: () => api.enterpriseTokens(tenantParam),
    enabled,
    retry: false,
  });

  const expiresInvalid =
    expires.trim() !== "" && (!Number.isInteger(Number(expires)) || Number(expires) < 1);

  const createToken = useMutation({
    mutationFn: () =>
      api.enterpriseCreateToken({
        tenantId: tenantId,
        name: name.trim(),
        role,
        platformAdmin: platformAdmin || undefined,
        expiresInDays: expires.trim() ? Number(expires) : undefined,
      }),
    onSuccess: (result) => {
      setIssued({ token: result.token, info: result.tokenInfo });
      setName("");
      setExpires("");
      setPlatformAdmin(false);
      setScope((current) => (current === "all" ? current : result.tokenInfo.tenantId));
      pushToast(t("enterprise.tokens.created"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.tokens.createFailed")), "error"),
  });

  const revokeToken = useMutation({
    mutationFn: (id: string) => api.enterpriseRevokeToken(id),
    onSuccess: () => {
      setRevokeId(null);
      pushToast(t("enterprise.tokens.revokedToast"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.tokens.revokeFailed")), "error"),
  });

  const entries = tokensQuery.data?.tokens ?? [];

  return (
    <div className="grid min-w-0 gap-3 xl:grid-cols-[360px_minmax(0,1fr)]">
      <div className="grid min-w-0 content-start gap-3">
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <KeyRound className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.tokens.createTitle")}</h2>
          </div>
          <div className="mt-2 grid gap-2">
            <Field label={t("enterprise.tokens.name")}>
              <input
                className={inputClass}
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder={t("enterprise.tokens.namePlaceholder")}
              />
            </Field>
            <Field label={t("enterprise.tokens.tenant")}>
              <select
                className={inputClass}
                value={tenantId}
                onChange={(event) => setTenantId(event.target.value)}
              >
                {tenants.map((tenant) => (
                  <option key={tenant.id} value={tenant.id}>
                    {tenant.name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label={t("enterprise.tokens.role")}>
              <select
                className={inputClass}
                value={role}
                onChange={(event) => setRole(event.target.value as EnterpriseRole)}
              >
                {roles.map((entry) => (
                  <option key={entry} value={entry}>
                    {t(`enterprise.roles.${entry}`)}
                  </option>
                ))}
              </select>
            </Field>
            <Field
              label={t("enterprise.tokens.expiresInDays")}
              hint={t("enterprise.tokens.expiresHint")}
            >
              <input
                className={inputClass}
                type="number"
                min={1}
                step={1}
                value={expires}
                onChange={(event) => setExpires(event.target.value)}
                placeholder="30"
              />
            </Field>
            <label className="inline-flex items-center gap-1.5 text-[11px] text-muted">
              <input
                type="checkbox"
                className="h-3.5 w-3.5 accent-accent"
                checked={platformAdmin}
                onChange={(event) => setPlatformAdmin(event.target.checked)}
              />
              {t("enterprise.tokens.platformAdmin")}
            </label>
            <Button
              variant="primary"
              size="sm"
              loading={createToken.isPending}
              disabled={!name.trim() || !tenantId || expiresInvalid}
              onClick={() => createToken.mutate()}
            >
              <Plus className="h-3.5 w-3.5" />
              {t("enterprise.tokens.create")}
            </Button>
            {expiresInvalid ? (
              <p className="text-[10px] text-danger">{t("enterprise.tokens.expiresInvalid")}</p>
            ) : null}
          </div>
        </Panel>

        {issued ? (
          <Panel className="border-success/30 p-4">
            <div className="flex flex-wrap items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-success" />
              <h2 className="text-sm font-semibold text-ink">
                {t("enterprise.tokens.onceTitle")}
              </h2>
              <Button
                size="xs"
                variant="ghost"
                className="ml-auto"
                onClick={() => setIssued(null)}
              >
                {t("common.close")}
              </Button>
            </div>
            <p className="mt-1 text-[10.5px] text-warning">{t("enterprise.tokens.onceWarning")}</p>
            <div className="mt-2 flex flex-wrap items-start gap-1.5 rounded-sm border border-line bg-panel-2 p-2">
              <code className="min-w-0 flex-1 break-all font-mono text-[10.5px] text-ink">
                {issued.token}
              </code>
              <CopyButton text={issued.token} />
            </div>
            <p className="mt-1 text-[10px] text-faint">
              {issued.info.name} · {t(`enterprise.roles.${issued.info.role}`)} ·{" "}
              {issued.info.tenantId}
            </p>
          </Panel>
        ) : null}
      </div>

      <Panel className="min-w-0 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <ShieldCheck className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.tokens.listTitle")}</h2>
          <span className="ml-auto font-mono text-[10px] text-faint">{entries.length}</span>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-[0.12em] text-faint">
            {t("enterprise.tokens.filter")}
          </span>
          <select
            className={cn(inputClass, "h-7 w-auto min-w-[160px]")}
            value={scope}
            onChange={(event) => setScope(event.target.value)}
          >
            <option value="all">{t("enterprise.allTenants")}</option>
            {tenants.map((tenant) => (
              <option key={tenant.id} value={tenant.id}>
                {tenant.name}
              </option>
            ))}
          </select>
        </div>
        {tokensQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : tokensQuery.isError ? (
          <p className="mt-2 text-xs text-danger">
            {errorText(tokensQuery.error, t("errors.loadFailed"))}
          </p>
        ) : entries.length === 0 ? (
          <EmptyState
            icon={<ShieldCheck className="h-4 w-4" />}
            title={t("enterprise.tokens.empty")}
            hint={t("enterprise.tokens.emptyHint")}
          />
        ) : (
          <div className="mt-2 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[880px] border-collapse text-left text-[11px]">
              <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                <tr>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.tokens.name")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.tokens.tenant")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.tokens.role")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.createdAt")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.tokens.lastUsed")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.tokens.expires")}</th>
                  <th className="px-3 py-1.5 font-medium" />
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => {
                  const state = tokenState(entry);
                  return (
                    <tr key={entry.id} className="border-t border-line">
                      <td className="px-3 py-1.5">
                        <div className="text-[11.5px] text-ink">{entry.name}</div>
                        <div className="flex items-center gap-1 font-mono text-[9px] text-faint">
                          <span>{entry.id}</span>
                          {entry.platformAdmin ? (
                            <Badge tone="accent">{t("enterprise.tokens.platformAdminShort")}</Badge>
                          ) : null}
                        </div>
                      </td>
                      <td className="px-3 py-1.5 font-mono text-[9.5px] text-muted">
                        {entry.tenantId}
                      </td>
                      <td className="px-3 py-1.5">
                        <Badge tone="neutral">{t(`enterprise.roles.${entry.role}`)}</Badge>
                      </td>
                      <td className="px-3 py-1.5">
                        <Badge
                          tone={
                            state === "active"
                              ? "success"
                              : state === "expired"
                                ? "warning"
                                : "danger"
                          }
                          dot
                        >
                          {t(`enterprise.status.${state}`)}
                        </Badge>
                      </td>
                      <td className="px-3 py-1.5">
                        <Timestamp value={entry.createdAt} />
                      </td>
                      <td className="px-3 py-1.5">
                        {entry.lastUsedAt ? (
                          <Timestamp value={entry.lastUsedAt} />
                        ) : (
                          <span className="text-[10px] text-faint">
                            {t("enterprise.tokens.never")}
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-1.5">
                        {entry.expiresAt ? (
                          <Timestamp value={entry.expiresAt} />
                        ) : (
                          <span className="text-[10px] text-faint">
                            {t("enterprise.tokens.never")}
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-1.5 text-right">
                        {entry.revokedAt ? null : revokeId === entry.id ? (
                          <div className="flex justify-end gap-1">
                            <Button
                              size="xs"
                              variant="danger"
                              loading={revokeToken.isPending}
                              onClick={() => revokeToken.mutate(entry.id)}
                            >
                              {t("enterprise.tokens.confirmRevoke")}
                            </Button>
                            <Button
                              size="xs"
                              variant="ghost"
                              onClick={() => setRevokeId(null)}
                            >
                              {t("common.cancel")}
                            </Button>
                          </div>
                        ) : (
                          <Button
                            size="xs"
                            variant="danger"
                            onClick={() => setRevokeId(entry.id)}
                          >
                            {t("enterprise.tokens.revoke")}
                          </Button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </div>
  );
}

function MembersSection({
  enabled,
  tenants,
  selectedTenantId,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
  selectedTenantId: string | null;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [scope, setScope] = useState<string>("all");
  const [tenantId, setTenantId] = useState("");
  const [subject, setSubject] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<EnterpriseRole>("developer");
  const [deleteId, setDeleteId] = useState<string | null>(null);

  useEffect(() => {
    if (tenants.length === 0) {
      setTenantId("");
      return;
    }
    if (!tenants.some((tenant) => tenant.id === tenantId)) {
      setTenantId(selectedTenantId ?? tenants[0]?.id ?? "");
    }
  }, [tenants, tenantId, selectedTenantId]);

  useEffect(() => {
    if (scope !== "all" && !tenants.some((tenant) => tenant.id === scope)) {
      setScope("all");
    }
  }, [tenants, scope]);

  const tenantParam = scope === "all" ? undefined : scope;
  const membersQuery = useQuery({
    queryKey: ["enterprise", "members", tenantParam ?? "all"],
    queryFn: () => api.enterpriseMembers(tenantParam),
    enabled,
    retry: false,
  });

  const emailInvalid = email.trim() !== "" && !email.includes("@");

  const createMember = useMutation({
    mutationFn: () =>
      api.enterpriseCreateMember({
        tenantId,
        subject: subject.trim(),
        displayName: displayName.trim() || undefined,
        email: email.trim() || undefined,
        role,
      }),
    onSuccess: (member: EnterpriseMember) => {
      setSubject("");
      setDisplayName("");
      setEmail("");
      setScope((current) => (current === "all" ? current : member.tenantId));
      pushToast(t("enterprise.members.created"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.members.createFailed")), "error"),
  });

  const updateMember = useMutation({
    mutationFn: (input: {
      id: string;
      role?: EnterpriseRole;
      status?: "active" | "disabled";
    }) =>
      api.enterpriseUpdateMember(input.id, {
        role: input.role,
        status: input.status,
      }),
    onSuccess: () => {
      pushToast(t("enterprise.members.updated"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.members.updateFailed")), "error"),
  });

  const deleteMember = useMutation({
    mutationFn: (id: string) => api.enterpriseDeleteMember(id),
    onSuccess: () => {
      setDeleteId(null);
      pushToast(t("enterprise.members.deleted"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.members.deleteFailed")), "error"),
  });

  const entries = membersQuery.data?.members ?? [];

  return (
    <div className="grid min-w-0 gap-3 xl:grid-cols-[360px_minmax(0,1fr)]">
      <div className="grid min-w-0 content-start gap-3">
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <Users className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">
              {t("enterprise.members.createTitle")}
            </h2>
          </div>
          <div className="mt-2 grid gap-2">
            <Field label={t("enterprise.members.tenant")}>
              <select
                className={inputClass}
                value={tenantId}
                onChange={(event) => setTenantId(event.target.value)}
              >
                {tenants.map((tenant) => (
                  <option key={tenant.id} value={tenant.id}>
                    {tenant.name}
                  </option>
                ))}
              </select>
            </Field>
            <Field
              label={t("enterprise.members.subject")}
              hint={t("enterprise.members.subjectHint")}
            >
              <input
                className={inputClass}
                value={subject}
                onChange={(event) => setSubject(event.target.value)}
                placeholder={t("enterprise.members.subjectPlaceholder")}
              />
            </Field>
            <Field label={t("enterprise.members.displayName")}>
              <input
                className={inputClass}
                value={displayName}
                onChange={(event) => setDisplayName(event.target.value)}
                placeholder={t("enterprise.members.displayNamePlaceholder")}
              />
            </Field>
            <Field label={t("enterprise.members.email")} hint={t("enterprise.members.emailHint")}>
              <input
                className={inputClass}
                value={email}
                onChange={(event) => setEmail(event.target.value)}
                placeholder="ada@example.com"
              />
            </Field>
            <Field label={t("enterprise.members.role")}>
              <select
                className={inputClass}
                value={role}
                onChange={(event) => setRole(event.target.value as EnterpriseRole)}
              >
                {roles.map((entry) => (
                  <option key={entry} value={entry}>
                    {t(`enterprise.roles.${entry}`)}
                  </option>
                ))}
              </select>
            </Field>
            <Button
              variant="primary"
              size="sm"
              loading={createMember.isPending}
              disabled={!subject.trim() || !tenantId || emailInvalid}
              onClick={() => createMember.mutate()}
            >
              <Plus className="h-3.5 w-3.5" />
              {t("enterprise.members.add")}
            </Button>
            {emailInvalid ? (
              <p className="text-[10px] text-danger">{t("enterprise.members.emailInvalid")}</p>
            ) : null}
          </div>
        </Panel>
        <Panel className="p-3">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-3.5 w-3.5 text-accent" />
            <h2 className="text-[12px] font-semibold text-ink">
              {t("enterprise.members.directoryNote")}
            </h2>
          </div>
        </Panel>
      </div>

      <Panel className="min-w-0 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <Users className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">
            {t("enterprise.members.listTitle")}
          </h2>
          <span className="ml-auto font-mono text-[10px] text-faint">{entries.length}</span>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-[0.12em] text-faint">
            {t("enterprise.members.filter")}
          </span>
          <select
            className={cn(inputClass, "h-7 w-auto min-w-[160px]")}
            value={scope}
            onChange={(event) => setScope(event.target.value)}
          >
            <option value="all">{t("enterprise.allTenants")}</option>
            {tenants.map((tenant) => (
              <option key={tenant.id} value={tenant.id}>
                {tenant.name}
              </option>
            ))}
          </select>
        </div>
        {membersQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : membersQuery.isError ? (
          <p className="mt-2 text-xs text-danger">
            {errorText(membersQuery.error, t("errors.loadFailed"))}
          </p>
        ) : entries.length === 0 ? (
          <EmptyState
            icon={<Users className="h-4 w-4" />}
            title={t("enterprise.members.empty")}
            hint={t("enterprise.members.emptyHint")}
          />
        ) : (
          <div className="mt-2 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[880px] border-collapse text-left text-[11px]">
              <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                <tr>
                  <th className="px-3 py-1.5 font-medium">
                    {t("enterprise.members.subjectColumn")}
                  </th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.members.tenant")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.members.role")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("common.createdAt")}</th>
                  <th className="px-3 py-1.5 font-medium" />
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => (
                  <tr key={entry.id} className="border-t border-line">
                    <td className="px-3 py-1.5">
                      <div className="text-[11.5px] text-ink">{entry.displayName}</div>
                      <div className="font-mono text-[9px] text-faint">{entry.subject}</div>
                      {entry.email ? (
                        <div className="text-[9.5px] text-muted">{entry.email}</div>
                      ) : null}
                    </td>
                    <td className="px-3 py-1.5 font-mono text-[9.5px] text-muted">
                      {entry.tenantId}
                    </td>
                    <td className="px-3 py-1.5">
                      <select
                        className={cn(inputClass, "h-7 w-auto min-w-[110px]")}
                        value={entry.role}
                        disabled={updateMember.isPending}
                        onChange={(event) =>
                          updateMember.mutate({
                            id: entry.id,
                            role: event.target.value as EnterpriseRole,
                          })
                        }
                      >
                        {roles.map((value) => (
                          <option key={value} value={value}>
                            {t(`enterprise.roles.${value}`)}
                          </option>
                        ))}
                      </select>
                    </td>
                    <td className="px-3 py-1.5">
                      <div className="flex flex-wrap items-center gap-1">
                        <Badge
                          tone={entry.status === "active" ? "success" : "danger"}
                          dot
                        >
                          {t(`enterprise.status.${entry.status}`)}
                        </Badge>
                        <Button
                          size="xs"
                          variant="ghost"
                          disabled={updateMember.isPending}
                          onClick={() =>
                            updateMember.mutate({
                              id: entry.id,
                              status: entry.status === "active" ? "disabled" : "active",
                            })
                          }
                        >
                          {entry.status === "active"
                            ? t("enterprise.members.disable")
                            : t("enterprise.members.enable")}
                        </Button>
                      </div>
                    </td>
                    <td className="px-3 py-1.5">
                      <Timestamp value={entry.createdAt} />
                    </td>
                    <td className="px-3 py-1.5 text-right">
                      {deleteId === entry.id ? (
                        <div className="flex justify-end gap-1">
                          <Button
                            size="xs"
                            variant="danger"
                            loading={deleteMember.isPending}
                            onClick={() => deleteMember.mutate(entry.id)}
                          >
                            {t("enterprise.members.confirmDelete")}
                          </Button>
                          <Button
                            size="xs"
                            variant="ghost"
                            onClick={() => setDeleteId(null)}
                          >
                            {t("common.cancel")}
                          </Button>
                        </div>
                      ) : (
                        <Button
                          size="xs"
                          variant="danger"
                          onClick={() => setDeleteId(entry.id)}
                        >
                          {t("enterprise.members.delete")}
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </div>
  );
}

function QuotasSection({
  enabled,
  tenants,
  tenantId,
  onTenant,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
  tenantId: string | null;
  onTenant: (id: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [draft, setDraft] = useState<QuotaDraft>(emptyQuotaDraft);

  const quotaQuery = useQuery({
    queryKey: ["enterprise", "quota", tenantId],
    queryFn: () => api.enterpriseQuota(tenantId ?? ""),
    enabled: enabled && Boolean(tenantId),
    retry: false,
  });

  useEffect(() => {
    if (quotaQuery.data) {
      setDraft(quotaDraftFrom(quotaQuery.data));
    }
  }, [quotaQuery.data]);

  const saveQuota = useMutation({
    mutationFn: () => api.enterpriseSaveQuota(tenantId ?? "", quotaBody(draft)),
    onSuccess: (quota) => {
      setDraft(quotaDraftFrom(quota));
      pushToast(t("enterprise.quotas.saved"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.quotas.saveFailed")), "error"),
  });

  return (
    <>
      <Panel className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <Gauge className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.quotas.title")}</h2>
          <div className="ml-auto flex flex-wrap items-center gap-1.5">
            <span className="text-[10px] uppercase tracking-[0.12em] text-faint">
              {t("enterprise.tokens.tenant")}
            </span>
            <select
              className={cn(inputClass, "h-7 w-auto min-w-[160px]")}
              value={tenantId ?? ""}
              onChange={(event) => onTenant(event.target.value)}
            >
              {tenants.map((tenant) => (
                <option key={tenant.id} value={tenant.id}>
                  {tenant.name}
                </option>
              ))}
            </select>
          </div>
        </div>
        <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.quotas.subtitle")}</p>
      </Panel>

      {!tenantId ? (
        <Panel className="p-4">
          <EmptyState
            icon={<Building2 className="h-4 w-4" />}
            title={t("enterprise.quotas.selectTenant")}
            hint={t("enterprise.tenants.emptyHint")}
          />
        </Panel>
      ) : (
        <Panel className="p-4">
          {quotaQuery.isLoading ? (
            <p className="text-xs text-muted">{t("common.loading")}</p>
          ) : quotaQuery.isError ? (
            <p className="text-xs text-danger">
              {errorText(quotaQuery.error, t("errors.loadFailed"))}
            </p>
          ) : (
            <>
              <div className="grid gap-2 min-[560px]:grid-cols-2 xl:grid-cols-4">
                {quotaFields.map((field) => (
                  <Field key={field} label={t(`enterprise.quotas.${field}`)}>
                    <input
                      className={inputClass}
                      type="number"
                      min={0}
                      step={1}
                      value={draft[field]}
                      onChange={(event) =>
                        setDraft((current) => ({ ...current, [field]: event.target.value }))
                      }
                    />
                  </Field>
                ))}
              </div>
              <div className="mt-3 flex flex-wrap items-center gap-1.5">
                <Button
                  variant="primary"
                  size="sm"
                  loading={saveQuota.isPending}
                  disabled={!quotaDraftValid(draft) || !tenantId}
                  onClick={() => saveQuota.mutate()}
                >
                  {t("common.save")}
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={!quotaQuery.data}
                  onClick={() => {
                    if (quotaQuery.data) setDraft(quotaDraftFrom(quotaQuery.data));
                  }}
                >
                  {t("enterprise.quotas.reset")}
                </Button>
                <span className="ml-auto text-[10px] text-faint">
                  {t("enterprise.quotas.hint")}
                </span>
              </div>
            </>
          )}
        </Panel>
      )}
    </>
  );
}

function WorkersSection({
  enabled,
  tenants,
  tenantId,
  onTenant,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
  tenantId: string | null;
  onTenant: (id: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [name, setName] = useState("");
  const [kind, setKind] = useState("local");
  const [capabilities, setCapabilities] = useState("workflow");
  const [maxConcurrency, setMaxConcurrency] = useState("2");
  const [metadata, setMetadata] = useState("{}");
  const [issuedToken, setIssuedToken] = useState<{
    token: string;
    tokenId?: string;
    workerId: string;
    workerName: string;
  } | null>(null);
  const [dispatchWorkerId, setDispatchWorkerId] = useState("");
  const [dispatchWorkflowId, setDispatchWorkflowId] = useState("");
  const [dispatchVersion, setDispatchVersion] = useState("");
  const [dispatchInput, setDispatchInput] = useState("{}");
  const [jobScope, setJobScope] = useState<"all" | "tenant">("all");
  const [cancelJobId, setCancelJobId] = useState<string | null>(null);

  const workersQuery = useQuery({
    queryKey: ["enterprise", "workers", tenantId],
    queryFn: () => api.enterpriseWorkers(tenantId ?? ""),
    enabled: enabled && Boolean(tenantId),
    retry: false,
  });

  const workflowsQuery = useQuery({
    queryKey: ["enterprise", "tenant-workflows", tenantId],
    queryFn: () => api.enterpriseTenantWorkflows(tenantId ?? ""),
    enabled: enabled && Boolean(tenantId),
    retry: false,
  });

  const jobsQuery = useQuery({
    queryKey: ["enterprise", "worker-jobs", jobScope, tenantId],
    queryFn: () =>
      api.enterpriseWorkerJobs(jobScope === "tenant" ? tenantId ?? undefined : undefined, 50),
    enabled: enabled && (jobScope === "all" || Boolean(tenantId)),
    retry: false,
    refetchInterval: 5_000,
  });

  const workers = workersQuery.data?.workers ?? [];
  const workflows = workflowsQuery.data?.workflows ?? [];
  const jobs = jobsQuery.data?.jobs ?? [];
  const capabilityList = splitCapabilities(capabilities);
  const maxConcurrencyValue = Number(maxConcurrency);
  const maxConcurrencyValid =
    Number.isInteger(maxConcurrencyValue) &&
    maxConcurrencyValue >= 1 &&
    maxConcurrencyValue <= 64;
  let dispatchInputValid = true;
  try {
    JSON.parse(dispatchInput);
  } catch {
    dispatchInputValid = false;
  }
  const workflowFallback =
    workflowsQuery.isError || (!workflowsQuery.isLoading && workflows.length === 0);

  useEffect(() => {
    setDispatchWorkerId("");
    setDispatchWorkflowId("");
    setDispatchVersion("");
  }, [tenantId]);

  useEffect(() => {
    if (workflows.length === 0) return;
    if (!workflows.some((workflow) => workflow.id === dispatchWorkflowId)) {
      setDispatchWorkflowId(workflows[0]?.id ?? "");
    }
  }, [workflows, dispatchWorkflowId]);

  useEffect(() => {
    if (!dispatchWorkerId) return;
    if (!workers.some((worker) => worker.id === dispatchWorkerId)) {
      setDispatchWorkerId("");
    }
  }, [workers, dispatchWorkerId]);

  const registerWorker = useMutation({
    mutationFn: () => {
      let metadataValue: JsonValue | undefined;
      try {
        metadataValue = parseMetadata(metadata);
      } catch {
        throw new Error(t("enterprise.invalidJson"));
      }
      return api.enterpriseRegisterWorker({
        tenantId: tenantId ?? "",
        name: name.trim(),
        kind: kind.trim() || "local",
        metadata: metadataValue,
        capabilities: capabilityList,
        maxConcurrency: maxConcurrencyValue,
      });
    },
    onSuccess: (worker) => {
      if (worker.workerToken) {
        setIssuedToken({
          token: worker.workerToken,
          workerId: worker.id,
          workerName: worker.name,
        });
      }
      setName("");
      setKind("local");
      setCapabilities("workflow");
      setMaxConcurrency("2");
      setMetadata("{}");
      pushToast(t("enterprise.workers.registered"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.workers.registerFailed")), "error"),
  });

  const rotateToken = useMutation({
    mutationFn: (id: string) => api.enterpriseRotateWorkerToken(id),
    onSuccess: (result, id) => {
      const worker = workers.find((entry) => entry.id === id);
      setIssuedToken({
        token: result.workerToken,
        tokenId: result.tokenId,
        workerId: id,
        workerName: worker?.name ?? id,
      });
      pushToast(t("enterprise.workers.rotateDone"), "success");
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.workers.rotateFailed")), "error"),
  });

  const heartbeat = useMutation({
    mutationFn: (id: string) => api.enterpriseWorkerHeartbeat(id),
    onSuccess: () => {
      pushToast(t("enterprise.workers.heartbeatDone"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.workers.heartbeatFailed")), "error"),
  });

  const disableWorker = useMutation({
    mutationFn: (id: string) => api.enterpriseDisableWorker(id),
    onSuccess: () => {
      pushToast(t("enterprise.workers.disabledToast"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.workers.disableFailed")), "error"),
  });

  const dispatchJob = useMutation({
    mutationFn: () => {
      let inputValue: JsonValue;
      try {
        inputValue = JSON.parse(dispatchInput) as JsonValue;
      } catch {
        throw new Error(t("enterprise.workers.inputInvalid"));
      }
      return api.enterpriseCreateWorkerJob({
        tenantId: tenantId ?? "",
        workflowId: dispatchWorkflowId.trim(),
        workerId: dispatchWorkerId || undefined,
        input: inputValue,
        version: dispatchVersion.trim() || undefined,
      });
    },
    onSuccess: () => {
      setDispatchInput("{}");
      pushToast(t("enterprise.workers.dispatched"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.workers.dispatchFailed")), "error"),
  });

  const cancelJob = useMutation({
    mutationFn: (id: string) => api.enterpriseCancelWorkerJob(id),
    onSuccess: () => {
      setCancelJobId(null);
      pushToast(t("enterprise.workers.jobCancelled"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.workers.cancelFailed")), "error"),
  });

  return (
    <>
      <Panel className="border-info/25 bg-info/5 p-3">
        <div className="flex flex-wrap items-center gap-2">
          <Server className="h-4 w-4 text-info" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.workers.demoTitle")}</h2>
          <span className="ml-auto text-[10px] text-faint">
            {t("enterprise.workers.demoBadge")}
          </span>
        </div>
        <p className="mt-1 text-[11px] text-muted">{t("enterprise.workers.demoText")}</p>
      </Panel>

      {issuedToken ? (
        <Panel className="border-success/30 p-4">
          <div className="flex flex-wrap items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-success" />
            <h2 className="text-sm font-semibold text-ink">
              {t("enterprise.workers.tokenOnceTitle")}
            </h2>
            <Button
              size="xs"
              variant="ghost"
              className="ml-auto"
              onClick={() => setIssuedToken(null)}
            >
              {t("common.close")}
            </Button>
          </div>
          <p className="mt-1 text-[10.5px] text-warning">
            {t("enterprise.workers.tokenOnceWarning")}
          </p>
          <div className="mt-2 flex flex-wrap items-start gap-1.5 rounded-sm border border-line bg-panel-2 p-2">
            <code className="min-w-0 flex-1 break-all font-mono text-[10.5px] text-ink">
              {issuedToken.token}
            </code>
            <CopyButton text={issuedToken.token} />
          </div>
          <p className="mt-1 text-[10px] text-faint">
            {issuedToken.workerName} · {issuedToken.workerId}
            {issuedToken.tokenId
              ? ` · ${t("enterprise.workers.tokenId")} ${issuedToken.tokenId}`
              : null}
          </p>
        </Panel>
      ) : null}

      <div className="grid min-w-0 gap-3 xl:grid-cols-[340px_minmax(0,1fr)]">
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <Plus className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">
              {t("enterprise.workers.registerTitle")}
            </h2>
          </div>
          <div className="mt-2 grid gap-2">
            <Field label={t("enterprise.workers.tenant")}>
              <select
                className={inputClass}
                value={tenantId ?? ""}
                onChange={(event) => onTenant(event.target.value)}
              >
                {tenants.map((tenant) => (
                  <option key={tenant.id} value={tenant.id}>
                    {tenant.name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label={t("enterprise.workers.name")}>
              <input
                className={inputClass}
                value={name}
                onChange={(event) => setName(event.target.value)}
                placeholder={t("enterprise.workers.namePlaceholder")}
              />
            </Field>
            <Field label={t("enterprise.workers.kind")} hint={t("enterprise.workers.kindHint")}>
              <input
                className={inputClass}
                value={kind}
                onChange={(event) => setKind(event.target.value)}
                placeholder={t("enterprise.workers.kindPlaceholder")}
              />
            </Field>
            <Field
              label={t("enterprise.workers.capabilities")}
              hint={t("enterprise.workers.capabilitiesHint")}
            >
              <input
                className={inputClass}
                value={capabilities}
                onChange={(event) => setCapabilities(event.target.value)}
                placeholder={t("enterprise.workers.capabilitiesPlaceholder")}
              />
            </Field>
            <Field
              label={t("enterprise.workers.maxConcurrency")}
              hint={t("enterprise.workers.maxConcurrencyHint")}
            >
              <input
                className={inputClass}
                type="number"
                min={1}
                max={64}
                step={1}
                value={maxConcurrency}
                onChange={(event) => setMaxConcurrency(event.target.value)}
              />
            </Field>
            {!maxConcurrencyValid ? (
              <p className="text-[10px] text-danger">
                {t("enterprise.workers.maxConcurrencyInvalid")}
              </p>
            ) : null}
            <Field label={t("enterprise.workers.metadata")} hint={t("enterprise.workers.metadataHint")}>
              <textarea
                className={cn(textareaClass, "min-h-[64px]")}
                value={metadata}
                onChange={(event) => setMetadata(event.target.value)}
                spellCheck={false}
              />
            </Field>
            <Button
              variant="primary"
              size="sm"
              loading={registerWorker.isPending}
              disabled={!name.trim() || !tenantId || !maxConcurrencyValid}
              onClick={() => registerWorker.mutate()}
            >
              <Server className="h-3.5 w-3.5" />
              {t("enterprise.workers.register")}
            </Button>
          </div>
        </Panel>

        <Panel className="min-w-0 p-4">
          <div className="flex flex-wrap items-center gap-2">
            <Server className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.workers.listTitle")}</h2>
            <span className="ml-auto font-mono text-[10px] text-faint">{workers.length}</span>
          </div>
          {!tenantId ? (
            <EmptyState
              icon={<Building2 className="h-4 w-4" />}
              title={t("enterprise.workers.selectTenant")}
            />
          ) : workersQuery.isLoading ? (
            <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
          ) : workersQuery.isError ? (
            <p className="mt-2 text-xs text-danger">
              {errorText(workersQuery.error, t("errors.loadFailed"))}
            </p>
          ) : workers.length === 0 ? (
            <EmptyState
              icon={<Server className="h-4 w-4" />}
              title={t("enterprise.workers.empty")}
              hint={t("enterprise.workers.emptyHint")}
            />
          ) : (
            <div className="mt-2 overflow-x-auto rounded-sm border border-line">
              <table className="w-full min-w-[1040px] border-collapse text-left text-[11px]">
                <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                  <tr>
                    <th className="px-3 py-1.5 font-medium">{t("enterprise.workers.name")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("enterprise.workers.kind")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                    <th className="px-3 py-1.5 font-medium">
                      {t("enterprise.workers.capabilities")}
                    </th>
                    <th className="px-3 py-1.5 font-medium">
                      {t("enterprise.workers.maxConcurrency")}
                    </th>
                    <th className="px-3 py-1.5 font-medium">
                      {t("enterprise.workers.runtimeVersion")}
                    </th>
                    <th className="px-3 py-1.5 font-medium">
                      {t("enterprise.workers.lastHeartbeat")}
                    </th>
                    <th className="px-3 py-1.5 font-medium">
                      {t("enterprise.workers.registeredAt")}
                    </th>
                    <th className="px-3 py-1.5 font-medium" />
                  </tr>
                </thead>
                <tbody>
                  {workers.map((worker) => (
                    <tr key={worker.id} className="border-t border-line">
                      <td className="px-3 py-1.5">
                        <div className="text-[11.5px] text-ink">{worker.name}</div>
                        <div className="font-mono text-[9px] text-faint">{worker.id}</div>
                      </td>
                      <td className="px-3 py-1.5">
                        <Badge tone="neutral">{worker.kind}</Badge>
                      </td>
                      <td className="px-3 py-1.5">
                        <Badge tone={workerTone(worker.status)} dot>
                          {t(`enterprise.status.${worker.status}`, {
                            defaultValue: worker.status,
                          })}
                        </Badge>
                      </td>
                      <td className="px-3 py-1.5">
                        {(worker.capabilities ?? []).length === 0 ? (
                          <span className="text-[10px] text-faint">—</span>
                        ) : (
                          <div className="flex flex-wrap gap-1">
                            {(worker.capabilities ?? []).map((capability) => (
                              <Badge key={capability} tone="neutral">
                                {capability}
                              </Badge>
                            ))}
                          </div>
                        )}
                      </td>
                      <td className="px-3 py-1.5 font-mono text-[10.5px] text-muted">
                        {worker.maxConcurrency ?? "—"}
                      </td>
                      <td className="px-3 py-1.5">
                        {worker.runtimeVersion ? (
                          <span className="font-mono text-[10px] text-muted">
                            {worker.runtimeVersion}
                          </span>
                        ) : (
                          <span className="text-[10px] text-faint">—</span>
                        )}
                      </td>
                      <td className="px-3 py-1.5">
                        {worker.lastHeartbeatAt ? (
                          <Timestamp value={worker.lastHeartbeatAt} />
                        ) : (
                          <span className="text-[10px] text-faint">
                            {t("enterprise.workers.never")}
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-1.5">
                        <Timestamp value={worker.registeredAt} />
                      </td>
                      <td className="px-3 py-1.5">
                        <div className="flex flex-wrap justify-end gap-1">
                          <Button
                            size="xs"
                            variant="secondary"
                            loading={heartbeat.isPending && heartbeat.variables === worker.id}
                            disabled={worker.status === "disabled"}
                            onClick={() => heartbeat.mutate(worker.id)}
                          >
                            <Activity className="h-3 w-3" />
                            {t("enterprise.workers.heartbeat")}
                          </Button>
                          <Button
                            size="xs"
                            variant="secondary"
                            loading={
                              rotateToken.isPending && rotateToken.variables === worker.id
                            }
                            onClick={() => rotateToken.mutate(worker.id)}
                          >
                            <RotateCw className="h-3 w-3" />
                            {t("enterprise.workers.rotateToken")}
                          </Button>
                          <Button
                            size="xs"
                            variant="danger"
                            loading={
                              disableWorker.isPending && disableWorker.variables === worker.id
                            }
                            disabled={worker.status === "disabled"}
                            onClick={() => disableWorker.mutate(worker.id)}
                          >
                            <PowerOff className="h-3 w-3" />
                            {t("enterprise.workers.disable")}
                          </Button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Panel>
      </div>

      <Panel className="min-w-0 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <Send className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">
            {t("enterprise.workers.dispatchTitle")}
          </h2>
        </div>
        <p className="mt-1 text-[11px] text-muted">{t("enterprise.workers.dispatchHint")}</p>
        <div className="mt-2 grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
          <Field label={t("enterprise.workers.tenant")}>
            <select
              className={inputClass}
              value={tenantId ?? ""}
              onChange={(event) => onTenant(event.target.value)}
            >
              {tenants.map((tenant) => (
                <option key={tenant.id} value={tenant.id}>
                  {tenant.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("enterprise.workers.worker")} hint={t("enterprise.workers.workerHint")}>
            <select
              className={inputClass}
              value={dispatchWorkerId}
              onChange={(event) => setDispatchWorkerId(event.target.value)}
            >
              <option value="">{t("enterprise.workers.anyWorker")}</option>
              {workers.map((worker) => (
                <option key={worker.id} value={worker.id}>
                  {worker.name}
                </option>
              ))}
            </select>
          </Field>
          <Field
            label={t("enterprise.workers.workflow")}
            hint={
              workflowFallback ? t("enterprise.workers.workflowFallbackHint") : undefined
            }
          >
            {workflowsQuery.isLoading ? (
              <select className={inputClass} disabled value="">
                <option>{t("common.loading")}</option>
              </select>
            ) : workflowFallback ? (
              <input
                className={inputClass}
                value={dispatchWorkflowId}
                onChange={(event) => setDispatchWorkflowId(event.target.value)}
                placeholder={t("enterprise.workers.workflowPlaceholder")}
              />
            ) : (
              <select
                className={inputClass}
                value={dispatchWorkflowId}
                onChange={(event) => setDispatchWorkflowId(event.target.value)}
              >
                <option value="">{t("enterprise.workers.selectWorkflow")}</option>
                {workflows.map((workflow) => (
                  <option key={workflow.id} value={workflow.id}>
                    {workflow.name} · {workflow.id} · v{workflow.version}
                  </option>
                ))}
              </select>
            )}
          </Field>
          <Field label={t("enterprise.workers.version")} hint={t("enterprise.workers.versionHint")}>
            <input
              className={inputClass}
              value={dispatchVersion}
              onChange={(event) => setDispatchVersion(event.target.value)}
              placeholder="1.0.0"
            />
          </Field>
          <Field
            label={t("enterprise.workers.input")}
            hint={t("enterprise.workers.inputHint")}
            className="sm:col-span-2"
          >
            <textarea
              className={cn(textareaClass, "min-h-[72px]")}
              value={dispatchInput}
              onChange={(event) => setDispatchInput(event.target.value)}
              spellCheck={false}
            />
          </Field>
        </div>
        {workflowsQuery.isError ? (
          <p className="mt-1 text-[10px] text-danger">
            {errorText(workflowsQuery.error, t("enterprise.workers.workflowLoadFailed"))}
          </p>
        ) : null}
        {!dispatchInputValid ? (
          <p className="mt-1 text-[10px] text-danger">
            {t("enterprise.workers.inputInvalid")}
          </p>
        ) : null}
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <Button
            variant="primary"
            size="sm"
            loading={dispatchJob.isPending}
            disabled={!tenantId || !dispatchWorkflowId.trim() || !dispatchInputValid}
            onClick={() => dispatchJob.mutate()}
          >
            <Send className="h-3.5 w-3.5" />
            {t("enterprise.workers.dispatch")}
          </Button>
        </div>
      </Panel>

      <Panel className="min-w-0 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <ListChecks className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.workers.queueTitle")}</h2>
          <span className="font-mono text-[10px] text-faint">{jobs.length}</span>
          <Button
            size="xs"
            variant="secondary"
            className="ml-auto"
            loading={jobsQuery.isFetching}
            onClick={() => void jobsQuery.refetch()}
          >
            <RefreshCw className="h-3 w-3" />
            {t("common.refresh")}
          </Button>
        </div>
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-[0.12em] text-faint">
            {t("enterprise.workers.queueFilter")}
          </span>
          <select
            className={cn(inputClass, "h-7 w-auto min-w-[170px]")}
            value={jobScope}
            onChange={(event) => setJobScope(event.target.value as "all" | "tenant")}
          >
            <option value="all">{t("enterprise.workers.filterAll")}</option>
            <option value="tenant">{t("enterprise.workers.filterSelected")}</option>
          </select>
        </div>
        {jobScope === "tenant" && !tenantId ? (
          <EmptyState
            icon={<Building2 className="h-4 w-4" />}
            title={t("enterprise.workers.selectTenant")}
          />
        ) : jobsQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : jobsQuery.isError ? (
          <p className="mt-2 text-xs text-danger">
            {errorText(jobsQuery.error, t("errors.loadFailed"))}
          </p>
        ) : jobs.length === 0 ? (
          <EmptyState
            icon={<ListChecks className="h-4 w-4" />}
            title={t("enterprise.workers.emptyJobs")}
            hint={t("enterprise.workers.emptyJobsHint")}
          />
        ) : (
          <div className="mt-2 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[880px] border-collapse text-left text-[11px]">
              <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                <tr>
                  <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                  <th className="px-3 py-1.5 font-medium">
                    {t("enterprise.workers.jobWorkflow")}
                  </th>
                  <th className="px-3 py-1.5 font-medium">
                    {t("enterprise.workers.jobWorker")}
                  </th>
                  <th className="px-3 py-1.5 font-medium">
                    {t("enterprise.workers.jobAttempts")}
                  </th>
                  <th className="px-3 py-1.5 font-medium">
                    {t("enterprise.workers.jobLease")}
                  </th>
                  <th className="px-3 py-1.5 font-medium">
                    {t("enterprise.workers.jobExecution")}
                  </th>
                  <th className="px-3 py-1.5 font-medium" />
                </tr>
              </thead>
              <tbody>
                {jobs.map((job) => {
                  const jobWorker = job.workerId
                    ? workers.find((worker) => worker.id === job.workerId)
                    : null;
                  const lease = epochTimestamp(job.leaseExpiresAt);
                  return (
                    <tr key={job.id} className="border-t border-line">
                      <td className="px-3 py-1.5">
                        <div className="flex flex-wrap items-center gap-1">
                          <Badge tone={workerJobTone(job.status)} dot>
                            {t(`enterprise.workers.jobStatus.${job.status}`, {
                              defaultValue: job.status,
                            })}
                          </Badge>
                          {job.cancelRequested && !isTerminalWorkerJob(job.status) ? (
                            <Badge tone="warning">
                              {t("enterprise.workers.cancelRequested")}
                            </Badge>
                          ) : null}
                        </div>
                        {job.error ? (
                          <div
                            className="mt-0.5 max-w-[200px] truncate text-[9.5px] text-danger"
                            title={job.error}
                          >
                            {job.error}
                          </div>
                        ) : null}
                      </td>
                      <td className="px-3 py-1.5">
                        <div className="font-mono text-[10.5px] text-ink">{job.workflowId}</div>
                        <div className="font-mono text-[9px] text-faint">
                          v{job.workflowVersion}
                        </div>
                      </td>
                      <td className="px-3 py-1.5">
                        {job.workerId ? (
                          <div>
                            <div className="text-[11px] text-ink">
                              {jobWorker?.name ?? job.workerId}
                            </div>
                            {jobWorker ? (
                              <div className="font-mono text-[9px] text-faint">
                                {job.workerId}
                              </div>
                            ) : null}
                          </div>
                        ) : (
                          <span className="text-[10px] text-faint">
                            {t("enterprise.workers.anyWorker")}
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-1.5 font-mono text-[10.5px] text-muted">
                        {job.attempts}
                      </td>
                      <td className="px-3 py-1.5">
                        {lease ? (
                          <Timestamp value={lease} />
                        ) : (
                          <span className="text-[10px] text-faint">—</span>
                        )}
                      </td>
                      <td className="px-3 py-1.5">
                        <Link
                          to="/executions/$executionId"
                          params={{ executionId: job.executionId }}
                          className="font-mono text-[10px] text-accent-text hover:underline"
                        >
                          {job.executionId}
                        </Link>
                      </td>
                      <td className="px-3 py-1.5">
                        {isTerminalWorkerJob(job.status) ? null : cancelJobId === job.id ? (
                          <div className="flex flex-wrap justify-end gap-1">
                            <Button
                              size="xs"
                              variant="danger"
                              loading={
                                cancelJob.isPending && cancelJob.variables === job.id
                              }
                              onClick={() => cancelJob.mutate(job.id)}
                            >
                              {t("enterprise.workers.confirmCancelJob")}
                            </Button>
                            <Button
                              size="xs"
                              variant="ghost"
                              onClick={() => setCancelJobId(null)}
                            >
                              {t("common.cancel")}
                            </Button>
                          </div>
                        ) : (
                          <Button
                            size="xs"
                            variant="danger"
                            disabled={job.cancelRequested}
                            onClick={() => setCancelJobId(job.id)}
                          >
                            <Ban className="h-3 w-3" />
                            {t("enterprise.workers.cancelJob")}
                          </Button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </>
  );
}

function BackupsSection({
  enabled,
  tenants,
  tenantId,
  onTenant,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
  tenantId: string | null;
  onTenant: (id: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [retainDays, setRetainDays] = useState("30");
  const [retainCount, setRetainCount] = useState("10");
  const [restoreId, setRestoreId] = useState<string | null>(null);

  const backupsQuery = useQuery({
    queryKey: ["enterprise", "backups", tenantId],
    queryFn: () => api.enterpriseBackups(tenantId ?? ""),
    enabled: enabled && Boolean(tenantId),
    retry: false,
  });

  const backups = backupsQuery.data?.backups ?? [];
  const restoreTarget = backups.find((backup) => backup.id === restoreId) ?? null;

  const createBackup = useMutation({
    mutationFn: () => api.enterpriseCreateBackup(tenantId ?? ""),
    onSuccess: () => {
      pushToast(t("enterprise.backups.created"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.backups.createFailed")), "error"),
  });

  const deleteBackup = useMutation({
    mutationFn: (id: string) => api.enterpriseDeleteBackup(id),
    onSuccess: () => {
      pushToast(t("enterprise.backups.deleted"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.backups.deleteFailed")), "error"),
  });

  const restoreBackup = useMutation({
    mutationFn: (id: string) => api.enterpriseRestoreBackup(id),
    onSuccess: (result) => {
      setRestoreId(null);
      pushToast(
        t(
          result.requiresRestart
            ? "enterprise.backups.restoredRestart"
            : "enterprise.backups.restored",
        ),
        result.requiresRestart ? "warning" : "success",
      );
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.backups.restoreFailed")), "error"),
  });

  const purge = useMutation({
    mutationFn: () =>
      api.enterprisePurgeBackups(Number(retainDays), Number(retainCount)),
    onSuccess: (result) => {
      pushToast(t("enterprise.backups.purged", { deleted: result.deleted }), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.backups.purgeFailed")), "error"),
  });

  const purgeValid =
    Number.isInteger(Number(retainDays)) &&
    Number(retainDays) >= 0 &&
    Number.isInteger(Number(retainCount)) &&
    Number(retainCount) >= 0;

  return (
    <>
      <Panel className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <DatabaseBackup className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.backups.title")}</h2>
          <div className="ml-auto flex flex-wrap items-center gap-1.5">
            <select
              className={cn(inputClass, "h-7 w-auto min-w-[160px]")}
              value={tenantId ?? ""}
              onChange={(event) => {
                onTenant(event.target.value);
                setRestoreId(null);
              }}
            >
              {tenants.map((tenant) => (
                <option key={tenant.id} value={tenant.id}>
                  {tenant.name}
                </option>
              ))}
            </select>
            <Button
              size="sm"
              variant="primary"
              loading={createBackup.isPending}
              disabled={!tenantId}
              onClick={() => createBackup.mutate()}
            >
              <Plus className="h-3.5 w-3.5" />
              {t("enterprise.backups.create")}
            </Button>
          </div>
        </div>
        <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.backups.subtitle")}</p>
      </Panel>

      <div className="grid min-w-0 gap-3 xl:grid-cols-[minmax(0,1fr)_320px]">
        <Panel className="min-w-0 p-4">
          <div className="flex items-center gap-2">
            <HardDrive className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.backups.listTitle")}</h2>
            <span className="ml-auto font-mono text-[10px] text-faint">{backups.length}</span>
          </div>
          {!tenantId ? (
            <EmptyState
              icon={<Building2 className="h-4 w-4" />}
              title={t("enterprise.backups.selectTenant")}
            />
          ) : backupsQuery.isLoading ? (
            <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
          ) : backupsQuery.isError ? (
            <p className="mt-2 text-xs text-danger">
              {errorText(backupsQuery.error, t("errors.loadFailed"))}
            </p>
          ) : backups.length === 0 ? (
            <EmptyState
              icon={<DatabaseBackup className="h-4 w-4" />}
              title={t("enterprise.backups.empty")}
              hint={t("enterprise.backups.emptyHint")}
            />
          ) : (
            <div className="mt-2 overflow-x-auto rounded-sm border border-line">
              <table className="w-full min-w-[820px] border-collapse text-left text-[11px]">
                <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                  <tr>
                    <th className="px-3 py-1.5 font-medium">{t("common.createdAt")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("enterprise.backups.path")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("enterprise.backups.size")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("enterprise.backups.hash")}</th>
                    <th className="px-3 py-1.5 font-medium">{t("common.status")}</th>
                    <th className="px-3 py-1.5 font-medium" />
                  </tr>
                </thead>
                <tbody>
                  {backups.map((backup: EnterpriseBackup) => (
                    <tr key={backup.id} className="border-t border-line">
                      <td className="px-3 py-1.5">
                        <Timestamp value={backup.createdAt} />
                      </td>
                      <td className="max-w-[220px] truncate px-3 py-1.5 font-mono text-[9.5px] text-muted">
                        {backup.path}
                      </td>
                      <td className="px-3 py-1.5 font-mono text-[10px] text-ink">
                        {formatBytes(backup.size)}
                      </td>
                      <td className="px-3 py-1.5">
                        <div className="flex items-center gap-1">
                          <span className="font-mono text-[9.5px] text-faint">
                            {backup.sha256.slice(0, 16)}
                          </span>
                          <CopyButton text={backup.sha256} />
                        </div>
                      </td>
                      <td className="px-3 py-1.5">
                        <Badge tone={backupTone(backup.status)} dot>
                          {t(`enterprise.status.${backup.status}`, {
                            defaultValue: backup.status,
                          })}
                        </Badge>
                      </td>
                      <td className="px-3 py-1.5">
                        <div className="flex flex-wrap justify-end gap-1">
                          <Button
                            size="xs"
                            variant="secondary"
                            onClick={() => setRestoreId(backup.id)}
                          >
                            {t("enterprise.backups.restore")}
                          </Button>
                          <Button
                            size="xs"
                            variant="danger"
                            loading={
                              deleteBackup.isPending && deleteBackup.variables === backup.id
                            }
                            onClick={() => deleteBackup.mutate(backup.id)}
                          >
                            <Trash2 className="h-3 w-3" />
                            {t("common.delete")}
                          </Button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {restoreTarget ? (
            <div className="mt-3 rounded-sm border border-warning/35 bg-warning/5 p-2.5">
              <p className="text-[11px] font-semibold text-warning">
                {t("enterprise.backups.restoreConfirmTitle")}
              </p>
              <p className="mt-1 text-[10.5px] text-muted">
                {t("enterprise.backups.restoreWarning")}
              </p>
              <p className="mt-1 break-all font-mono text-[9.5px] text-faint">
                {restoreTarget.path} · <Timestamp value={restoreTarget.createdAt} />
              </p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <Button
                  size="sm"
                  variant="danger"
                  loading={restoreBackup.isPending}
                  onClick={() => restoreBackup.mutate(restoreTarget.id)}
                >
                  {t("enterprise.backups.restoreConfirm")}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setRestoreId(null)}
                >
                  {t("common.cancel")}
                </Button>
              </div>
            </div>
          ) : null}
        </Panel>

        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <Trash2 className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.backups.purgeTitle")}</h2>
          </div>
          <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.backups.purgeHint")}</p>
          <div className="mt-2 grid grid-cols-2 gap-2">
            <Field label={t("enterprise.backups.retainDays")}>
              <input
                className={inputClass}
                type="number"
                min={0}
                step={1}
                value={retainDays}
                onChange={(event) => setRetainDays(event.target.value)}
              />
            </Field>
            <Field label={t("enterprise.backups.retainCount")}>
              <input
                className={inputClass}
                type="number"
                min={0}
                step={1}
                value={retainCount}
                onChange={(event) => setRetainCount(event.target.value)}
              />
            </Field>
          </div>
          <Button
            className="mt-2"
            size="sm"
            variant="danger"
            loading={purge.isPending}
            disabled={!purgeValid}
            onClick={() => purge.mutate()}
          >
            <Trash2 className="h-3.5 w-3.5" />
            {t("enterprise.backups.purge")}
          </Button>
        </Panel>
      </div>
    </>
  );
}

function AuditSection({
  enabled,
  tenants,
}: {
  enabled: boolean;
  tenants: EnterpriseTenant[];
}) {
  const { t } = useTranslation();
  const pushToast = useUiStore((state) => state.pushToast);
  const [scope, setScope] = useState("all");
  const [limit, setLimit] = useState("50");
  const [exporting, setExporting] = useState<"json" | "csv" | null>(null);

  const tenantParam = scope === "all" ? undefined : scope;
  const limitValue = Number(limit) || 50;
  const auditQuery = useQuery({
    queryKey: ["enterprise", "audit", tenantParam ?? "all", limitValue],
    queryFn: () => api.enterpriseAudit(tenantParam, limitValue),
    enabled,
    retry: false,
  });

  const entries = auditQuery.data?.entries ?? [];

  async function exportAudit(format: "json" | "csv") {
    setExporting(format);
    try {
      const blob = await api.enterpriseExportAudit(tenantParam, format);
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `enterprise-audit-${new Date().toISOString().slice(0, 10)}.${format}`;
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(url);
      pushToast(t("enterprise.audit.exported"), "success");
    } catch (err) {
      pushToast(errorText(err, t("enterprise.audit.exportFailed")), "error");
    } finally {
      setExporting(null);
    }
  }

  return (
    <>
      <Panel className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <ScrollText className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.audit.title")}</h2>
          <div className="ml-auto flex flex-wrap items-center gap-1.5">
            <select
              className={cn(inputClass, "h-7 w-auto min-w-[150px]")}
              value={scope}
              onChange={(event) => setScope(event.target.value)}
              aria-label={t("enterprise.audit.filterTenant")}
            >
              <option value="all">{t("enterprise.allTenants")}</option>
              {tenants.map((tenant) => (
                <option key={tenant.id} value={tenant.id}>
                  {tenant.name}
                </option>
              ))}
            </select>
            <select
              className={cn(inputClass, "h-7 w-auto")}
              value={limit}
              onChange={(event) => setLimit(event.target.value)}
              aria-label={t("enterprise.audit.limit")}
            >
              {["50", "100", "200", "500"].map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
            <Button
              size="sm"
              variant="secondary"
              loading={exporting === "json"}
              disabled={exporting !== null}
              onClick={() => void exportAudit("json")}
            >
              <Download className="h-3.5 w-3.5" />
              {t("enterprise.audit.exportJson")}
            </Button>
            <Button
              size="sm"
              variant="secondary"
              loading={exporting === "csv"}
              disabled={exporting !== null}
              onClick={() => void exportAudit("csv")}
            >
              <Download className="h-3.5 w-3.5" />
              {t("enterprise.audit.exportCsv")}
            </Button>
          </div>
        </div>
        <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.audit.subtitle")}</p>
      </Panel>

      <Panel className="min-w-0 p-4">
        <div className="flex items-center gap-2">
          <Activity className="h-4 w-4 text-accent" />
          <span className="text-sm font-semibold text-ink">{t("enterprise.audit.events")}</span>
          <span className="ml-auto font-mono text-[10px] text-faint">{entries.length}</span>
        </div>
        {auditQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : auditQuery.isError ? (
          <p className="mt-2 text-xs text-danger">
            {errorText(auditQuery.error, t("errors.loadFailed"))}
          </p>
        ) : entries.length === 0 ? (
          <EmptyState
            icon={<ScrollText className="h-4 w-4" />}
            title={t("enterprise.audit.empty")}
            hint={t("enterprise.audit.emptyHint")}
          />
        ) : (
          <div className="mt-2 overflow-x-auto rounded-sm border border-line">
            <table className="w-full min-w-[1000px] border-collapse text-left text-[11px]">
              <thead className="bg-panel-3 text-[9.5px] uppercase tracking-[0.12em] text-faint">
                <tr>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.time")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.tenant")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.actor")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.action")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.decision")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.resource")}</th>
                  <th className="px-3 py-1.5 font-medium">{t("enterprise.audit.reason")}</th>
                </tr>
              </thead>
              <tbody>
                {entries.map((entry: EnterpriseAuditEntry) => (
                  <tr key={entry.id} className="border-t border-line align-top">
                    <td className="whitespace-nowrap px-3 py-1.5">
                      <Timestamp value={entry.at} />
                    </td>
                    <td className="px-3 py-1.5 font-mono text-[9.5px] text-muted">
                      {entry.tenantId}
                    </td>
                    <td className="max-w-[150px] truncate px-3 py-1.5 font-mono text-[9.5px] text-muted">
                      {entry.actorType}:{entry.actorId}
                    </td>
                    <td className="px-3 py-1.5 font-mono text-[10px] text-ink">{entry.action}</td>
                    <td className="px-3 py-1.5">
                      <Badge tone={decisionTone(entry.decision)}>
                        {t(`enterprise.decision.${entry.decision}`, {
                          defaultValue: entry.decision,
                        })}
                      </Badge>
                    </td>
                    <td className="max-w-[200px] truncate px-3 py-1.5 font-mono text-[9.5px] text-muted">
                      {entry.resource}
                    </td>
                    <td className="max-w-[220px] truncate px-3 py-1.5 text-[10px] text-faint">
                      {entry.reason}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Panel>
    </>
  );
}

function splitScopes(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function splitLines(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function OidcSection({ enabled }: { enabled: boolean }) {
  const { t } = useTranslation();
  const { authenticated, principal, logout, refreshSession } = useAuth();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [issuer, setIssuer] = useState("");
  const [audience, setAudience] = useState("");
  const [jwksUrl, setJwksUrl] = useState("");
  const [network, setNetwork] = useState("");
  const [clientId, setClientId] = useState("");
  const [authorizationEndpoint, setAuthorizationEndpoint] = useState("");
  const [tokenEndpoint, setTokenEndpoint] = useState("");
  const [redirectUri, setRedirectUri] = useState("");
  const [endSessionEndpoint, setEndSessionEndpoint] = useState("");
  const [postLogoutRedirectUri, setPostLogoutRedirectUri] = useState("");
  const [scopes, setScopes] = useState("openid email profile");
  const [adminSubjects, setAdminSubjects] = useState("");
  const [cookieSecure, setCookieSecure] = useState(false);
  const [sessionTtl, setSessionTtl] = useState("28800");

  const browserConfigured = Boolean(
    clientId.trim() && authorizationEndpoint.trim() && tokenEndpoint.trim() && issuer.trim(),
  );
  const ttlValue = Number(sessionTtl.trim());
  const ttlValid = Number.isFinite(ttlValue) && ttlValue >= 300 && ttlValue <= 604800;

  const oidcQuery = useQuery({
    queryKey: ["enterprise", "oidc"],
    queryFn: api.enterpriseOidc,
    enabled,
    retry: false,
  });

  useEffect(() => {
    if (!oidcQuery.data) return;
    const data = oidcQuery.data;
    setIssuer(data.issuer);
    setAudience(data.audience);
    setJwksUrl(data.jwksUrl);
    setNetwork(data.network.join("\n"));
    setClientId(data.clientId ?? "");
    setAuthorizationEndpoint(data.authorizationEndpoint ?? "");
    setTokenEndpoint(data.tokenEndpoint ?? "");
    setRedirectUri(data.redirectUri ?? "");
    setEndSessionEndpoint(data.endSessionEndpoint ?? "");
    setPostLogoutRedirectUri(data.postLogoutRedirectUri ?? "");
    setScopes((data.scopes ?? []).join("\n"));
    setAdminSubjects((data.adminSubjects ?? []).join("\n"));
    setCookieSecure(Boolean(data.cookieSecure));
    setSessionTtl(String(data.sessionTtlSeconds ?? 28800));
  }, [oidcQuery.data]);

  const saveOidc = useMutation({
    mutationFn: () => {
      const resolvedRedirectUri =
        redirectUri.trim() || `${window.location.origin}/auth/oidc/callback`;
      const resolvedPostLogoutRedirectUri =
        postLogoutRedirectUri.trim() || `${window.location.origin}/auth/oidc/logout/callback`;
      return api.enterpriseSaveOidc({
        issuer: issuer.trim(),
        audience: audience.trim(),
        jwksUrl: jwksUrl.trim(),
        network: splitLines(network),
        clientId: clientId.trim(),
        authorizationEndpoint: authorizationEndpoint.trim(),
        tokenEndpoint: tokenEndpoint.trim(),
        redirectUri: resolvedRedirectUri,
        endSessionEndpoint: endSessionEndpoint.trim(),
        postLogoutRedirectUri: endSessionEndpoint.trim() ? resolvedPostLogoutRedirectUri : "",
        scopes: splitScopes(scopes),
        adminSubjects: splitLines(adminSubjects),
        cookieSecure,
        sessionTtlSeconds: ttlValid ? Math.round(ttlValue) : 28800,
      });
    },
    onSuccess: (result) => {
      setIssuer(result.issuer);
      setAudience(result.audience);
      setJwksUrl(result.jwksUrl);
      setNetwork(result.network.join("\n"));
      setClientId(result.clientId ?? "");
      setAuthorizationEndpoint(result.authorizationEndpoint ?? "");
      setTokenEndpoint(result.tokenEndpoint ?? "");
      setRedirectUri(result.redirectUri ?? "");
      setEndSessionEndpoint(result.endSessionEndpoint ?? "");
      setPostLogoutRedirectUri(result.postLogoutRedirectUri ?? "");
      setScopes((result.scopes ?? []).join("\n"));
      setAdminSubjects((result.adminSubjects ?? []).join("\n"));
      setCookieSecure(Boolean(result.cookieSecure));
      setSessionTtl(String(result.sessionTtlSeconds ?? 28800));
      pushToast(t("enterprise.oidc.saved"), "success");
      void queryClient.invalidateQueries({ queryKey: ["enterprise"] });
      void refreshSession();
    },
    onError: (err) =>
      pushToast(errorText(err, t("enterprise.oidc.saveFailed")), "error"),
  });

  async function signOut() {
    await logout();
    queryClient.removeQueries({ queryKey: ["enterprise"] });
  }

  return (
    <div className="grid min-w-0 gap-3 xl:grid-cols-[minmax(0,1fr)_320px]">
      <Panel className="p-4">
        <div className="flex flex-wrap items-center gap-2">
          <LockKeyhole className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.oidc.title")}</h2>
          <Badge tone="accent">{t("enterprise.oidc.flowBadge")}</Badge>
          <Badge tone={browserConfigured ? "success" : "neutral"} dot className="ml-auto">
            {browserConfigured
              ? t("enterprise.oidc.ssoConfigured")
              : t("enterprise.oidc.bearerOnly")}
          </Badge>
        </div>
        <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.oidc.subtitle")}</p>
        {oidcQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : oidcQuery.isError ? (
          <p className="mt-2 text-xs text-danger">
            {errorText(oidcQuery.error, t("errors.loadFailed"))}
          </p>
        ) : (
          <div className="mt-3 grid gap-2">
            <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
              {t("enterprise.oidc.groupToken")}
            </p>
            <Field label={t("enterprise.oidc.issuer")}>
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={issuer}
                onChange={(event) => setIssuer(event.target.value)}
                placeholder="https://issuer.example.com"
              />
            </Field>
            <Field label={t("enterprise.oidc.audience")} hint={t("enterprise.oidc.audienceHint")}>
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={audience}
                onChange={(event) => setAudience(event.target.value)}
                placeholder="hpa-enterprise"
              />
            </Field>
            <Field label={t("enterprise.oidc.jwksUrl")}>
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={jwksUrl}
                onChange={(event) => setJwksUrl(event.target.value)}
                placeholder="https://issuer.example.com/.well-known/jwks.json"
              />
            </Field>
            <Field label={t("enterprise.oidc.network")} hint={t("enterprise.oidc.networkHint")}>
              <textarea
                className={cn(textareaClass, "min-h-[70px]")}
                value={network}
                onChange={(event) => setNetwork(event.target.value)}
                spellCheck={false}
              />
            </Field>

            <p className="mt-2 text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
              {t("enterprise.oidc.groupBrowser")}
            </p>
            <Field label={t("enterprise.oidc.clientId")} hint={t("enterprise.oidc.clientIdHint")}>
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={clientId}
                onChange={(event) => setClientId(event.target.value)}
                placeholder="hpa-browser"
              />
            </Field>
            <Field
              label={t("enterprise.oidc.authorizationEndpoint")}
              hint={t("enterprise.oidc.endpointHint")}
            >
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={authorizationEndpoint}
                onChange={(event) => setAuthorizationEndpoint(event.target.value)}
                placeholder="https://issuer.example.com/authorize"
              />
            </Field>
            <Field
              label={t("enterprise.oidc.tokenEndpoint")}
              hint={t("enterprise.oidc.endpointHint")}
            >
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={tokenEndpoint}
                onChange={(event) => setTokenEndpoint(event.target.value)}
                placeholder="https://issuer.example.com/token"
              />
            </Field>
            <Field
              label={t("enterprise.oidc.redirectUri")}
              hint={t("enterprise.oidc.redirectUriHint")}
            >
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={redirectUri}
                onChange={(event) => setRedirectUri(event.target.value)}
                placeholder={`${window.location.origin}/auth/oidc/callback`}
              />
            </Field>
            <Field
              label={t("enterprise.oidc.endSessionEndpoint")}
              hint={t("enterprise.oidc.endSessionEndpointHint")}
            >
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={endSessionEndpoint}
                onChange={(event) => setEndSessionEndpoint(event.target.value)}
                placeholder="https://issuer.example.com/logout"
              />
            </Field>
            <Field
              label={t("enterprise.oidc.postLogoutRedirectUri")}
              hint={t("enterprise.oidc.postLogoutRedirectUriHint")}
            >
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={postLogoutRedirectUri}
                onChange={(event) => setPostLogoutRedirectUri(event.target.value)}
                placeholder={`${window.location.origin}/auth/oidc/logout/callback`}
              />
            </Field>
            <Field label={t("enterprise.oidc.scopes")} hint={t("enterprise.oidc.scopesHint")}>
              <textarea
                className={cn(textareaClass, "min-h-[58px]")}
                value={scopes}
                onChange={(event) => setScopes(event.target.value)}
                spellCheck={false}
                placeholder={"openid\nemail\nprofile"}
              />
            </Field>
            <Field
              label={t("enterprise.oidc.adminSubjects")}
              hint={t("enterprise.oidc.adminSubjectsHint")}
            >
              <textarea
                className={cn(textareaClass, "min-h-[70px] font-mono text-[11px]")}
                value={adminSubjects}
                onChange={(event) => setAdminSubjects(event.target.value)}
                spellCheck={false}
                placeholder="oidc|admin&#10;admin@example.com"
              />
            </Field>
            <div className="grid gap-2 sm:grid-cols-2">
              <Field label={t("enterprise.oidc.sessionTtl")} hint={t("enterprise.oidc.sessionTtlHint")}>
                <input
                  className={cn(inputClass, "font-mono text-[11px]")}
                  type="number"
                  min={300}
                  max={604800}
                  value={sessionTtl}
                  onChange={(event) => setSessionTtl(event.target.value)}
                />
              </Field>
              <Field label={t("enterprise.oidc.cookieSecure")} hint={t("enterprise.oidc.cookieSecureHint")}>
                <span className="flex h-7 items-center gap-2 text-[11.5px] text-muted">
                  <input
                    type="checkbox"
                    className="h-3.5 w-3.5 accent-[var(--hpa-accent)]"
                    checked={cookieSecure}
                    onChange={(event) => setCookieSecure(event.target.checked)}
                  />
                  {cookieSecure ? t("enterprise.oidc.on") : t("enterprise.oidc.off")}
                </span>
              </Field>
            </div>
            {!ttlValid ? (
              <p className="text-[10.5px] text-danger">
                {t("enterprise.oidc.sessionTtlInvalid")}
              </p>
            ) : null}

            <Button
              variant="primary"
              size="sm"
              className="mt-1 w-fit"
              loading={saveOidc.isPending}
              disabled={!ttlValid}
              onClick={() => saveOidc.mutate()}
            >
              {t("common.save")}
            </Button>
          </div>
        )}
      </Panel>

      <div className="grid min-w-0 content-start gap-3">
        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <LogIn className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.oidc.ssoTitle")}</h2>
          </div>
          <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.oidc.ssoText")}</p>
          {authenticated && principal ? (
            <div className="mt-2 rounded-sm border border-line bg-panel-2 px-2.5 py-2">
              <p className="truncate text-[11.5px] font-semibold text-ink">
                {principal.displayName || principal.email || principal.subject}
              </p>
              <p className="truncate font-mono text-[10px] text-faint">
                {principal.tenantId} · {principal.role}
              </p>
            </div>
          ) : null}
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            {authenticated ? (
              <Button variant="secondary" size="sm" onClick={() => void signOut()}>
                <LogOut className="h-3.5 w-3.5" />
                {t("auth.logout")}
              </Button>
            ) : (
              <Link
                to="/login"
                className="inline-flex h-6 items-center gap-1.5 rounded-sm border border-accent bg-accent px-2 text-[11.5px] font-medium text-accent-ink"
              >
                <LogIn className="h-3.5 w-3.5" />
                {t("auth.login")}
              </Link>
            )}
            <Link
              to="/login"
              className="inline-flex h-6 items-center gap-1.5 rounded-sm border border-line px-2 text-[11.5px] text-muted hover:text-ink"
            >
              {t("enterprise.oidc.ssoOpen")}
            </Link>
          </div>
          <p className="mt-2 rounded-sm border border-warning/30 bg-warning/5 px-2 py-1.5 text-[10.5px] text-warning">
            {t("enterprise.oidc.ssoNote")}
          </p>
        </Panel>

        <Panel className="p-4">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.oidc.verifyTitle")}</h2>
          </div>
          <p className="mt-2 text-[11.5px] text-muted">{t("enterprise.oidc.verifyNote")}</p>
        </Panel>
      </div>
    </div>
  );
}

function StatCard({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="rounded-sm border border-line bg-panel px-3 py-2">
      <p className="text-[9px] uppercase tracking-[0.14em] text-faint">{label}</p>
      <p className="mt-0.5 font-mono text-sm text-ink">{value}</p>
    </div>
  );
}

function MiniStat({
  label,
  value,
  stamp,
}: {
  label: string;
  value: string;
  stamp?: string;
}) {
  return (
    <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-1.5">
      <dt className="text-[8.5px] uppercase tracking-[0.12em] text-faint">{label}</dt>
      <dd className="mt-0.5 truncate text-[11px] text-ink">
        {stamp ? <Timestamp value={stamp} /> : value}
      </dd>
    </div>
  );
}
