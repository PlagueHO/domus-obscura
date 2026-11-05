import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./browser",
  workers: 1,
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
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
    command:
      process.platform === "win32"
        ? "..\\target\\debug\\examples\\browser_workflow.exe"
        : "../target/debug/examples/browser_workflow",
    url: "http://127.0.0.1:4174/api/session",
    reuseExistingServer: false,
    timeout: 30000,
    gracefulShutdown: { signal: "SIGINT", timeout: 10000 },
  },
});
