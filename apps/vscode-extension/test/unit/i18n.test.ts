import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { I18n, localeFromLanguage, messageKeys, missingKeys, type Locale } from "../../src/i18n";

const locales: Locale[] = ["ja", "en", "zh"];

describe("message catalogs", () => {
  it("keeps the three locales consistent with the Japanese reference", () => {
    const keys = messageKeys();
    expect(keys.length).toBeGreaterThan(0);
    expect(keys).toEqual([...keys].sort());

    for (const locale of locales) {
      expect(missingKeys(locale)).toEqual([]);
      const i18n = new I18n(locale);
      for (const key of keys) {
        expect(i18n.has(key)).toBe(true);
      }
    }
  });

  it("treats English as a complete fallback catalog", () => {
    expect(missingKeys("en")).toEqual([]);
    expect(new I18n("en").has("errors.unauthorized")).toBe(true);
  });

  it("ships every static key used by extension source files", () => {
    const keys = new Set(messageKeys());
    const sourceFiles = (directory: URL): string[] =>
      readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
        const child = new URL(`${entry.name}${entry.isDirectory() ? "/" : ""}`, directory);
        if (entry.isDirectory()) return sourceFiles(child);
        return /\.(ts|tsx)$/.test(entry.name) && entry.name !== "i18n.ts"
          ? [readFileSync(child, "utf8")]
          : [];
      });
    for (const source of sourceFiles(new URL("../../src/", import.meta.url))) {
      for (const match of source.matchAll(/\.t\(\s*["']([^"']+)["']/g)) {
        expect(keys.has(match[1]!), match[1]).toBe(true);
      }
    }
  });
});

describe("I18n.t", () => {
  it("interpolates named values", () => {
    expect(
      new I18n("en").t("skills.installConfirm", {
        name: "release-notes",
        path: ".github/skills/release-notes",
      }),
    ).toBe('Install Skill "release-notes" to .github/skills/release-notes?');
  });

  it("interpolates numeric values", () => {
    expect(
      new I18n("en").t("errors.http", {
        status: 500,
        code: "INTERNAL",
        message: "boom",
      }),
    ).toBe("HTTP 500 (INTERNAL): boom");
  });

  it("interpolates per locale", () => {
    expect(new I18n("zh").t("skills.installedToast", { name: "demo" })).toBe(
      "已安装技能“demo”。",
    );
    expect(new I18n("ja").t("skills.installedToast", { name: "demo" })).toBe(
      "スキル「demo」をインストールしました。",
    );
  });

  it("keeps unresolved placeholders", () => {
    expect(new I18n("en").t("skills.installConfirm", { name: "demo" })).toBe(
      'Install Skill "demo" to {{path}}?',
    );
  });

  it("falls back to English and then to the original key", () => {
    const i18n = new I18n("ja");
    const internals = i18n as unknown as { messages: Record<string, string> };
    internals.messages = {};
    expect(i18n.t("common.refresh")).toBe("Refresh");
    expect(i18n.t("missing.key")).toBe("missing.key");
    expect(new I18n("ja").t("missing.key")).toBe("missing.key");
    expect(new I18n("ja").has("missing.key")).toBe(false);
  });
});

describe("localeFromLanguage", () => {
  it("honors an explicit locale setting", () => {
    expect(localeFromLanguage("zh-CN", "ja")).toBe("ja");
    expect(localeFromLanguage("ja", "en")).toBe("en");
    expect(localeFromLanguage("en", "zh")).toBe("zh");
  });

  it("recognizes Chinese and English VS Code languages", () => {
    expect(localeFromLanguage("zh-cn", "auto")).toBe("zh");
    expect(localeFromLanguage("zh-Hant", "auto")).toBe("zh");
    expect(localeFromLanguage("en-US", "auto")).toBe("en");
    expect(localeFromLanguage("EN", "auto")).toBe("en");
  });

  it("defaults to Japanese for auto and unknown languages", () => {
    expect(localeFromLanguage("ja", "auto")).toBe("ja");
    expect(localeFromLanguage("ja-JP", "auto")).toBe("ja");
    expect(localeFromLanguage("fr-FR", "auto")).toBe("ja");
    expect(localeFromLanguage(undefined, "auto")).toBe("ja");
    expect(localeFromLanguage("", "auto")).toBe("ja");
  });
});
