import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Ban,
  KeyRound,
  Plus,
  RefreshCw,
  RotateCw,
  ShieldCheck,
  Trash2,
  Webhook,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  CopyButton,
  Field,
  Panel,
  Timestamp,
  cn,
  inputClass,
} from "../components/ui";
import { api } from "../lib/api";
import type { OauthClient, OauthTokenSummary } from "../lib/types";
import { useUiStore } from "../stores/ui";

export function OAuthSection({ enabled }: { enabled: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const pushToast = useUiStore((state) => state.pushToast);
  const [name, setName] = useState("");
  const [redirects, setRedirects] = useState("");
  const [authMethod, setAuthMethod] = useState("none");
  const [scope, setScope] = useState("mcp:read mcp:execute");
  const [secret, setSecret] = useState<{ clientId: string; value: string } | null>(null);

  const overviewQuery = useQuery({
    queryKey: ["enterprise", "oauth", "overview"],
    queryFn: api.enterpriseOauth,
    enabled,
    retry: false,
  });
  const tokensQuery = useQuery({
    queryKey: ["enterprise", "oauth", "tokens"],
    queryFn: api.enterpriseOauthTokens,
    enabled,
    retry: false,
  });

  const createClient = useMutation({
    mutationFn: () =>
      api.enterpriseOauthCreateClient({
        name: name.trim(),
        redirect_uris: splitLines(redirects),
        token_endpoint_auth_method: authMethod,
        scope: scope.trim(),
      }),
    onSuccess: (client) => {
      if (client.clientSecret) {
        setSecret({ clientId: client.id, value: client.clientSecret });
      }
      setName("");
      setRedirects("");
      pushToast(t("enterprise.oauth.created"), "success");
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const disableClient = useMutation({
    mutationFn: (id: string) => api.enterpriseOauthDisableClient(id),
    onSuccess: () => {
      pushToast(t("enterprise.oauth.disabled"), "warning");
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const rotateSecret = useMutation({
    mutationFn: (id: string) => api.enterpriseOauthRotateSecret(id),
    onSuccess: (result) => {
      setSecret({ clientId: result.clientId, value: result.clientSecret });
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const revokeToken = useMutation({
    mutationFn: (id: string) => api.enterpriseOauthRevokeToken(id),
    onSuccess: () => {
      pushToast(t("enterprise.oauth.revoked"), "warning");
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  const rotateKey = useMutation({
    mutationFn: () => api.enterpriseOauthRotateKey(),
    onSuccess: (result) => {
      pushToast(t("enterprise.oauth.keyRotated", { kid: result.kid }), "success");
      refresh();
    },
    onError: (error) => pushToast(messageOf(error), "error"),
  });

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["enterprise", "oauth"] });
  }

  const overview = overviewQuery.data;
  const clients = overview?.clients ?? [];
  const tokens = tokensQuery.data?.tokens ?? [];
  const activeTokens = tokens.filter(
    (token) => token.revokedAt === null && token.expiresAt > Date.now(),
  );

  return (
    <div className="grid min-w-0 gap-3 xl:grid-cols-[minmax(0,1fr)_340px]">
      <div className="grid min-w-0 gap-3">
        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.oauth.title")}</h2>
            {overview ? <Badge tone="accent">{overview.signingAlg}</Badge> : null}
            <div className="ml-auto flex items-center gap-1.5">
              <Button
                size="xs"
                variant="ghost"
                disabled={rotateKey.isPending}
                onClick={() => rotateKey.mutate()}
              >
                <RotateCw className="h-3 w-3" />
                {t("enterprise.oauth.rotateKey")}
              </Button>
              <Button size="xs" variant="ghost" onClick={refresh}>
                <RefreshCw className="h-3 w-3" />
                {t("common.refresh")}
              </Button>
            </div>
          </div>
          <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.oauth.subtitle")}</p>
          {overviewQuery.isLoading ? (
            <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
          ) : overviewQuery.isError ? (
            <p className="mt-2 text-xs text-danger">{messageOf(overviewQuery.error)}</p>
          ) : overview ? (
            <div className="mt-2 grid gap-1">
              <EndpointRow label={t("enterprise.oauth.issuer")} value={overview.issuer} />
              <EndpointRow label={t("enterprise.oauth.resource")} value={overview.resource} />
              <EndpointRow label={t("enterprise.oauth.jwks")} value={overview.jwksUri} />
              <EndpointRow
                label={t("enterprise.oauth.registerEndpoint")}
                value={overview.registrationEndpoint}
              />
              <div className="mt-1 flex flex-wrap items-center gap-1.5">
                {overview.scopes.map((entry) => (
                  <Badge key={entry} tone="neutral">
                    {entry}
                  </Badge>
                ))}
                <Badge tone="neutral">
                  {t("enterprise.oauth.counts", {
                    clients: overview.clientCount,
                    tokens: overview.activeTokenCount,
                  })}
                </Badge>
              </div>
            </div>
          ) : null}
        </Panel>

        <Panel className="p-4">
          <div className="flex flex-wrap items-center gap-2">
            <Webhook className="h-4 w-4 text-accent" />
            <h2 className="text-sm font-semibold text-ink">{t("enterprise.oauth.clientsTitle")}</h2>
            <Badge tone="neutral" className="ml-auto">
              {clients.length}
            </Badge>
          </div>
          <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.oauth.clientsHint")}</p>
          {clients.length === 0 ? (
            <p className="mt-3 text-[11.5px] text-muted">{t("enterprise.oauth.clientsEmpty")}</p>
          ) : (
            <div className="mt-2 grid gap-1.5">
              {clients.map((client) => (
                <ClientRow
                  key={client.id}
                  client={client}
                  onDisable={() => disableClient.mutate(client.id)}
                  onRotate={() => rotateSecret.mutate(client.id)}
                  busy={disableClient.isPending || rotateSecret.isPending}
                />
              ))}
            </div>
          )}

          <details className="mt-4 border-t border-line pt-3">
            <summary className="cursor-pointer text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
              {t("enterprise.oauth.createTitle")}
            </summary>
            <div className="mt-2 grid gap-2 sm:grid-cols-2">
              <Field label={t("enterprise.oauth.clientName")}>
                <input
                  className={cn(inputClass, "text-[11px]")}
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="Claude Code"
                />
              </Field>
              <Field label={t("enterprise.oauth.authMethod")}>
                <select
                  className={cn(inputClass, "text-[11px]")}
                  value={authMethod}
                  onChange={(event) => setAuthMethod(event.target.value)}
                >
                  <option value="none">{t("enterprise.oauth.methodNone")}</option>
                  <option value="client_secret_post">{t("enterprise.oauth.methodPost")}</option>
                  <option value="client_secret_basic">{t("enterprise.oauth.methodBasic")}</option>
                </select>
              </Field>
            </div>
            <Field
              label={t("enterprise.oauth.redirectUris")}
              hint={t("enterprise.oauth.redirectHint")}
              className="mt-2"
            >
              <textarea
                className={cn(inputClass, "min-h-[54px] font-mono text-[11px]")}
                value={redirects}
                onChange={(event) => setRedirects(event.target.value)}
                spellCheck={false}
                placeholder="http://127.0.0.1:6277/callback"
              />
            </Field>
            <Field label={t("enterprise.oauth.scope")} className="mt-2">
              <input
                className={cn(inputClass, "font-mono text-[11px]")}
                value={scope}
                onChange={(event) => setScope(event.target.value)}
              />
            </Field>
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              <Button
                size="sm"
                variant="primary"
                disabled={createClient.isPending || !name.trim() || splitLines(redirects).length === 0}
                onClick={() => createClient.mutate()}
              >
                <Plus className="h-3 w-3" />
                {t("enterprise.oauth.create")}
              </Button>
              <p className="text-[10.5px] text-faint">{t("enterprise.oauth.dcrHint")}</p>
            </div>
          </details>

          {secret ? (
            <div className="mt-3 rounded-sm border border-warning/30 bg-warning/5 px-2.5 py-2">
              <p className="text-[11.5px] font-semibold text-warning">
                {t("enterprise.oauth.secretTitle")}
              </p>
              <p className="mt-0.5 text-[10.5px] text-muted">
                {t("enterprise.oauth.secretHint", { client: secret.clientId })}
              </p>
              <div className="mt-1.5 flex items-center gap-1.5">
                <code className="min-w-0 flex-1 truncate rounded-sm border border-line bg-panel px-1.5 py-1 font-mono text-[10.5px] text-ink">
                  {secret.value}
                </code>
                <CopyButton text={secret.value} label={t("common.copy")} />
                <Button size="xs" variant="ghost" onClick={() => setSecret(null)}>
                  {t("common.close")}
                </Button>
              </div>
            </div>
          ) : null}
        </Panel>
      </div>

      <Panel className="min-w-0 p-4">
        <div className="flex flex-wrap items-center gap-2">
          <KeyRound className="h-4 w-4 text-accent" />
          <h2 className="text-sm font-semibold text-ink">{t("enterprise.oauth.tokensTitle")}</h2>
          <Badge tone="neutral" className="ml-auto">
            {activeTokens.length}
          </Badge>
        </div>
        <p className="mt-1 text-[10.5px] text-faint">{t("enterprise.oauth.tokensHint")}</p>
        {tokensQuery.isLoading ? (
          <p className="mt-2 text-xs text-muted">{t("common.loading")}</p>
        ) : tokens.length === 0 ? (
          <p className="mt-3 text-[11.5px] text-muted">{t("enterprise.oauth.tokensEmpty")}</p>
        ) : (
          <ul className="mt-2 grid gap-1.5">
            {tokens.slice(0, 24).map((token) => (
              <TokenRow
                key={token.id}
                token={token}
                onRevoke={() => revokeToken.mutate(token.id)}
                busy={revokeToken.isPending}
              />
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function ClientRow({
  client,
  onDisable,
  onRotate,
  busy,
}: {
  client: OauthClient;
  onDisable: () => void;
  onRotate: () => void;
  busy: boolean;
}) {
  const { t } = useTranslation();
  const disabled = client.disabledAt !== null;
  return (
    <div className="min-w-0 rounded-sm border border-line bg-panel-2 px-2.5 py-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <p className="min-w-0 truncate text-[12px] font-semibold text-ink">{client.name}</p>
        <Badge tone={disabled ? "danger" : "success"} dot>
          {disabled ? t("enterprise.oauth.statusDisabled") : t("enterprise.oauth.statusActive")}
        </Badge>
        <Badge tone="neutral">{client.tokenEndpointAuthMethod}</Badge>
        <Badge tone="neutral">{client.origin}</Badge>
        <div className="ml-auto flex items-center gap-1">
          {client.hasSecret && !disabled ? (
            <Button size="xs" variant="ghost" disabled={busy} onClick={onRotate}>
              <RotateCw className="h-3 w-3" />
              {t("enterprise.oauth.rotateSecret")}
            </Button>
          ) : null}
          {!disabled ? (
            <Button size="xs" variant="ghost" disabled={busy} onClick={onDisable}>
              <Ban className="h-3 w-3" />
              {t("enterprise.oauth.disable")}
            </Button>
          ) : null}
        </div>
      </div>
      <p className="mt-0.5 truncate font-mono text-[10px] text-faint">{client.id}</p>
      <p className="mt-0.5 truncate font-mono text-[10px] text-muted">
        {client.redirectUris.join(" · ")}
      </p>
      <div className="mt-1 flex flex-wrap items-center gap-1.5">
        {client.scopes.map((entry) => (
          <span
            key={entry}
            className="rounded-sm border border-line bg-panel px-1 py-0.5 font-mono text-[9.5px] text-faint"
          >
            {entry}
          </span>
        ))}
        {client.lastUsedAt ? (
          <span className="ml-auto text-[9.5px] text-faint">
            {t("enterprise.oauth.lastUsed")} <Timestamp value={new Date(client.lastUsedAt).toISOString()} />
          </span>
        ) : null}
      </div>
    </div>
  );
}

function TokenRow({
  token,
  onRevoke,
  busy,
}: {
  token: OauthTokenSummary;
  onRevoke: () => void;
  busy: boolean;
}) {
  const { t } = useTranslation();
  const revoked = token.revokedAt !== null;
  const expired = token.expiresAt <= Date.now();
  const inactive = revoked || expired;
  return (
    <li className="min-w-0 rounded-sm border border-line bg-panel-2 px-2 py-1.5">
      <div className="flex min-w-0 items-center gap-1.5">
        <Badge tone={token.kind === "access" ? "accent" : "neutral"}>
          {token.kind === "access"
            ? t("enterprise.oauth.kindAccess")
            : t("enterprise.oauth.kindRefresh")}
        </Badge>
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted">
          {token.clientId}
        </span>
        {inactive ? (
          <Badge tone="neutral" className="shrink-0">
            {revoked ? t("enterprise.oauth.statusRevoked") : t("enterprise.oauth.statusExpired")}
          </Badge>
        ) : (
          <Button
            size="xs"
            variant="ghost"
            className="shrink-0"
            title={t("enterprise.oauth.revoke")}
            aria-label={t("enterprise.oauth.revoke")}
            disabled={busy}
            onClick={onRevoke}
          >
            <Trash2 className="h-3 w-3" />
          </Button>
        )}
      </div>
      <div className="mt-0.5 flex min-w-0 flex-wrap items-center gap-1.5 text-[9.5px] text-faint">
        <span className="min-w-0 flex-1 truncate font-mono">{token.scope}</span>
        <span className="shrink-0">
          <Timestamp value={new Date(token.expiresAt).toISOString()} />
        </span>
      </div>
    </li>
  );
}

function EndpointRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <span className="w-[74px] shrink-0 text-[10px] uppercase tracking-[0.08em] text-faint">
        {label}
      </span>
      <code className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-muted">{value}</code>
      <CopyButton text={value} />
    </div>
  );
}

function splitLines(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
