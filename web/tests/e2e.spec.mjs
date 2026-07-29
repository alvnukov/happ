import { test, expect } from "@playwright/test";
import { AxeBuilder } from "@axe-core/playwright";
import { openUtility, stabilizePage } from "./helpers.mjs";

test("main flows and converter behavior work", async ({ page }) => {
  await page.goto("/");
  await stabilizePage(page);

  await expect(page.getByRole("heading", { name: "Main Import" })).toBeVisible();
  await openUtility(page, "Converters");
  await expect(page.getByRole("heading", { name: "Converters" })).toBeVisible();

  await page.locator("select").first().selectOption("text-to-hex");
  const inputEditor = page.locator(".conv-grid .cm-editor").first();
  await inputEditor.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("happ");
  await page.getByRole("button", { name: "plain" }).click();

  const output = page.locator(".conv-grid .code-output, .conv-grid .hexdump-view").nth(0);
  await expect(output).toContainText("68617070");
});

test("main import keeps mode-specific context explicit", async ({ page }) => {
  await page.goto("/");
  await stabilizePage(page);

  await expect(page.getByRole("button", { name: "Import chart" })).toBeVisible();
  await expect(page.getByText("No generated values yet")).toBeVisible();

  await page.getByRole("button", { name: "Manifests" }).click();
  await expect(page.getByRole("button", { name: "Import manifests" })).toBeVisible();
  await expect(page.getByText("path optional")).toBeVisible();
  await expect(page.getByText("input only (ignore path manifests)")).toBeVisible();

  await page.getByRole("button", { name: "Compose" }).click();
  await expect(page.getByRole("button", { name: "Import compose" })).toBeVisible();
  await expect(page.getByText("Compose import is path-based")).toBeVisible();
  await expect(page.getByText("server path required")).toBeVisible();

  await page.getByRole("button", { name: "Advanced settings" }).click();
  await expect(page.getByText("Main screen owns source and render identity")).toBeVisible();
  await expect(page.getByText("Generated values layout")).toBeVisible();
  await expect(page.getByText("Dedup and extraction")).toBeVisible();
  await expect(page.getByText("Unsupported include handling")).toBeVisible();
  await expect(page.getByText("Capability overrides")).toBeVisible();
});

test("main import keeps selected path and error state consistent", async ({ page }) => {
  await page.goto("/");
  await stabilizePage(page);

  const missingChartPath = "/definitely/missing-happ-chart";
  const sourcePathInput = page.locator(".context-card").first().locator("input[type='text']").first();
  await sourcePathInput.fill(missingChartPath);
  await expect(page.getByText(`Selected: ${missingChartPath}`)).toBeVisible();

  await page.getByRole("button", { name: "Import chart" }).click();
  await expect(page.getByText("Import failed")).toBeVisible();
  await expect(page.getByText("No generated values yet")).toHaveCount(0);
});

test("accessibility smoke has no critical issues", async ({ page }) => {
  await page.goto("/");
  await stabilizePage(page);
  const results = await new AxeBuilder({ page }).analyze();
  const severe = results.violations.filter((v) => v.impact === "critical");
  expect(severe, `Critical axe violations: ${JSON.stringify(severe, null, 2)}`).toEqual([]);
});

test("jq playground query supports undo and redo shortcuts", async ({ page }) => {
  await page.goto("/");
  await stabilizePage(page);
  await openUtility(page, "jq Playground");

  const queryEditor = page.locator(".jq-query-input");
  await queryEditor.click();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type(".");
  await page.keyboard.type("a");
  await expect(queryEditor).toHaveValue(".a");

  await page.keyboard.press("ControlOrMeta+Z");
  await expect(queryEditor).toHaveValue(".");

  await page.keyboard.press("ControlOrMeta+Shift+Z");
  await expect(queryEditor).toHaveValue(".a");
});
