import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  api,
  setCsrfToken,
  setUnauthorizedHandler,
} from "./api";
import type { AuthPrincipal, AuthSession } from "./types";

interface AuthContextValue {
  session: AuthSession | null;
  loading: boolean;
  authenticated: boolean;
  principal: AuthPrincipal | null;
  ssoConfigured: boolean;
  refreshSession: () => Promise<AuthSession | null>;
  logout: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<AuthSession | null>(null);
  const [loading, setLoading] = useState(true);

  const refreshSession = useCallback(async () => {
    try {
      const next = await api.authSession();
      setCsrfToken(next.authenticated ? next.csrfToken : null);
      setSession(next);
      return next;
    } catch {
      setCsrfToken(null);
      setSession(null);
      return null;
    } finally {
      setLoading(false);
    }
  }, []);

  const logout = useCallback(async () => {
    let logoutUrl: string | null | undefined;
    try {
      const result = await api.authLogout();
      logoutUrl = result.logoutUrl;
    } catch {
      setCsrfToken(null);
    } finally {
      setCsrfToken(null);
      setSession((current) =>
        current
          ? {
              ...current,
              authenticated: false,
              principal: undefined,
              csrfToken: null,
              expiresAt: undefined,
            }
          : null,
      );
      if (logoutUrl) window.location.assign(logoutUrl);
    }
  }, []);

  useEffect(() => {
    void refreshSession();
  }, [refreshSession]);

  useEffect(() => {
    setUnauthorizedHandler(() => {
      setCsrfToken(null);
      setSession((current) =>
        current?.authenticated
          ? {
              ...current,
              authenticated: false,
              principal: undefined,
              csrfToken: null,
              expiresAt: undefined,
            }
          : current,
      );
    });
    return () => setUnauthorizedHandler(null);
  }, []);

  useEffect(() => {
    if (!session?.authenticated || !session.refreshAvailable) return;
    const refreshAt = session.accessTokenExpiresAt ?? session.expiresAt;
    if (!refreshAt) return;
    const delay = Math.max(0, Math.min(refreshAt - Date.now() - 60_000, 2_147_483_647));
    const timer = window.setTimeout(() => {
      void api
        .authRefresh()
        .then((next) => {
          setSession((current) =>
            current?.authenticated
              ? {
                  ...current,
                  expiresAt: next.expiresAt,
                  accessTokenExpiresAt: next.accessTokenExpiresAt,
                  refreshAvailable: next.refreshAvailable,
                }
              : current,
          );
        })
        .catch(() => refreshSession());
    }, delay);
    return () => window.clearTimeout(timer);
  }, [session, refreshSession]);

  const value = useMemo<AuthContextValue>(
    () => ({
      session,
      loading,
      authenticated: Boolean(session?.authenticated),
      principal: session?.principal ?? null,
      ssoConfigured: Boolean(session?.ssoConfigured),
      refreshSession,
      logout,
    }),
    [session, loading, refreshSession, logout],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const value = useContext(AuthContext);
  if (!value) {
    throw new Error("useAuth must be used within AuthProvider");
  }
  return value;
}
