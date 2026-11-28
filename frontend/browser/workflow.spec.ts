import { expect, test, type Page } from "@playwright/test";
import type { JobSummary } from "../src/generated/JobSummary";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { CatalogBatchPlanRequest } from "../src/generated/CatalogBatchPlanRequest";
import type { RecoveryEntry } from "../src/generated/RecoveryEntry";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import type { QuestionDetails } from "../src/generated/QuestionDetails";
import type { ApproveArtwork } from "../src/generated/ApproveArtwork";
import type { AudioProfile } from "../src/generated/AudioProfile";

const origin = "http://127.0.0.1:4174";
const guards = { Origin: origin, "x-domus-csrf": "1" };

for (const backup of [false, true]) {
  test(`${backup ? "backup" : "primary"} location requires exact review and a separate coordinated restart`, async ({
    page,
  }, testInfo) => {
    const path = backup ? "\\\\fixture\\Backup" : "D:\\Generated-fixture\\Music";
    const role = backup ? "backup library" : "primary library";
    const title = backup ? "Backup library" : "Primary library";
    const roleInput = backup ? { backup: true } : {};
    await page.setViewportSize({
      width: testInfo.project.name === "narrow" ? 375 : 1280,
      height: 900,
    });
    let finishInspection!: () => void;
    const inspection = new Promise<void>((resolve) => {
      finishInspection = resolve;
    });
    let state = "approved";
    let inspections = 0;
    let approvals = 0;
    let activations = 0;
    const saved = () => ({
      id: 12,
      path,
      state,
      location_id: state === "active" ? 9 : null,
      configuration_revision: state === "active" ? "b".repeat(64) : null,
      last_error: null,
    });
    await page.route("**/api/library/primary/**", async (route) => {
      const url = new URL(route.request().url());
      const endpoint = url.pathname;
      if (endpoint.endsWith("/control")) {
        await route.fulfill({
          json: {
            available: true,
            reason: null,
            pending_change_id: approvals && state !== "active" ? 12 : null,
            pending_backup: backup,
          },
        });
      } else if (endpoint.endsWith("/inspect")) {
        inspections++;
        expect(route.request().postDataJSON()).toEqual({ path, ...roleInput });
        await inspection;
        await route.fulfill({
          json: {
            path,
            expected_revision: "a".repeat(64),
            previous_location_id: 1,
            review_token: "c".repeat(64),
            storage_layout: [
              { role: "Application storage", path: "D:\\Generated-fixture\\application" },
              { role: backup ? "New backup library (scan only)" : "New primary library", path },
            ],
          },
        });
      } else if (endpoint.endsWith("/activate")) {
        activations++;
        state = "active";
        await route.fulfill({ status: 202, json: null });
      } else if (endpoint.endsWith("/changes")) {
        approvals++;
        expect(route.request().postDataJSON()).toEqual({
          path,
          expected_revision: "a".repeat(64),
          previous_location_id: 1,
          review_token: "c".repeat(64),
          confirm_separation: true,
          ...roleInput,
        });
        expect(route.request().headers()["x-domus-csrf"]).toBe("1");
        await route.fulfill({ json: saved() });
      } else {
        await route.fulfill({ json: saved() });
      }
    });
    await page.goto("/#/settings?section=music_roots");
    const card = page.getByRole("region", {
      name: backup ? "Backup libraries" : title,
      exact: true,
    });
    const change = card.getByRole("button", {
      name: backup ? "Add backup" : "Change…",
      exact: true,
    });
    await expect(change).toHaveAttribute("aria-expanded", "false");
    await change.click();
    const form = page.getByRole("region", {
      name: `${backup ? "Add" : "Change"} ${role}`,
      exact: true,
    });
    const field = form.getByRole("textbox", { name: `${title} folder` });
    await expect(field).toBeEnabled();
    await expect(form.getByRole("button", { name: "Browse…" })).toBeEnabled();
    await field.fill(path);
    const check = form.getByRole("button", { name: "Check", exact: true });
    try {
      await check.click();
      await expect(check).toHaveAttribute("aria-busy", "true");
      await expect(check).toBeDisabled();
      await expect(check.locator(".button-spinner")).toBeVisible();
    } finally {
      finishInspection();
    }
    const save = page
      .getByRole("group", { name: "Settings save actions" })
      .getByRole("button", { name: `Save ${role} change`, exact: true });
    if (backup) {
      await expect(form.getByText("New backup library (scan only)", { exact: true })).toBeVisible();
      await expect(save).toBeDisabled();
      await form.getByRole("checkbox", { name: /I confirm/ }).check();
    }
    await expect(save).toBeEnabled();
    await expect(form.getByText(`Ready: ${path}`, { exact: false })).toBeVisible();
    await page.screenshot({
      path: test.info().outputPath(`${backup ? "backup" : "primary"}-library-review.png`),
      fullPage: true,
    });
    await save.click();
    await expect(
      form.getByText(`${title} change saved, not yet active`, { exact: true }),
    ).toBeVisible();
    expect(inspections).toBe(1);
    expect(approvals).toBe(1);
    expect(activations).toBe(0);
    if (backup) {
      await page.reload();
      await expect(
        page.getByRole("region", { name: "Change primary library", exact: true }),
      ).toHaveCount(0);
      await form.getByRole("button", { name: "Check saved change" }).click();
    }
    await form.getByRole("button", { name: `Restart and apply ${role}`, exact: true }).click();
    await expect(
      form.getByText(backup ? "Backup library added" : "Primary library changed", { exact: true }),
    ).toBeVisible();
    expect(activations).toBe(1);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
  });
}

test("startup wizard saves missing output policy without scanning media", async ({ page }) => {
  await control(page, "/__fixture/reset", { output_configured: false });
  await page.goto("/");
  await expect(page).toHaveURL(/#\/setup$/);
  await expect(
    page.getByRole("heading", { name: "Shared output policy", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Save initial output policy", exact: true }).click();
  await expect(page.getByText("An output policy is saved.", { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "Continue to finish", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Required setup is complete" })).toBeVisible();
  await expect(
    page.getByText("Backups and download providers are optional.", { exact: false }),
  ).toBeVisible();
  await page.screenshot({ path: test.info().outputPath("setup-wizard.png"), fullPage: true });
  await page.getByRole("link", { name: "Open library", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Your Music library", exact: true }),
  ).toBeVisible();
  expect((await (await page.request.get("/api/settings")).json()).version).toBe(1);
});

test("library folder settings separate registration from primary-led scans and backup checks", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({
    width: testInfo.project.name === "narrow" ? 375 : 1280,
    height: 900,
  });
  await page.goto("/#/settings?section=music_roots");
  const primary = page.getByRole("region", { name: "Primary library", exact: true });
  const backups = page.getByRole("region", { name: "Backup libraries", exact: true });
  await expect(primary.getByRole("table", { name: "Current primary folder" })).toBeVisible();
  const backupFolders = backups.getByRole("table", { name: "Backup folders" });
  await expect(backupFolders.getByRole("cell", { name: /backup/i }).first()).toBeVisible();
  const change = primary.getByRole("button", { name: "Change…", exact: true });
  const add = backups.getByRole("button", { name: "Add backup", exact: true });
  expect((await change.boundingBox())?.width).toBe((await add.boundingBox())?.width);
  for (const button of [change, add]) {
    const spacing = await button.evaluate((element) => {
      const header = element.closest("header")!;
      const outer = header.getBoundingClientRect();
      const inner = element.getBoundingClientRect();
      return [
        inner.top - outer.top,
        outer.bottom - inner.bottom - parseFloat(getComputedStyle(header).borderBottomWidth),
      ];
    });
    expect(spacing[0]).toBeCloseTo(2);
    expect(spacing[1]).toBeCloseTo(2);
  }
  await expect(page.getByRole("textbox", { name: "Server folder path", exact: true })).toHaveCount(
    0,
  );
  await expect(page.getByRole("button", { name: /^Scan / })).toHaveCount(0);
  await add.click();
  await expect(backups.getByRole("textbox", { name: "Backup library folder" })).toBeVisible();
  await expect(backups.getByRole("combobox", { name: "Library folder" })).toHaveCount(0);
  await expect(add).toHaveCount(0);
  await backups.getByRole("button", { name: "Close form" }).click();
  await page.screenshot({ path: test.info().outputPath("library-folders.png"), fullPage: true });
  const scans = page.getByRole("link", { name: "Open Library scans", exact: true });
  await expect(scans).toHaveAttribute("title", "Open Library scans");
  await scans.focus();
  await scans.press("Enter");
  const scan = page.getByRole("button", {
    name: "Scan primary library and check backups",
    exact: true,
  });
  await expect(scan).toBeEnabled();
  await expect(
    page.getByRole("combobox", { name: "Automatic scan frequency", exact: true }),
  ).toHaveValue("");
  await expect(
    page.getByRole("button", { name: "Save scan schedule", exact: true }),
  ).toBeDisabled();
  await scan.click();
  await expect(page.getByText("Library scan requested.", { exact: false })).toBeVisible();
  await expect(
    page.getByText("No path, type or size differences on this page.", { exact: false }),
  ).toBeVisible();
  await page.screenshot({ path: test.info().outputPath("library-scans.png"), fullPage: true });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  const folders = await (await page.request.get("/api/locations")).json();
  expect(folders.map((folder: { coverage: string }) => folder.coverage)).toEqual([
    "complete",
    "complete",
  ]);
});

async function openSelection(page: Page) {
  const selection = page.getByRole("region", { name: /^Selected albums/ });
  const toggle = selection.getByRole("button", { name: /^(Open|Hide) selection$/ });
  await expect(toggle).toBeVisible();
  if ((await toggle.getAttribute("aria-expanded")) !== "true") await toggle.click();
  return selection;
}
async function control(page: Page, path: string, data = {}) {
  const response = await page.request.post(path, { headers: guards, data });
  expect(response.ok(), await response.text()).toBe(true);
}

async function submitCommand(page: Page, path: string, button: string) {
  const [response] = await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === "POST" && new URL(response.url()).pathname === path,
    ),
    page.getByRole("button", { name: button, exact: true }).click(),
  ]);
  expect(response.status()).toBe(204);
}

async function resumePublication(page: Page) {
  await page.getByRole("button", { name: "Review publication recovery", exact: true }).click();
  await submitCommand(
    page,
    "/api/jobs/1/plans/2/publication/resume",
    "Confirm publication recovery",
  );
}

test.beforeEach(async ({ page, browser }) => {
  test.info().annotations.push({ type: "browser-version", description: browser.version() });
  await control(page, "/__fixture/reset");
  await page.route("**/*", (route) =>
    new URL(route.request().url()).origin === origin ? route.continue() : route.abort(),
  );
});

async function selectAlbums(page: Page, count = 1, backup = false, profile?: AudioProfile) {
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
  if (profile)
    await selection.getByLabel("Audio profile for Browser Album 1").selectOption(profile);
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

async function verifyOutput(
  page: Page,
  job = 1,
  {
    multiDisc = false,
    format = "mp3",
    backup = false,
    pending = false,
  }: { multiDisc?: boolean; format?: "mp3" | "flac"; backup?: boolean; pending?: boolean } = {},
) {
  const response = await page.request.get(`/__fixture/verify/${job}/2?backup_pending=${pending}`);
  expect(response.ok(), await response.text()).toBe(true);
  const evidence: {
    audio_unchanged: boolean;
    tags_verified: boolean;
    exact_jpeg: boolean;
    state: string;
    files: string[];
    destinations: PublicationDestination[];
  } = await response.json();
  expect(evidence.audio_unchanged).toBe(true);
  expect(evidence.tags_verified).toBe(true);
  expect(evidence.exact_jpeg).toBe(true);
  expect(evidence.state).toBe(pending ? "backup_pending" : "complete");
  expect(evidence.destinations.map((destination) => destination.location_id).sort()).toEqual(
    backup ? [1, 2] : [1],
  );
  const verified = evidence.destinations.filter((destination) => destination.verified_at !== null);
  expect(verified.map((destination) => destination.location_id).sort()).toEqual(
    backup && !pending ? [1, 2] : [1],
  );
  const titles = multiDisc ? ["First Light", "Second Signal", "Third Return"] : ["Generated Track"];
  const files = titles.map(
    (title, track) =>
      `B/BROWSER ARTIST/BROWSER ALBUM ${job}/Browser Artist - Browser Album ${job} - ${String(track + 1).padStart(2, "0")} - ${title}.${format}`,
  );
  expect(evidence.files.sort()).toEqual(verified.flatMap(() => files).sort());
  return evidence;
}

for (const format of ["mp3", "flac"] as const) {
  for (const backup of [false, true]) {
    test(`complete two-disc ${format} album verifies exact output in ${backup ? "main and backup" : "main"}`, async ({
      page,
    }) => {
      await control(page, "/__fixture/reset", { format, multi_disc: true });
      await selectAlbums(page, 1, backup, format === "mp3" ? "mp3_cbr320" : "flac_lossless");
      await approveAlbum(page);
      if (backup) await control(page, "/__fixture/backup", { offline: true });
      await submitCommand(page, "/api/jobs/1/plans/2/run", "Start Download");
      if (backup) {
        await expect(
          page.getByText("The primary is verified. The backup is not complete.", { exact: false }),
        ).toBeVisible();
        await verifyOutput(page, 1, { multiDisc: true, format, backup, pending: true });
        const premature = await page.request.get("/__fixture/verify/1/2");
        expect(premature.status()).toBe(409);
        await control(page, "/__fixture/backup", { offline: false });
        await resumePublication(page);
      }
      await expect(
        page.getByText("The server reports this request complete.", { exact: false }),
      ).toBeVisible();
      await verifyOutput(page, 1, { multiDisc: true, format, backup });
      await control(page, "/__fixture/scan");
      await page.goto("/#/library?location=1&view=artists");
      await page.getByRole("link", { name: "Browser Artist", exact: true }).click();
      await page.locator(".library-list a").filter({ hasText: "Browser Album 1" }).click();
      for (const title of ["First Light", "Second Signal", "Third Return"])
        await expect(
          page.getByRole("link", { name: new RegExp(`${title}\\.${format}$`) }),
        ).toBeVisible();
      await page.getByRole("link", { name: new RegExp(`Third Return\\.${format}$`) }).click();
      await expect(page.getByRole("heading", { name: "Library file", exact: true })).toBeVisible();
      await expect(page.getByText("Third Return", { exact: true })).toBeVisible();
      await verifyOutput(page, 1, { multiDisc: true, format, backup });
    });
  }
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
  await expect(
    page.getByRole("heading", { name: "Your download queue", exact: true }),
  ).toBeVisible();
  await expect(
    selection.getByRole("button", { name: "Open selection", exact: true }),
  ).toBeVisible();
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
  await verifyOutput(page, 1, { backup: true, pending: true });
  await control(page, "/__fixture/backup", { offline: false });
  await resumePublication(page);
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

test("host configuration is read-only and old write routes preserve startup state", async ({
  page,
}) => {
  await page.goto("/#/settings");
  const initial: BackendConfigurationState = await (
    await page.request.get("/api/system/configuration")
  ).json();
  await expect(
    page.getByText("Other host settings are managed by the operator.", { exact: false }),
  ).toBeVisible();
  await expect(page.getByRole("textbox", { name: /^Listen address/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Save changes", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Reload/ })).toHaveCount(0);
  for (const endpoint of ["save", "sections/save", "reload"]) {
    const rejected = await page.request.post(`/api/system/configuration/${endpoint}`, {
      headers: guards,
      data: {
        expected_revision: initial.saved_revision,
        sections: [
          {
            section: "general",
            environment: "single_user",
            listen: "127.0.0.1:4175",
            origin: "http://127.0.0.1:4175",
            browser_assets: null,
          },
        ],
      },
    });
    expect(rejected.status()).toBe(410);
  }
  const saved: BackendConfigurationState = await (
    await page.request.get("/api/system/configuration")
  ).json();
  expect(saved.active_revision).toBe(initial.active_revision);
  expect(saved.active.origin).toBe(origin);
  expect(saved.saved).toEqual(initial.saved);
  expect(saved.restart_required).toBe(false);
  expect(saved.saved_revision).toBe(initial.saved_revision);
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
  await approveAlbum(page);
  await page.getByRole("button", { name: "Pause at a safe boundary", exact: true }).click();
  await page.getByRole("button", { name: "Refresh request", exact: true }).click();
  await expect(page.getByText("Action required:", { exact: false })).toBeVisible();
  await page.getByRole("link", { name: "Open the saved decision", exact: true }).click();
  await expect(page.getByRole("heading", { name: /Decision #/ })).toBeVisible();
  await expect(page.getByText("Nothing is", { exact: false }).first()).toBeVisible();
  const questionId = page.url().split("/").at(-1);
  const details: QuestionDetails = await (
    await page.request.get(`/api/questions/${questionId}`)
  ).json();
  expect(details.recovery.kind).toBe("job_control");
  expect(details.question.answer).toBeNull();
  const stale = await page.request.post(`/api/questions/${questionId}/answer`, {
    headers: guards,
    data: { plan_version: 1, choice: "Resume" },
  });
  expect(stale.status()).toBe(409);
  expect(
    (await (await page.request.get(`/api/questions/${questionId}`)).json()).question.answer,
  ).toBeNull();
  await page.getByLabel("Your answer").selectOption("Resume");
  await page.getByRole("button", { name: "Save answer", exact: true }).click();
  await expect(page.getByText("Saved answer:", { exact: true })).toBeVisible();
  const answered: QuestionDetails = await (
    await page.request.get(`/api/questions/${questionId}`)
  ).json();
  expect(answered.job_state).toBe("awaiting_input");
  expect(answered.question.answer).toBe("Resume");
  if (answered.recovery.kind !== "job_control") throw new Error("Expected saved pause recovery");
  expect(answered.recovery.control.continued_at).toBeNull();
  expect(await (await page.request.get("/api/jobs/1/plans/2/transfers")).json()).toEqual([]);
  const staleContinuation = await page.request.post(
    `/api/job-controls/${answered.recovery.control.id}/continue`,
    {
      headers: guards,
      data: { plan_version: 1 },
    },
  );
  expect(staleContinuation.status()).toBe(409);
  await page.getByRole("button", { name: "Apply saved pause answer", exact: true }).click();
  await expect(
    page.getByText("This decision is no longer active.", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Apply saved pause answer", exact: true }),
  ).toHaveCount(0);
  const resumed: QuestionDetails = await (
    await page.request.get(`/api/questions/${questionId}`)
  ).json();
  expect(resumed.job_state).toBe("queued");
  if (resumed.recovery.kind !== "job_control") throw new Error("Expected saved pause recovery");
  expect(resumed.recovery.control.continued_at).not.toBeNull();
  expect(await (await page.request.get("/api/jobs/1/plans/2/transfers")).json()).toEqual([]);
  expect(await (await page.request.get("/api/jobs/1/plans/2/publication")).json()).toEqual([]);
});

test("a changed cover approval is rejected without transfers or publication", async ({ page }) => {
  await selectAlbums(page);
  await page.route("**/api/jobs/1/plans/2/artwork/approve", (route) => {
    const approval: ApproveArtwork = route.request().postDataJSON();
    approval.sha256 = "0".repeat(64);
    return route.continue({ postData: JSON.stringify(approval) });
  });
  await approveAlbum(page, true);
  await expect(page.getByRole("alert")).toBeVisible();
  const artwork: { approved_at: number | null }[] = await (
    await page.request.get("/api/jobs/1/plans/2/artwork")
  ).json();
  expect(artwork).toHaveLength(1);
  expect(artwork[0]!.approved_at).toBeNull();
  expect(await (await page.request.get("/api/jobs/1/plans/2/transfers")).json()).toEqual([]);
  expect(await (await page.request.get("/api/jobs/1/plans/2/publication")).json()).toEqual([]);
  await page.unroute("**/api/jobs/1/plans/2/artwork/approve");
  await page.getByRole("button", { name: "Approve this exact cover", exact: true }).click();
  await expect(page.getByRole("button", { name: "Start Download", exact: true })).toBeEnabled();
  expect(await (await page.request.get("/api/jobs/1/plans/2/transfers")).json()).toEqual([]);
});

test("tag rules freeze exact album review and keep approval separate from local work", async ({
  page,
}) => {
  await page.goto("/#/settings");
  if (test.info().project.name === "narrow")
    await page
      .getByRole("combobox", { name: "Settings section", exact: true })
      .selectOption("music_output");
  else await page.getByRole("button", { name: "Output policy", exact: true }).click();
  await page
    .getByRole("combobox", { name: "Copyright action", exact: true })
    .selectOption("retain");
  await expect(page.getByRole("combobox", { name: "Copyright mode", exact: true })).toHaveCount(0);
  await page.getByRole("combobox", { name: "Composer action", exact: true }).selectOption("update");
  await page.getByRole("textbox", { name: /^Composer value/ }).fill("  Exact composer  ");
  await page.getByRole("button", { name: "Save system output settings" }).click();
  await expect(page.getByText("Output policy saved.", { exact: false })).toBeVisible();
  await selectAlbums(page);
  await expect(
    page.getByText('Composer: update to "  Exact composer  "', { exact: false }),
  ).toBeVisible();
  await approveAlbum(page);
  await page.getByRole("button", { name: "Start Download", exact: true }).click();
  const review = page
    .getByRole("heading", { name: "Exact downloaded tag review", exact: true })
    .locator("..");
  await expect(
    review.getByRole("button", { name: "Approve exact tags for this album" }),
  ).toBeEnabled();
  await expect(review).toContainText("Not inspected for automatic rule");
  await review.getByRole("button", { name: "Approve exact tags for this album" }).click();
  await expect(review.getByRole("button", { name: "Apply saved tag approval" })).toBeEnabled();
  const approved: { approved: boolean; resumed: boolean } = await (
    await page.request.get("/api/jobs/1/plans/2/tags")
  ).json();
  expect(approved).toMatchObject({ approved: true, resumed: false });
  await review.getByRole("button", { name: "Apply saved tag approval" }).click();
  await expect(page.getByText("Tag approval applied.", { exact: false })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Continue approved local work", exact: true }),
  ).toBeEnabled();
  expect(await (await page.request.get("/api/jobs/1/plans/2/publication")).json()).toEqual([]);
  await page.getByRole("button", { name: "Continue approved local work", exact: true }).click();
  await verifyOutput(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test("settings opens as a fixed-width modal and returns focus to the previous page", async ({
  page,
}) => {
  await page.goto("/#/library");
  const opener = page.getByRole("link", { name: "System settings", exact: true });
  await opener.focus();
  await opener.press("Enter");
  const dialog = page.getByRole("dialog", { name: "System Settings", exact: true });
  await expect(dialog).toBeVisible();
  expect(await dialog.evaluate((element) => element.matches(":modal"))).toBe(true);
  const size = () =>
    dialog.evaluate((element) => ({
      width: element.getBoundingClientRect().width,
      viewport: window.innerWidth,
      fixed: 64 * parseFloat(getComputedStyle(document.documentElement).fontSize),
    }));
  const { width, viewport, fixed } = await size();
  expect(width).toBeCloseTo(Math.min(fixed, viewport - 24));
  if (viewport > fixed + 224) {
    await page.setViewportSize({ width: viewport - 200, height: 900 });
    expect((await size()).width).toBeCloseTo(width);
  }
  await expect(dialog.getByRole("button", { name: "Close settings" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(/#\/library$/);
  await expect(opener).toBeFocused();
  await page.goto("/?direct-settings=1#/settings?section=music_output");
  await dialog.getByRole("button", { name: "Close settings" }).click();
  await expect(page).toHaveURL(/#\/$/);
});

test("settings panes scroll independently and keep navigation in view", async ({ page }) => {
  const narrow = test.info().project.name === "narrow";
  await page.setViewportSize({ width: narrow ? 375 : 1280, height: 480 });
  await page.goto("/#/settings?section=music_output");
  const dialog = page.getByRole("dialog", { name: "System Settings", exact: true });
  const close = dialog.getByRole("button", { name: "Close settings" });
  const title = dialog.getByRole("heading", { name: "System Settings", level: 1 });
  const band = title.locator("..");
  const sidebar = dialog.getByRole("navigation", { name: "Configuration sections" });
  const picker = dialog.getByRole("combobox", { name: "Settings section", exact: true });
  const content = dialog.getByRole("region", { name: "Settings content" });
  const footer = dialog.getByRole("group", { name: "Settings save actions" });
  const save = footer.getByRole("button", { name: "Save system output settings" });
  const scrollTop = (pane: typeof content) => pane.evaluate((element) => element.scrollTop);
  const bounds = await dialog.boundingBox();
  expect(bounds?.height).toBeCloseTo(360);
  expect(bounds?.y).toBeCloseTo(60);
  const bandBounds = (await band.boundingBox())!;
  expect(bandBounds.x).toBeCloseTo(bounds!.x + 1);
  expect(bandBounds.width).toBeCloseTo(bounds!.width - 2);
  expect(bandBounds.y).toBeCloseTo(bounds!.y + 1);
  expect(
    await title.evaluate((element) => parseFloat(getComputedStyle(element).fontSize)),
  ).toBeGreaterThan(
    await dialog.evaluate((element) => parseFloat(getComputedStyle(element).fontSize)),
  );
  const chromeBefore = await close.boundingBox();
  const pickerBefore = narrow ? await picker.boundingBox() : null;
  await expect(save).toBeVisible();
  const saveBefore = await save.boundingBox();
  const footerBefore = await footer.boundingBox();
  const contentBefore = await content.boundingBox();
  expect(contentBefore?.height).toBeGreaterThanOrEqual(40);
  expect(saveBefore?.width).toBe(224);
  expect(saveBefore?.height).toBe(36);
  await expect(save.locator(".button-icon svg")).toBeVisible();
  expect(footerBefore!.x + footerBefore!.width - saveBefore!.x - saveBefore!.width).toBe(12);
  expect(footerBefore!.x).toBe(contentBefore!.x);
  expect(footerBefore!.width).toBe(contentBefore!.width);
  expect(footerBefore!.y).toBe(contentBefore!.y + contentBefore!.height);
  const layout = await dialog.locator(".configuration-layout").boundingBox();
  expect(footerBefore!.y + footerBefore!.height).toBe(layout!.y + layout!.height - 1);
  const header = content.getByRole("heading", { name: "Output policy", exact: true }).locator("..");
  const gradient = await header.evaluate((element) => getComputedStyle(element).backgroundImage);
  expect(gradient).toMatch(/^linear-gradient\(/);
  await expect(band).toHaveCSS("background-image", gradient);
  await expect(footer).toHaveCSS(
    "background-image",
    gradient.replace(/^linear-gradient\((?:180deg, )?/, "linear-gradient(0deg, "),
  );

  await content.focus();
  const bottom = await content.evaluate((element) => element.scrollHeight - element.clientHeight);
  expect(bottom).toBeGreaterThan(100);
  await content.press("End");
  await expect.poll(() => scrollTop(content)).toBeCloseTo(bottom, 0);
  expect(await save.boundingBox()).toEqual(saveBefore);
  await page.screenshot({ path: test.info().outputPath("settings-save-footer.png") });
  if (narrow) {
    await expect(sidebar).toBeHidden();
    expect(await picker.boundingBox()).toEqual(pickerBefore);
    await picker.selectOption("general");
  } else {
    expect(await scrollTop(sidebar)).toBe(0);
    await content.hover();
    await page.mouse.wheel(0, -100);
    await expect.poll(() => scrollTop(content)).toBeCloseTo(bottom - 100, 0);
    await sidebar.hover();
    await page.mouse.wheel(0, 250);
    await expect.poll(() => scrollTop(sidebar)).toBeGreaterThan(0);
    expect(await scrollTop(content)).toBeCloseTo(bottom - 100, 0);
    await sidebar.getByRole("button", { name: "General", exact: true }).click();
  }
  await expect(content.getByRole("heading", { name: "General", exact: true })).toBeVisible();
  await expect(footer).toBeHidden();
  await expect.poll(() => scrollTop(content)).toBe(0);
  expect(await dialog.boundingBox()).toEqual(bounds);
  expect(await close.boundingBox()).toEqual(chromeBefore);
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
  const profiles = page.getByRole("group", { name: "Accepted audio profiles" });
  const rows = profiles.getByRole("switch");
  await expect(rows.first()).toBeVisible();
  const toggle = rows.first();
  const wasChecked = await toggle.isChecked();
  const save = page.getByRole("button", { name: "Save system output settings" });
  await expect(save).toBeDisabled();
  await toggle.press("Space");
  await expect(toggle).toBeChecked({ checked: !wasChecked });
  await expect(save).toBeEnabled();
  await toggle.press("Space");
  await expect(toggle).toBeChecked({ checked: wasChecked });
  await expect(save).toBeDisabled();
  const layout = await rows.evaluateAll((inputs) =>
    inputs.map((input) => {
      const box = input.getBoundingClientRect();
      const text = (input as HTMLInputElement).labels![0]!.getBoundingClientRect();
      const row = input.closest('[data-slot="settings-row"]')!.getBoundingClientRect();
      return {
        offset: box.y + box.height / 2 - text.y - text.height / 2,
        top: row.top,
        bottom: row.bottom,
        rightGap: row.right - box.right,
        height: row.height,
        labelHeight: text.height,
      };
    }),
  );
  expect(layout.length).toBeGreaterThan(1);
  for (const [index, row] of layout.entries()) {
    expect(row.offset).toBeCloseTo(0);
    expect(row.rightGap).toBeCloseTo(0);
    expect(row.height).toBeCloseTo(Math.max(24, row.labelHeight));
    if (index > 0) expect(row.top - layout[index - 1]!.bottom).toBeCloseTo(0);
  }
  await page.screenshot({ path: test.info().outputPath("output-policy.png") });
  const genreAction = page.getByRole("combobox", { name: "Genre action", exact: true });
  const genreMode = page.getByRole("combobox", { name: "Genre mode", exact: true });
  await expect(genreAction).toHaveAccessibleDescription(
    "Remove, keep, or replace the source genre tag.",
  );
  const actionLayout = await genreAction.evaluate((control: HTMLSelectElement) => {
    const label = control.labels![0]!.getBoundingClientRect();
    const hint = document.getElementById(control.getAttribute("aria-describedby")!)!;
    const box = control.getBoundingClientRect();
    return {
      besideLabel: box.left >= label.right,
      besideHint: box.left >= hint.getBoundingClientRect().right,
      right: box.right,
    };
  });
  expect(actionLayout.right).toBeCloseTo(
    await genreMode.evaluate((control) => control.getBoundingClientRect().right),
  );
  expect(actionLayout.besideLabel).toBe(true);
  expect(actionLayout.besideHint).toBe(true);
  const template = page.getByRole("textbox", { name: "Album folder template", exact: true });
  expect(
    await template.evaluate((control: HTMLInputElement) => {
      const caption = control.labels![0]!.getBoundingClientRect();
      const box = control.getBoundingClientRect();
      const hint = document.getElementById(control.getAttribute("aria-describedby")!)!;
      return box.top >= caption.bottom && box.bottom <= hint.getBoundingClientRect().top;
    }),
  ).toBe(true);
  const settings: SettingsVersion = await (await page.request.get("/api/settings")).json();
  const artwork = page.getByRole("combobox", { name: "Artwork size", exact: true });
  const previousSize = await artwork.inputValue();
  const nextSize = previousSize === "large" ? "recommended" : "large";
  await artwork.selectOption(nextSize);
  await expect(save).toBeEnabled();
  await save.click();
  await expect(
    page.getByText(`New plans use version ${settings.version + 1}`, { exact: false }),
  ).toBeVisible();
  await expect(artwork).toBeEnabled();
  await expect(artwork).toHaveValue(nextSize);
  await expect(save).toBeDisabled();
  const persisted: SettingsVersion = await (await page.request.get("/api/settings")).json();
  expect(persisted.settings.artwork_size).toBe(nextSize);
  await artwork.selectOption(previousSize === "legacy" ? "small" : previousSize);
  await expect(save).toBeEnabled();
  await artwork.selectOption(nextSize);
  await expect(save).toBeDisabled();
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
  await expect(page.getByRole("combobox", { name: "Genre action", exact: true })).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Frozen optional tag rules", exact: true }),
  ).toBeVisible();
  const rejected = await page.request.put("/api/settings", {
    headers: guards,
    data: { expected_version: settings.version + 1, settings: settings.settings },
  });
  expect(rejected.status()).toBe(409);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});
