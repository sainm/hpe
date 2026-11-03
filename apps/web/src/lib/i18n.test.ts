import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import i18n from "./i18n";

function flatten(value: unknown, prefix = ""): string[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return prefix ? [prefix] : [];
  }
  return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) =>
    flatten(child, prefix ? `${prefix}.${key}` : key),
  );
}

describe("i18n resources", () => {
  const ja = flatten(i18n.getResourceBundle("ja", "translation"));
  const en = flatten(i18n.getResourceBundle("en", "translation"));
  const zh = flatten(i18n.getResourceBundle("zh", "translation"));

  it("keeps the same keys in every locale", () => {
    expect([...en].sort()).toEqual([...ja].sort());
    expect([...zh].sort()).toEqual([...ja].sort());
  });

  it("ships every Agent Center key in every locale", () => {
    const agentKeys = ja.filter((key) => key.startsWith("agents."));
    expect(agentKeys.length).toBeGreaterThan(40);
    for (const key of agentKeys) {
      expect(en).toContain(key);
      expect(zh).toContain(key);
    }
  });

  it("ships every Plugin and Connector Center key in every locale", () => {
    const ecosystemKeys = ja.filter(
      (key) => key.startsWith("plugins.") || key.startsWith("connectors."),
    );
    expect(ecosystemKeys.length).toBeGreaterThan(120);
    for (const key of ecosystemKeys) {
      expect(en).toContain(key);
      expect(zh).toContain(key);
    }
    for (const key of ["nav.plugins", "nav.connectors"]) {
      expect(en).toContain(key);
      expect(zh).toContain(key);
    }
  });

  it("ships every auth key in every locale", () => {
    const authKeys = ja.filter((key) => key.startsWith("auth."));
    expect(authKeys.length).toBeGreaterThan(20);
    for (const key of authKeys) {
      expect(en).toContain(key);
      expect(zh).toContain(key);
    }
  });


  it("uses i18next double-brace interpolation everywhere", () => {
    const singleBrace = /(?<!\{)\{[a-zA-Z][a-zA-Z0-9_]*\}(?!\})/;
    const offenders: string[] = [];
    const walk = (value: unknown, path: string) => {
      if (typeof value === "string") {
        const match = value.match(singleBrace);
        if (match) offenders.push(`${path}: ${match[0]}`);
        return;
      }
      if (!value || typeof value !== "object" || Array.isArray(value)) return;
      for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
        walk(child, path ? `${path}.${key}` : key);
      }
    };
    for (const locale of ["ja", "en", "zh"] as const) {
      walk(i18n.getResourceBundle(locale, "translation"), locale);
    }
    expect(offenders).toEqual([]);
  });

  it("ships every static key used by the ecosystem pages", () => {
    const minimums: Record<string, number> = {
      Plugins: 30,
      Connectors: 30,
      Enterprise: 30,
      Login: 15,
      Consent: 15,
      EnterpriseOAuth: 20,
      Triggers: 20,
    };
    for (const [page, minimum] of Object.entries(minimums)) {
      const source = readFileSync(new URL(`../pages/${page}.tsx`, import.meta.url), "utf8");
      const keys = [...source.matchAll(/\bt\("([^"]+)"/g)].map((match) => match[1]!);
      expect(keys.length).toBeGreaterThan(minimum);
      for (const key of keys) {
        expect(en).toContain(key);
        expect(zh).toContain(key);
      }
    }
  });

  it("ships every static key used by any page", () => {
    for (const file of readdirSync(new URL("../pages/", import.meta.url))) {
      if (!file.endsWith(".tsx")) continue;
      const source = readFileSync(new URL(`../pages/${file}`, import.meta.url), "utf8");
      for (const match of source.matchAll(/\bt\("([^"]+)"/g)) {
        const key = match[1]!;
        expect(en, `${file}: ${key}`).toContain(key);
        expect(zh, `${file}: ${key}`).toContain(key);
      }
    }
  });
});
