import AxeBuilder from "@axe-core/playwright";
import { chromium } from "playwright";

const baseUrl = (process.env.HPA_BASE_URL || "http://127.0.0.1:8787").replace(/\/+$/, "");
const routes = [
  "/",
  "/workflows",
  "/triggers",
  "/agents",
  "/skills",
  "/enterprise",
  "/login",
  "/settings",
];
const themes = ["light", "dark", "ocean"];
const viewports = [1440, 607];
const failures = [];
const browser = await chromium.launch({ headless: true });

try {
  for (const theme of themes) {
    for (const width of viewports) {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      await context.addInitScript((selected) => {
        localStorage.setItem("hpa.theme", selected);
      }, theme);
      const page = await context.newPage();
      for (const route of routes) {
        const response = await page.goto(`${baseUrl}${route}`, { waitUntil: "networkidle" });
        if (!response || !response.ok()) {
          failures.push(`${theme}@${width}${route}: HTTP ${response?.status() ?? "no response"}`);
          continue;
        }
        const result = await new AxeBuilder({ page })
          .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
          .analyze();
        for (const violation of result.violations) {
          if (["critical", "serious"].includes(violation.impact)) {
            failures.push(
              `${theme}@${width}${route}: ${violation.id} (${violation.impact})`,
            );
          }
        }
        const overflow = await page.evaluate(
          () => document.documentElement.scrollWidth > window.innerWidth,
        );
        if (overflow) {
          failures.push(`${theme}@${width}${route}: horizontal overflow`);
        }
      }

      await page.goto(baseUrl, { waitUntil: "networkidle" });
      await page.keyboard.press("Tab");
      const focus = await page.evaluate(() => ({
        tag: document.activeElement?.tagName ?? "",
        text: document.activeElement?.textContent?.trim().slice(0, 80) ?? "",
      }));
      if (focus.tag === "BODY") {
        failures.push(
          `${theme}@${width} keyboard: the first Tab does not move focus away from body`,
        );
      }
      console.log(`${theme}@${width} keyboard focus reached ${focus.tag}: ${focus.text}`);
      await context.close();
    }
  }
} finally {
  await browser.close();
}

if (failures.length > 0) {
  console.error(`Web accessibility check failed (${failures.length}):`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(
  `Web accessibility check passed: ${routes.length} routes × ${themes.length} themes × ${viewports.length} widths, serious/critical axe violations = 0`,
);
