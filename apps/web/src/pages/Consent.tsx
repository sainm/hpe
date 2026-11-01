import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { ArrowLeft, KeyRound, ShieldCheck, ShieldX } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Panel, cn } from "../components/ui";
import { api } from "../lib/api";
import type { OauthConsentRequest } from "../lib/types";

const scopeKeys: Record<string, string> = {
  "mcp:read": "auth.consent.scopeRead",
  "mcp:execute": "auth.consent.scopeExecute",
  offline_access: "auth.consent.scopeOffline",
};

export function ConsentPage() {
  const { t } = useTranslation();
  const searchStr = useRouterState({ select: (state) => state.location.searchStr });
  const params = new URLSearchParams(searchStr.startsWith("?") ? searchStr.slice(1) : searchStr);
  const requestId = params.get("request") ?? "";
  const [remember, setRemember] = useState(true);
  const [pending, setPending] = useState<"allow" | "deny" | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);

  const query = useQuery({
    queryKey: ["oauth", "request", requestId],
    queryFn: () => api.oauthConsentRequest(requestId),
    enabled: Boolean(requestId),
    retry: false,
    refetchOnWindowFocus: false,
  });

  useEffect(() => {
    if (!requestId) setFailure(t("auth.consent.invalid"));
  }, [requestId, t]);

  async function decide(decision: "allow" | "deny") {
    if (!requestId || pending) return;
    setPending(decision);
    setFailure(null);
    try {
      const result = await api.oauthConsentDecision(requestId, decision, remember);
      setLeaving(true);
      window.location.replace(result.redirect);
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
      setPending(null);
    }
  }

  if (query.isLoading) {
    return (
      <div className="mx-auto w-full max-w-[520px]">
        <Panel className="p-5">
          <p className="text-xs text-muted">{t("common.loading")}</p>
        </Panel>
      </div>
    );
  }

  if (query.isError || !query.data) {
    return (
      <ConsentFailure
        title={t("auth.consent.invalid")}
        detail={
          query.error instanceof Error ? query.error.message : t("auth.consent.invalidHint")
        }
      />
    );
  }

  const request: OauthConsentRequest = query.data;

  return (
    <div className="mx-auto grid w-full min-w-0 max-w-[520px] gap-3">
      <div className="flex items-center gap-2">
        <a
          href="/"
          className="inline-flex items-center gap-1.5 text-[11.5px] text-muted hover:text-ink"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {t("auth.consent.back")}
        </a>
        <Badge tone="neutral" className="ml-auto">
          {t("auth.consent.badge")}
        </Badge>
      </div>

      <Panel className="p-5">
        <div className="flex items-start gap-2">
          <div className="grid h-8 w-8 shrink-0 place-items-center rounded-full bg-accent-soft text-accent-text">
            <ShieldCheck className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h1 className="text-[15px] font-semibold text-ink-strong">
              {t("auth.consent.title")}
            </h1>
            <p className="text-[11px] text-faint">
              {t("auth.consent.subtitle", { client: request.clientName })}
            </p>
          </div>
        </div>

        <dl className="mt-4 grid gap-2">
          <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-2">
            <dt className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
              {t("auth.consent.client")}
            </dt>
            <dd className="mt-0.5 truncate text-[12px] font-semibold text-ink">
              {request.clientName}
            </dd>
            <dd className="truncate font-mono text-[10.5px] text-faint">{request.clientId}</dd>
          </div>

          <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-2">
            <dt className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
              {t("auth.consent.redirect")}
            </dt>
            <dd className="mt-0.5 truncate text-[12px] text-ink">
              {request.redirectHost || t("auth.consent.unknownHost")}
            </dd>
            <dd className="truncate font-mono text-[10.5px] text-faint">{request.redirectUri}</dd>
          </div>

          <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-2">
            <dt className="text-[10px] font-semibold uppercase tracking-[0.12em] text-faint">
              {t("auth.consent.scopes")}
            </dt>
            <dd className="mt-1 grid gap-1">
              {request.scopes.map((scope) => (
                <div key={scope} className="flex items-start gap-1.5">
                  <KeyRound className="mt-0.5 h-3 w-3 shrink-0 text-accent" />
                  <div className="min-w-0">
                    <p className="text-[11.5px] text-ink">
                      {scopeKeys[scope] ? t(scopeKeys[scope]) : scope}
                    </p>
                    <p className="font-mono text-[10px] text-faint">{scope}</p>
                  </div>
                </div>
              ))}
            </dd>
          </div>
        </dl>

        <div className="mt-3 rounded-sm border border-line bg-panel-2 px-2.5 py-2">
          <p className="text-[11px] text-muted">
            {request.authenticated
              ? t("auth.consent.signedInAs", {
                  subject: request.subject,
                  tenant: request.tenantId,
                })
              : t("auth.consent.localMode")}
          </p>
          {request.resource ? (
            <p className="mt-1 truncate font-mono text-[10px] text-faint">{request.resource}</p>
          ) : null}
        </div>

        <label className="mt-3 flex items-start gap-2 text-[11px] text-muted">
          <input
            type="checkbox"
            className="mt-0.5 h-3.5 w-3.5 accent-[var(--color-accent)]"
            checked={remember}
            onChange={(event) => setRemember(event.target.checked)}
          />
          <span>{t("auth.consent.remember")}</span>
        </label>

        {failure ? (
          <div role="alert" className="mt-3 rounded-sm border border-danger/30 bg-danger/5 px-2.5 py-2">
            <p className="text-[11.5px] font-semibold text-danger">{t("auth.consent.failed")}</p>
            <p className="mt-0.5 break-words text-[11px] text-muted">{failure}</p>
          </div>
        ) : null}

        {leaving ? (
          <p className="mt-3 text-[11px] text-muted">{t("auth.consent.returning")}</p>
        ) : (
          <div className="mt-4 flex flex-wrap items-center gap-2">
            <Button
              variant="primary"
              size="md"
              disabled={pending !== null}
              onClick={() => void decide("allow")}
            >
              <ShieldCheck className="h-3.5 w-3.5" />
              {pending === "allow" ? t("auth.consent.allowing") : t("auth.consent.allow")}
            </Button>
            <Button
              variant="secondary"
              size="md"
              disabled={pending !== null}
              onClick={() => void decide("deny")}
            >
              <ShieldX className="h-3.5 w-3.5" />
              {t("auth.consent.deny")}
            </Button>
          </div>
        )}
      </Panel>

      <p className="text-center text-[10.5px] text-faint">{t("auth.consent.footer")}</p>
    </div>
  );
}

function ConsentFailure({ title, detail }: { title: string; detail: string }) {
  const { t } = useTranslation();
  return (
    <div className="mx-auto w-full max-w-[520px]">
      <Panel className="p-5">
        <div className="flex items-start gap-2">
          <div className="grid h-8 w-8 shrink-0 place-items-center rounded-full bg-danger/10 text-danger">
            <ShieldX className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h1 className="text-[15px] font-semibold text-ink-strong">{title}</h1>
            <p className={cn("mt-0.5 break-words text-[11px] text-muted")}>{detail}</p>
          </div>
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <a href="/">
            <Button variant="secondary" size="sm">
              {t("auth.consent.backHome")}
            </Button>
          </a>
          <a href="/settings">
            <Button variant="ghost" size="sm">
              {t("auth.consent.openSettings")}
            </Button>
          </a>
        </div>
      </Panel>
    </div>
  );
}
