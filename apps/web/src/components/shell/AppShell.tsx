import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import {
  Activity,
  BadgeCheck,
  Bot,
  Boxes,
  Building2,
  Cable,
  LayoutDashboard,
  LogIn,
  LogOut,
  Moon,
  Network,
  Plug,
  Puzzle,
  Settings,
  SquarePen,
  Sun,
  Waves,
  Workflow,
} from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../lib/api";
import { currentLocale, locales, setLocale, type Locale } from "../../lib/i18n";
import { useAuth } from "../../lib/session";
import { useEditorStore } from "../../stores/editor";
import { useUiStore, type Theme } from "../../stores/ui";
import { ToastViewport, cn } from "../ui";
import styles from "./AppShell.module.css";

interface NavEntry {
  path: string;
  labelKey: string;
  icon: ReactNode;
  exact?: boolean;
  counter?: number;
}

interface NavGroup {
  labelKey: string;
  items: NavEntry[];
}

export function AppShell() {
  const { t } = useTranslation();
  const health = useQuery({
    queryKey: ["health"],
    queryFn: api.health,
    refetchInterval: 10_000,
    retry: 1,
  });
  const approvals = useQuery({
    queryKey: ["approvals"],
    queryFn: api.listApprovals,
    refetchInterval: 5_000,
    retry: 1,
  });
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const theme = useUiStore((state) => state.theme);
  const setTheme = useUiStore((state) => state.setTheme);
  const designerName = useEditorStore((state) => state.ir?.name);
  const { authenticated, principal, loading, logout } = useAuth();
  const queryClient = useQueryClient();

  async function signOut() {
    await logout();
    queryClient.clear();
  }

  const sessionLabel = principal
    ? principal.displayName || principal.email || principal.subject
    : "";
  const sessionInitial = sessionLabel ? Array.from(sessionLabel)[0]?.toUpperCase() ?? "?" : "";

  const productName = health.data?.brand?.productName ?? t("app.nameFallback");
  const runtimeLabel = health.isLoading
    ? t("shell.checking")
    : health.isError
      ? t("shell.offline")
      : t("shell.online");

  const navGroups: NavGroup[] = [
    {
      labelKey: "nav.groupWorkspace",
      items: [
        {
          path: "/",
          labelKey: "nav.dashboard",
          icon: <LayoutDashboard className={styles.navIcon} />,
          exact: true,
        },
        {
          path: "/workflows",
          labelKey: "nav.workflows",
          icon: <Workflow className={styles.navIcon} />,
        },
        {
          path: "/designer",
          labelKey: "nav.designer",
          icon: <SquarePen className={styles.navIcon} />,
        },
        {
          path: "/agents",
          labelKey: "nav.agents",
          icon: <Bot className={styles.navIcon} />,
        },
        {
          path: "/skills",
          labelKey: "nav.skills",
          icon: <Puzzle className={styles.navIcon} />,
        },
        {
          path: "/plugins",
          labelKey: "nav.plugins",
          icon: <Plug className={styles.navIcon} />,
        },
        {
          path: "/connectors",
          labelKey: "nav.connectors",
          icon: <Cable className={styles.navIcon} />,
        },
        {
          path: "/mcp-center",
          labelKey: "nav.mcp",
          icon: <Network className={styles.navIcon} />,
        },
      ],
    },
    {
      labelKey: "nav.groupOperations",
      items: [
        {
          path: "/executions",
          labelKey: "nav.executions",
          icon: <Activity className={styles.navIcon} />,
        },
        {
          path: "/approvals",
          labelKey: "nav.approvals",
          icon: <BadgeCheck className={styles.navIcon} />,
          counter: approvals.data?.length ?? 0,
        },
        {
          path: "/artifacts",
          labelKey: "nav.artifacts",
          icon: <Boxes className={styles.navIcon} />,
        },
      ],
    },
    {
      labelKey: "nav.groupSystem",
      items: [
        {
          path: "/settings",
          labelKey: "nav.settings",
          icon: <Settings className={styles.navIcon} />,
        },
        {
          path: "/enterprise",
          labelKey: "nav.enterprise",
          icon: <Building2 className={styles.navIcon} />,
        },
      ],
    },
  ];

  const title = pageTitle(pathname, t, designerName);

  return (
    <div className={styles.shell}>
      <aside className={styles.sidebar}>
        <Link to="/" className={styles.brand} aria-label={productName}>
          <span className={styles.brandMark} aria-hidden="true">
            h
          </span>
          <span className={styles.brandText}>
            <span className={styles.brandName}>{productName}</span>
            <span className={styles.brandMeta}>{health.data?.internalId ?? "hpa"} · local</span>
          </span>
        </Link>
        <nav className={styles.nav} aria-label={t("shell.navigation")}>
          {navGroups.map((group) => (
            <div key={group.labelKey} className={styles.navGroup}>
              <div className={styles.navSectionLabel}>{t(group.labelKey)}</div>
              {group.items.map((entry) => (
                <Link
                  key={entry.path}
                  to={entry.path}
                  className={styles.navItem}
                  activeProps={{ className: cn(styles.navItem, styles.navItemActive) }}
                  activeOptions={{ exact: entry.exact ?? false }}
                >
                  {entry.icon}
                  <span>{t(entry.labelKey)}</span>
                  {entry.counter ? <span className={styles.navCounter}>{entry.counter}</span> : null}
                </Link>
              ))}
            </div>
          ))}
        </nav>
        <div className={styles.sidebarFooter}>
          {authenticated && principal ? (
            <div className={styles.sessionBox}>
              <span className={styles.sessionAvatar} aria-hidden="true">
                {sessionInitial}
              </span>
              <span className={styles.sessionText}>
                <span className={styles.sessionName}>{sessionLabel}</span>
                <span className={styles.sessionMeta}>
                  {principal.tenantId} · {t(`enterprise.roles.${principal.role}`, {
                    defaultValue: principal.role,
                  })}
                  {principal.platformAdmin ? ` · ${t("auth.platformAdmin")}` : ""}
                </span>
              </span>
              <button
                type="button"
                className={styles.sessionAction}
                onClick={() => void signOut()}
                title={t("auth.logout")}
                aria-label={t("auth.logout")}
              >
                <LogOut className="h-3.5 w-3.5" />
              </button>
            </div>
          ) : (
            <Link to="/login" className={styles.sessionLogin}>
              <LogIn className="h-3.5 w-3.5" />
              <span>{t("auth.login")}</span>
              {loading ? (
                <span className={styles.sessionMeta}>{t("auth.checking")}</span>
              ) : null}
            </Link>
          )}
          <div className={styles.runtimePill} title={runtimeLabel}>
            <span
              className={cn(
                styles.statusDot,
                health.isSuccess && styles.statusDotOnline,
                health.isError && styles.statusDotOffline,
              )}
            />
            <span className={styles.runtimeLabel}>{t("shell.runtime")}</span>
            <span className="ml-auto text-[10px] text-faint">{runtimeLabel}</span>
          </div>
        </div>
      </aside>
      <div className={styles.main}>
        <header className={styles.topbar}>
          <div className={styles.topbarBreadcrumb}>
            <span className={styles.topbarRoot}>{health.data?.internalId ?? "hpa"}</span>
            <span className={styles.topbarSlash}>/</span>
            <span className={styles.topbarTitle}>{title}</span>
          </div>
          <div className={styles.topbarRight}>
            {authenticated && principal ? (
              <div className={styles.topbarSession}>
                <span className={styles.topbarAvatar} aria-hidden="true">
                  {sessionInitial}
                </span>
                <span className={styles.topbarSessionName} title={sessionLabel}>
                  {sessionLabel}
                </span>
                <button
                  type="button"
                  className={styles.topbarIcon}
                  onClick={() => void signOut()}
                  title={t("auth.logout")}
                  aria-label={t("auth.logout")}
                >
                  <LogOut className="h-3.5 w-3.5" />
                </button>
              </div>
            ) : (
              <Link
                to="/login"
                className={styles.topbarLogin}
                title={t("auth.login")}
                aria-label={t("auth.login")}
              >
                <LogIn className="h-3.5 w-3.5" />
                <span className={styles.topbarLoginLabel}>{t("auth.login")}</span>
              </Link>
            )}
            <span className={styles.topbarRuntime}>
              <span
                className={cn(
                  styles.statusDot,
                  health.isSuccess && styles.statusDotOnline,
                  health.isError && styles.statusDotOffline,
                )}
              />
              {runtimeLabel}
            </span>
            <button
              type="button"
              className={styles.topbarIcon}
              onClick={() => setTheme(nextTheme(theme))}
              title={t("settings.theme")}
              aria-label={t("settings.theme")}
            >
              {theme === "light" ? (
                <Sun className="h-3.5 w-3.5" />
              ) : theme === "ocean" ? (
                <Waves className="h-3.5 w-3.5" />
              ) : (
                <Moon className="h-3.5 w-3.5" />
              )}
            </button>
            <select
              className={styles.topbarLang}
              value={currentLocale()}
              onChange={(event) => setLocale(event.target.value as Locale)}
              aria-label={t("shell.language")}
            >
              {locales.map((locale) => (
                <option key={locale} value={locale}>
                  {locale === "ja" ? "日本語" : locale === "en" ? "English" : "中文"}
                </option>
              ))}
            </select>
          </div>
        </header>
        <main className={cn(styles.content, pathname === "/designer" && styles.contentFull)}>
          <Outlet />
        </main>
      </div>
      <ToastViewport />
    </div>
  );
}

function pageTitle(
  pathname: string,
  t: (key: string) => string,
  designerName?: string,
): string {
  if (pathname === "/") return t("nav.dashboard");
  if (pathname.startsWith("/workflows")) return t("nav.workflows");
  if (pathname.startsWith("/designer")) return designerName ?? t("nav.designer");
  if (pathname.startsWith("/agents")) return t("nav.agents");
  if (pathname.startsWith("/skills")) return t("nav.skills");
  if (pathname.startsWith("/plugins")) return t("nav.plugins");
  if (pathname.startsWith("/connectors")) return t("nav.connectors");
  if (pathname.startsWith("/mcp-center")) return t("nav.mcp");
  if (pathname.startsWith("/executions/")) return t("executions.detailTitle");
  if (pathname.startsWith("/executions")) return t("nav.executions");
  if (pathname.startsWith("/approvals")) return t("nav.approvals");
  if (pathname.startsWith("/artifacts")) return t("nav.artifacts");
  if (pathname.startsWith("/enterprise")) return t("nav.enterprise");
  if (pathname.startsWith("/settings")) return t("nav.settings");
  if (pathname.startsWith("/login")) return t("auth.loginTitle");
  return t("nav.dashboard");
}

function nextTheme(theme: Theme): Theme {
  if (theme === "light") return "dark";
  if (theme === "dark") return "ocean";
  return "light";
}
