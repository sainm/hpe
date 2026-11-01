import { Link, useRouterState } from "@tanstack/react-router";
import { ArrowLeft, Building2, KeyRound, LogIn, ShieldCheck } from "lucide-react";
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Badge, Button, Panel } from "../components/ui";
import { useAuth } from "../lib/session";

const loginErrors = [
  "sso_unavailable",
  "state_invalid",
  "member_disabled",
  "access_denied",
] as const;

type LoginError = (typeof loginErrors)[number];

function isLoginError(value: string | null): value is LoginError {
  return Boolean(value) && (loginErrors as readonly string[]).includes(value as string);
}

export function safeRedirect(value: string | null | undefined): string {
  if (!value) return "/";
  if (!value.startsWith("/") || value.startsWith("//")) return "/";
  if (/[\u0000-\u001f\u007f]/.test(value)) return "/";
  if (value.startsWith("/login")) return "/";
  return value;
}

export function LoginPage() {
  const { t } = useTranslation();
  const { authenticated, principal, loading, ssoConfigured, logout } = useAuth();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const searchStr = useRouterState({ select: (state) => state.location.searchStr });
  const params = new URLSearchParams(searchStr.startsWith("?") ? searchStr.slice(1) : searchStr);
  const errorCode = params.get("error");
  const error: LoginError | null = isLoginError(errorCode) ? errorCode : null;
  const hasUnknownError = Boolean(errorCode) && !error;
  const redirect = safeRedirect(
    params.get("redirect") ?? (pathname === "/login" ? "/" : `${pathname}${searchStr}`),
  );

  useEffect(() => {
    if (authenticated) {
      window.location.replace(redirect);
    }
  }, [authenticated, redirect]);

  function startSso() {
    window.location.assign(`/auth/oidc/login?redirect=${encodeURIComponent(redirect)}`);
  }

  async function signOut() {
    await logout();
  }

  return (
    <div className="mx-auto grid w-full min-w-0 max-w-[460px] gap-3">
      <div className="flex items-center gap-2">
        <Link
          to="/"
          className="inline-flex items-center gap-1.5 text-[11.5px] text-muted hover:text-ink"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {t("auth.loginBack")}
        </Link>
        <Badge tone="neutral" className="ml-auto">
          {t("auth.sessionBadge")}
        </Badge>
      </div>

      <Panel className="p-5">
        <div className="flex items-center gap-2">
          <div className="grid h-8 w-8 place-items-center rounded-full bg-accent-soft text-accent-text">
            <ShieldCheck className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h1 className="text-[15px] font-semibold text-ink-strong">{t("auth.loginTitle")}</h1>
            <p className="text-[11px] text-faint">{t("auth.loginSubtitle")}</p>
          </div>
        </div>

        {error || hasUnknownError ? (
          <div
            role="alert"
            className="mt-3 rounded-sm border border-danger/30 bg-danger/5 px-2.5 py-2"
          >
            <p className="text-[11.5px] font-semibold text-danger">
              {t("auth.loginErrorTitle")}
            </p>
            <p className="mt-0.5 text-[11px] text-muted">
              {error ? t(`auth.error.${error}`) : t("auth.error.unknown")}
            </p>
            {errorCode ? (
              <p className="mt-1 font-mono text-[10px] text-faint">{errorCode}</p>
            ) : null}
          </div>
        ) : null}

        {authenticated && principal ? (
          <div className="mt-3 grid gap-2">
            <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-2">
              <p className="truncate text-[12px] font-semibold text-ink">
                {principal.displayName || principal.email || principal.subject}
              </p>
              {principal.email ? (
                <p className="truncate font-mono text-[10.5px] text-faint">{principal.email}</p>
              ) : null}
              <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[10.5px] text-muted">
                <span>{t("auth.tenant")}: {principal.tenantId}</span>
                <span aria-hidden="true">·</span>
                <span>{t("auth.role")}: {t(`enterprise.roles.${principal.role}`)}</span>
                {principal.platformAdmin ? (
                  <Badge tone="accent">{t("auth.platformAdmin")}</Badge>
                ) : null}
              </div>
            </div>
            <p className="text-[11px] text-faint">{t("auth.loginSignedIn")}</p>
            <div className="flex flex-wrap items-center gap-1.5">
              <Button variant="primary" size="sm" onClick={() => window.location.replace(redirect)}>
                {t("auth.loginContinue")}
              </Button>
              <Button variant="secondary" size="sm" onClick={() => void signOut()}>
                {t("auth.logout")}
              </Button>
            </div>
          </div>
        ) : (
          <div className="mt-4 grid gap-3">
            {ssoConfigured ? (
              <Button
                variant="primary"
                size="md"
                className="w-full"
                disabled={loading}
                onClick={startSso}
              >
                <LogIn className="h-3.5 w-3.5" />
                {t("auth.loginSso")}
              </Button>
            ) : (
              <div className="rounded-sm border border-warning/30 bg-warning/5 px-2.5 py-2">
                <p className="text-[11.5px] font-semibold text-warning">
                  {t("auth.loginNotConfigured")}
                </p>
                <p className="mt-1 text-[11px] text-muted">{t("auth.loginNotConfiguredHint")}</p>
                <Link
                  to="/enterprise"
                  className="mt-1.5 inline-flex items-center gap-1.5 text-[11px] text-accent hover:underline"
                >
                  <Building2 className="h-3.5 w-3.5" />
                  {t("auth.loginConfigureLink")}
                </Link>
              </div>
            )}

            <div className="rounded-sm border border-line bg-panel-2 px-2.5 py-2">
              <div className="flex items-center gap-1.5">
                <KeyRound className="h-3.5 w-3.5 text-faint" />
                <p className="text-[11px] font-semibold text-muted">{t("auth.loginLocalTitle")}</p>
              </div>
              <p className="mt-1 text-[11px] text-faint">{t("auth.loginLocalText")}</p>
            </div>
          </div>
        )}
      </Panel>

      <p className="text-center text-[10.5px] text-faint">
        {t("auth.loginFooter")}
      </p>
    </div>
  );
}
