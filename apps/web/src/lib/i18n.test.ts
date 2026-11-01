import { readFileSync } from "node:fs";
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

  it("ships every static key used by the ecosystem pages", () => {
    for (const page of ["Plugins", "Connectors", "Enterprise", "Login"]) {
      const source = readFileSync(new URL(`../pages/${page}.tsx`, import.meta.url), "utf8");
      const keys = [...source.matchAll(/\bt\("([^"]+)"/g)].map((match) => match[1]!);
      expect(keys.length).toBeGreaterThan(page === "Login" ? 15 : 30);
      for (const key of keys) {
        expect(en).toContain(key);
        expect(zh).toContain(key);
      }
    }
  });
});
