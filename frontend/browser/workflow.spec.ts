import { expect, test, type Page } from "@playwright/test";
import type { JobSummary } from "../src/generated/JobSummary";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { CatalogBatchPlanRequest } from "../src/generated/CatalogBatchPlanRequest";
import type { RecoveryEntry } from "../src/generated/RecoveryEntry";

const origin = "http://127.0.0.1:4174";
const guards = { Origin: origin, "x-domus-csrf": "1" };

async function openSelection(page: Page) {
  const selection = page.getByRole("region", { name: /^Selected albums/ });
  const open = selection.getByRole("button", { name: "Open selection" });
  if (await open.isVisible()) await open.click();
  return selection;
}
async function control(page: Page, path: string, data = {}) {
  const response = await page.request.post(path, { headers: guards, data });
  expect(response.ok(), await response.text()).toBe(true);
}

test.beforeEach(async ({ page }) => {
  await control(page, "/__fixture/reset");
  await page.route("**/*", (route) =>
    new URL(route.request().url()).origin === origin ? route.continue() : route.abort(),
  );
});

async function selectAlbums(page: Page, count = 1, backup = false) {
  await page.goto("/");
  await page.getByRole("searchbox").fill("Browser");
  await page.getByRole("searchbox").press("Enter");
  await page.getByRole("button", { name: "Select Browser Album 1", exact: true }).click();
  await page.locator(".artist-card a").click();
  await expect(page.getByRole("heading", { name: "Browser Artist", exact: true })).toBeVisible();
  if (count === 2)
    await page.getByRole("button", { name: "Select Browser Album 2", exact: true }).click();
  await page.screenshot({ path: test.info().outputPath("artist.png") });
  await page.locator(".album-card").first().click();
  await expect(page.getByRole("heading", { name: "Browser Album 1", exact: true })).toBeVisible();
  const selection = await openSelection(page);
  await expect(selection).toContainText(`${count} / 25`);
  await selection.getByRole("checkbox", { name: /^main/ }).check();
  if (backup) await selection.getByRole("checkbox", { name: /^backup/ }).check();
  await page.locator(".tray").screenshot({ path: test.info().outputPath("selection.png") });
  await selection.getByRole("button", { name: "Prepare selected albums" }).click();
  await expect(page).toHaveURL(/#\/request\/1\/2$/);
  return selection;
}

async function approveAlbum(page: Page, collision = false) {
  await page.getByRole("button", { name: "Approve frozen plan 2" }).click();
  const listing = await page.request.get("/__fixture/listing");
  await page.getByLabel("Or import a bounded SourceFolder JSON listing").setInputFiles({
    name: "generated-source.json",
    mimeType: "application/json",
    buffer: await listing.body(),
  });
  await page
    .getByRole("checkbox", {
      name: "I checked that this is the exact edition, and I have permission to download it.",
    })
    .check();
  await page.getByRole("button", { name: "Acknowledge this exact source" }).click();
  await expect(
    page.getByText("Change or inspect acknowledged source", { exact: false }),
  ).toBeVisible();
  await expect(page.locator("fieldset.review-fields")).toBeEnabled();
  await page
    .getByRole("checkbox", { name: "I have permission to use the image I will upload." })
    .check();
  const cover = await page.request.get("/__fixture/cover");
  await page.getByLabel("Upload a local cover").setInputFiles({
    name: "generated-cover.png",
    mimeType: "image/png",
    buffer: await cover.body(),
  });
  await page
    .getByRole("checkbox", { name: "I reviewed this exact normalized JPEG preview." })
    .check();
  await page.getByRole("button", { name: "Approve this exact cover", exact: true }).click();
  if (!collision)
    await expect(page.getByRole("button", { name: "Start Download", exact: true })).toBeEnabled();
}

async function verifyOutput(page: Page, job = 1) {
  const response = await page.request.get(`/__fixture/verify/${job}/2`);
  expect(response.ok(), await response.text()).toBe(true);
  const evidence = await response.json();
  expect(evidence.audio_unchanged).toBe(true);
  expect(evidence.tags_verified).toBe(true);
  expect(evidence.exact_jpeg).toBe(true);
  expect(evidence.files).toEqual([
    `B/BROWSER ARTIST/BROWSER ALBUM ${job}/Browser Artist - Browser Album ${job} - 01 - Generated Track.mp3`,
  ]);
}

test("search, persistent multi-selection and separate approvals publish generated albums", async ({
  page,
}) => {
  await selectAlbums(page, 2);
  let jobs: JobSummary[] = await (await page.request.get("/api/jobs")).json();
  expect(jobs.map((job) => job.approved_at)).toEqual([null, null]);
  await approveAlbum(page);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: test.info().outputPath("request.png") });
  jobs = await (await page.request.get("/api/jobs")).json();
  expect(jobs[1]!.approved_at).toBeNull();
  await page.getByRole("button", { name: "Approve and review next (1)" }).click();
  await expect(page).toHaveURL(/#\/request\/2\/2$/);
  await expect(page.getByRole("button", { name: "Approve frozen plan 2" })).toBeVisible();
  expect(await (await page.request.get("/api/jobs/2/plans/2/source-candidates")).json()).toEqual(
    [],
  );
  expect(await (await page.request.get("/api/jobs/2/plans/2/artwork")).json()).toEqual([]);
  await verifyOutput(page);
  await approveAlbum(page);
  await page.getByRole("button", { name: "Start Download", exact: true }).click();
  await expect(
    page.getByText("The server reports this request complete.", { exact: false }),
  ).toBeVisible();
  await verifyOutput(page, 2);
  await control(page, "/__fixture/scan");
  await page.goto("/#/library?location=1&view=artists");
  await expect(page.getByRole("heading", { name: /Library artists/i })).toBeVisible();
  await expect(page.getByText("Browser Artist", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Browser Artist", exact: true }).click();
  await page.locator(".library-list a").filter({ hasText: "Browser Album 1" }).click();
  await expect(page.getByRole("heading", { name: "Browser Album 1", exact: true })).toBeVisible();
  await page
    .getByRole("link", { name: /Browser Artist - Browser Album 1 - 01 - Generated Track\.mp3$/ })
    .click();
  await expect(page.getByRole("heading", { name: "Library file", exact: true })).toBeVisible();
  await expect(page.getByText("Generated Track", { exact: true })).toBeVisible();
  await verifyOutput(page);
  await page.screenshot({ path: test.info().outputPath("library.png"), fullPage: true });
});

test("unknown job creation is never retried", async ({ page }) => {
  let submissions = 0;
  await page.route("**/api/jobs", async (route) => {
    if (route.request().method() !== "POST") {
      await route.continue();
      return;
    }
    submissions++;
    await route.fetch();
    await route.abort("connectionreset");
  });
  await page.goto("/#/search?q=Browser");
  await page.getByRole("button", { name: "Select Browser Album 1", exact: true }).click();
  const selection = await openSelection(page);
  await selection.getByRole("checkbox", { name: /^main/ }).check();
  await selection.getByRole("button", { name: "Prepare selected albums" }).click();
  await expect(selection.getByText("Job creation result unknown.", { exact: false })).toBeVisible();
  await expect(selection.getByRole("button", { name: "Prepare selected albums" })).toBeDisabled();
  expect(submissions).toBe(1);
  expect(await (await page.request.get("/api/jobs")).json()).toHaveLength(1);
});

test("download dialog supports keyboard navigation and returns focus on Escape", async ({
  page,
}) => {
  await page.goto("/#/album/20000000-0000-4000-8000-000000000001");
  const open = page.getByRole("button", { name: "Start Download", exact: true });
  await open.click();
  const dialog = page.getByRole("dialog", { name: "Download album", exact: true });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("checkbox", { name: /^main/ })).toBeEnabled();
  await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true);
  await page.screenshot({ path: test.info().outputPath("drawer.png") });
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  await expect(open).toBeFocused();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("lost batch response is reconciled without a second submission or new jobs", async ({
  page,
}) => {
  let submissions = 0;
  await page.route("**/api/discovery/plans", async (route) => {
    submissions++;
    await route.fetch();
    await route.abort("connectionreset");
  });
  await page.goto("/#/search?q=Browser");
  await page.getByRole("button", { name: "Select Browser Album 1", exact: true }).click();
  const selection = await openSelection(page);
  await selection.getByRole("checkbox", { name: /^main/ }).check();
  await selection.getByRole("button", { name: "Prepare selected albums" }).click();
  await expect(selection.getByRole("alert")).toBeVisible();
  await expect(selection.getByRole("button", { name: "Remove Browser Album 1" })).toBeDisabled();
  await selection.getByRole("button", { name: "Check saved batch result" }).click();
  await expect(selection.getByRole("link", { name: "Review Browser Album 1" })).toBeVisible();
  expect(submissions).toBe(1);
  const jobs: JobSummary[] = await (await page.request.get("/api/jobs")).json();
  expect(jobs).toHaveLength(1);
  expect(jobs[0]!.plan_version).toBe(2);
});

test("an inspected mismatched batch can be removed without creating or retrying jobs", async ({
  page,
}) => {
  let submissions = 0;
  let creations = 0;
  await page.route("**/api/jobs", async (route) => {
    if (route.request().method() === "POST") creations++;
    await route.continue();
  });
  await page.route("**/api/discovery/plans", async (route) => {
    submissions++;
    const body = route.request().postDataJSON() as CatalogBatchPlanRequest;
    body.albums[0]!.request.accepts_incomplete_coverage = true;
    await route.fetch({ postData: JSON.stringify(body) });
    await route.abort("connectionreset");
  });
  await page.goto("/#/search?q=Browser");
  await page.getByRole("button", { name: "Select Browser Album 1", exact: true }).click();
  await page.getByRole("button", { name: "Select Browser Album 2", exact: true }).click();
  let selection = await openSelection(page);
  await selection.getByRole("checkbox", { name: /^main/ }).check();
  await selection.getByRole("button", { name: "Prepare selected albums" }).click();
  await expect(selection.getByRole("alert")).toBeVisible();
  const abandon = selection.getByRole("button", {
    name: "Remove inspected batch from selection",
  });
  await expect(abandon).toHaveCount(0);
  await page.route("**/api/jobs?*", (route) => route.abort("connectionreset"));
  await selection.getByRole("button", { name: "Check saved batch result" }).click();
  await expect(selection.getByRole("alert")).toContainText("Failed to fetch");
  await expect(abandon).toHaveCount(0);
  await expect(selection.getByRole("button", { name: "Remove Browser Album 1" })).toBeDisabled();
  await page.unroute("**/api/jobs?*");
  await selection.getByRole("button", { name: "Check saved batch result" }).click();
  await expect(selection.getByRole("alert")).toContainText("Saved plans changed");
  await expect(abandon).toBeEnabled();
  await expect(selection.getByRole("button", { name: "Remove Browser Album 2" })).toBeDisabled();
  await selection.getByRole("link", { name: "Inspect the saved queue" }).click();
  selection = await openSelection(page);
  await selection.getByRole("button", { name: "Remove inspected batch from selection" }).click();
  await expect(page.getByRole("region", { name: /^Selected albums/ })).toHaveCount(0);
  await page.goto("/#/search?q=Browser");
  await expect(
    page.getByRole("button", { name: "Select Browser Album 1", exact: true }),
  ).toBeEnabled();
  const jobs: JobSummary[] = await (await page.request.get("/api/jobs")).json();
  expect(jobs.map((job) => job.plan_version)).toEqual([2, 2]);
  expect(creations).toBe(2);
  expect(submissions).toBe(1);
});

test("backup pending keeps the verified main separate and resumes retained publication", async ({
  page,
}) => {
  await selectAlbums(page, 1, true);
  await approveAlbum(page);
  await control(page, "/__fixture/backup", { offline: true });
  await page.getByRole("button", { name: "Refresh request", exact: true }).click();
  await page.getByRole("button", { name: "Start Download", exact: true }).click();
  await expect(
    page.getByText("The primary is verified. The backup is not complete.", { exact: false }),
  ).toBeVisible();
  await verifyOutput(page);
  await control(page, "/__fixture/backup", { offline: false });
  await page.getByRole("button", { name: "Review publication recovery", exact: true }).click();
  await page.getByRole("button", { name: "Confirm publication recovery", exact: true }).click();
  await expect(
    page.getByText("The server reports this request complete.", { exact: false }),
  ).toBeVisible();
  const evidence = await (await page.request.get("/__fixture/verify/1/2")).json();
  expect(
    evidence.destinations.filter(
      (destination: { verified_at: number | null }) => destination.verified_at !== null,
    ),
  ).toHaveLength(2);
});

test("replacement retains the original and restore requires a fresh explicit review", async ({
  page,
}) => {
  await selectAlbums(page);
  await control(page, "/__fixture/existing/1/2");
  await approveAlbum(page, true);
  await page.getByRole("button", { name: "Refresh request", exact: true }).click();
  await page.getByRole("button", { name: "Replace this album", exact: true }).click();
  await page.getByRole("button", { name: /Confirm replacement/ }).click();
  await page.getByRole("button", { name: "Start Download", exact: true }).click();
  await verifyOutput(page);
  await page.getByRole("button", { name: "Review restore", exact: true }).click();
  await expect(page.getByRole("listitem").filter({ hasText: "original.txt" })).toBeVisible();
  await control(page, "/__fixture/change-current/1/2");
  await page.getByRole("button", { name: "Confirm restore", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.getByRole("button", { name: "Confirm restore", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Review restore", exact: true }).click();
  await expect(page.getByRole("listitem").filter({ hasText: "changed.txt" })).toBeVisible();
  await page.getByRole("button", { name: /Confirm.*restore|Restore.*original/ }).click();
  await expect(
    page.getByText("The reviewed original album was restored.", { exact: false }),
  ).toBeVisible();
  await page.goto("/#/recovery?job=1");
  await expect(page.getByRole("heading", { name: "Recovery", exact: true })).toBeVisible();
  await expect(page.getByText("Recorded bytes are saved evidence", { exact: false })).toBeVisible();
  const rows: RecoveryEntry[] = await (await page.request.get("/api/recovery?job=1")).json();
  const retained = rows.find((row) => row.category === "retained" && row.eligible);
  expect(retained).toBeDefined();
  await page.getByRole("button", { name: `Inspect entry ${retained!.id}`, exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Delete this retained version", exact: true }),
  ).toBeVisible();
  await page.route("**/api/recovery/*/preview", (route) =>
    route.fulfill({ status: 503, json: { error: "Generated preview unavailable" } }),
  );
  await page.getByRole("button", { name: "Refresh selected preview", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Generated preview unavailable");
  await expect(page.getByRole("button", { name: "Review irreversible deletion" })).toHaveCount(0);
  await page.unroute("**/api/recovery/*/preview");
  await page.getByRole("button", { name: "Refresh selected preview", exact: true }).click();
  await page
    .getByRole("checkbox", {
      name: "I understand that this retained version will be permanently deleted.",
    })
    .check();
  await page.getByRole("button", { name: "Review irreversible deletion" }).click();
  await page.getByRole("button", { name: `Permanently delete entry ${retained!.id}` }).click();
  await expect(page.getByText("The command returned.", { exact: false })).toBeVisible();
  const original = await page.request.get("/__fixture/verify-original/1/2");
  expect(original.ok(), await original.text()).toBe(true);
});

test("backend JSON save preserves startup state and rejects a stale section revision", async ({
  page,
}) => {
  await page.goto("/#/settings");
  const initial: BackendConfigurationState = await (
    await page.request.get("/api/system/configuration")
  ).json();
  await page.getByRole("textbox", { name: /^Listen address/ }).fill("127.0.0.1:4175");
  await page.getByRole("textbox", { name: /^Browser origin/ }).fill("http://127.0.0.1:4175");
  const submission = page.waitForRequest((request) =>
    request.url().endsWith("/api/system/configuration/sections/save"),
  );
  await page.getByRole("button", { name: "Save changes", exact: true }).click();
  const body: unknown = (await submission).postDataJSON();
  await expect(page.getByText("Changes saved.", { exact: true })).toBeVisible();
  const saved: BackendConfigurationState = await (
    await page.request.get("/api/system/configuration")
  ).json();
  expect(saved.active_revision).toBe(initial.active_revision);
  expect(saved.active.origin).toBe(origin);
  expect(saved.saved.origin).toBe("http://127.0.0.1:4175");
  expect(saved.restart_required).toBe(true);
  expect(saved.saved_revision).not.toBe(initial.saved_revision);
  const rejected = await page.request.post("/api/system/configuration/sections/save", {
    headers: guards,
    data: body,
  });
  expect(rejected.status()).toBe(409);
});

test("one stale album rejects the full batch and manual reconciliation preserves both jobs", async ({
  page,
}) => {
  await page.route("**/api/discovery/plans", async (route) => {
    const body = route.request().postDataJSON() as CatalogBatchPlanRequest;
    body.albums[1]!.request.expected_version = 99;
    await route.continue({ postData: JSON.stringify(body) });
  });
  await page.goto("/#/search?q=Browser");
  await page.getByRole("button", { name: "Select Browser Album 1", exact: true }).click();
  await page.getByRole("button", { name: "Select Browser Album 2", exact: true }).click();
  const selection = await openSelection(page);
  await selection.getByRole("checkbox", { name: /^main/ }).check();
  await selection.getByRole("button", { name: "Prepare selected albums" }).click();
  await expect(selection.getByRole("alert")).toBeVisible();
  let jobs: JobSummary[] = await (await page.request.get("/api/jobs")).json();
  expect(jobs.map((job) => job.plan_version)).toEqual([1, 1]);
  await selection.getByRole("button", { name: "Check saved batch result" }).click();
  await expect(selection.getByRole("alert")).toContainText("no new plans");
  await page.unroute("**/api/discovery/plans");
  await selection.getByRole("button", { name: "Prepare selected albums" }).click();
  await expect(page).toHaveURL(/#\/request\/1\/2$/);
  jobs = await (await page.request.get("/api/jobs")).json();
  expect(jobs.map((job) => job.plan_version)).toEqual([2, 2]);
});

test("loading, failed and empty metadata never appear as current evidence", async ({ page }) => {
  let release: (() => void) | undefined;
  const wait = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/discovery/releases?*", async (route) => {
    await wait;
    await route.continue();
  });
  await page.goto("/#/search?q=Browser");
  await expect(page.getByText("Searching albums", { exact: false })).toBeVisible();
  release!();
  await expect(
    page.getByRole("button", { name: "Select Browser Album 1", exact: true }),
  ).toBeVisible();
  await page.unroute("**/api/discovery/releases?*");
  await page.route("**/api/discovery/releases?*", (route) =>
    route.fulfill({ status: 503, json: { error: "Generated provider unavailable" } }),
  );
  await page.getByRole("button", { name: "Search albums again", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Generated provider unavailable");
  await expect(
    page.getByRole("button", { name: "Select Browser Album 1", exact: true }),
  ).toHaveCount(0);
  await page.unroute("**/api/discovery/releases?*");
  await page.route("**/api/discovery/releases?*", (route) =>
    route.fulfill({
      json: {
        source: "generated",
        cached: true,
        data: { offset: 0, count: 0, next_offset: null, items: [] },
      },
    }),
  );
  await page.getByRole("button", { name: "Search albums again", exact: true }).click();
  await expect(page.getByText("No albums", { exact: false })).toBeVisible();
  await control(page, "/__fixture/incomplete");
  await page.goto("/#/library?location=1&view=artists");
  await expect(page.getByText("incomplete", { exact: false }).first()).toBeVisible();
  await page.getByRole("button", { name: "Open notifications", exact: true }).click();
  await expect(
    page.getByRole("complementary", { name: "Notifications", exact: true }),
  ).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  if (test.info().project.name === "narrow")
    expect(await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches)).toBe(
      true,
    );
});

test("paused questions keep answer and recovery separate", async ({ page }) => {
  await selectAlbums(page);
  await page.getByRole("button", { name: "Pause at a safe boundary", exact: true }).click();
  await page.getByRole("button", { name: "Refresh request", exact: true }).click();
  await expect(page.getByText("Action required:", { exact: false })).toBeVisible();
  await page.getByRole("link", { name: "Open the saved decision", exact: true }).click();
  await expect(page.getByRole("heading", { name: /Decision #/ })).toBeVisible();
  await expect(page.getByText("Nothing is", { exact: false }).first()).toBeVisible();
});

test("settings policy is version-bound and read-only; keyboard and reduced motion remain usable", async ({
  page,
}) => {
  await page.goto("/");
  expect(
    await page.locator(".orbit").evaluate((header) => {
      const tools = header.querySelector(".orbit-tools")?.getBoundingClientRect();
      const bounds = header.getBoundingClientRect();
      return tools !== undefined && tools.top >= bounds.top && tools.bottom <= bounds.bottom;
    }),
  ).toBe(true);
  await page.keyboard.press("Tab");
  await expect(page.getByRole("link", { name: "Skip to Music content" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#main")).toBeFocused();
  await page.getByRole("link", { name: "System settings", exact: true }).click();
  if (test.info().project.name === "narrow")
    await page
      .getByRole("combobox", { name: "Settings section", exact: true })
      .selectOption("music_output");
  else await page.getByRole("button", { name: "Output policy", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Output policy", exact: true })).toBeVisible();
  const settings: SettingsVersion = await (await page.request.get("/api/settings")).json();
  await page.getByRole("button", { name: "Save system output settings" }).click();
  await expect(
    page.getByText(`New plans use version ${settings.version + 1}`, { exact: false }),
  ).toBeVisible();
  const stale = await page.request.put("/api/settings", {
    headers: guards,
    data: { expected_version: settings.version, settings: settings.settings },
  });
  expect(stale.status()).toBe(409);
  await control(page, "/__fixture/read-only", { read_only: true });
  await page.reload();
  if (test.info().project.name === "narrow")
    await page
      .getByRole("combobox", { name: "Settings section", exact: true })
      .selectOption("music_output");
  else await page.getByRole("button", { name: "Output policy", exact: true }).click();
  await expect(page.getByText("This form is read-only.", { exact: false })).toBeVisible();
  const rejected = await page.request.put("/api/settings", {
    headers: guards,
    data: { expected_version: settings.version + 1, settings: settings.settings },
  });
  expect(rejected.status()).toBe(409);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});
