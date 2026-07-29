import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "@playwright/test";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rustRoot = path.resolve(__dirname, "..");
const baseURL = process.env.HAPP_WEB_BASE_URL || "http://127.0.0.1:18088";
const listenAddr = process.env.HAPP_WEB_LISTEN_ADDR || new URL(baseURL).host;
const skipManagedServer = process.env.HAPP_WEB_SKIP_SERVER === "1";
const webServerCommand =
  process.env.HAPP_WEB_SERVER_CMD ||
  `sh -lc 'exec ./target/debug/happ --web --web-addr ${listenAddr} --web-open-browser=false < /dev/null'`;

export default defineConfig({
  testDir: "./tests",
  timeout: 45_000,
  expect: {
    timeout: 10_000,
  },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 2 : undefined,
  outputDir: "test-artifacts",
  reporter: [
    ["list"],
    ["html", { open: "never", outputFolder: "test-results/playwright-report-html" }],
    ["json", { outputFile: "test-results/playwright-report.json" }],
  ],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    viewport: { width: 1720, height: 1080 },
  },
  webServer: skipManagedServer
    ? undefined
    : {
        command: webServerCommand,
        cwd: rustRoot,
        url: baseURL,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
      },
});
