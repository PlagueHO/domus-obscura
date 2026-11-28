import { defineConfig } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

export default defineConfig({
  testDir: "./browser",
  workers: 1,
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  reporter: [["list"], ["json", { outputFile: "test-results/browser-results.json" }]],
  metadata: {
    source_revision: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
    dirty: execFileSync("git", ["status", "--porcelain"], { encoding: "utf8" }).trim().length > 0,
    platform: process.platform,
    architecture: process.arch,
    fixtures:
      "Private temporary SQLite and roots; generated MP3/FLAC, catalog and cover; no live providers",
  },
  globalTeardown: "./browser/teardown.ts",
  timeout: 45000,
  globalTimeout: 600000,
  use: {
    baseURL: "http://127.0.0.1:4174",
    browserName: "chromium",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    ...(process.env.DOMUS_BROWSER_EXECUTABLE
      ? { launchOptions: { executablePath: process.env.DOMUS_BROWSER_EXECUTABLE } }
      : {}),
  },
  projects: [
    { name: "desktop", use: { viewport: { width: 1440, height: 1000 } } },
    { name: "narrow", use: { viewport: { width: 390, height: 844 }, reducedMotion: "reduce" } },
  ],
  webServer: {
    command: `"${resolve(
      "..",
      process.env.CARGO_TARGET_DIR ?? "target",
      "debug",
      "examples",
      process.platform === "win32" ? "browser_workflow.exe" : "browser_workflow",
    )}"`,
    url: "http://127.0.0.1:4174/api/session",
    reuseExistingServer: false,
    timeout: 30000,
    gracefulShutdown: { signal: "SIGINT", timeout: 10000 },
  },
});
