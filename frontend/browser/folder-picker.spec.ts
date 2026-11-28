import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  const reset = await page.request.post("/__fixture/reset", {
    headers: { Origin: "http://127.0.0.1:4174", "x-domus-csrf": "1" },
    data: {},
  });
  expect(reset.ok()).toBe(true);
});

test("folder selection uses the backend, pages safely, and does not approve a library", async ({
  page,
}) => {
  const root = "D:\\Generated-folders";
  const album = `${root}\\Albums`;
  let inspections = 0;
  let closes = 0;
  await page.route("**/api/library/primary/**", async (route) => {
    if (route.request().url().endsWith("/control")) {
      await route.fulfill({ json: { available: true, reason: null, pending_change_id: null } });
    } else {
      inspections++;
      await route.fulfill({ status: 409, json: { error: "Separate inspection required." } });
    }
  });
  await page.route("**/api/system/folders/**", async (route) => {
    if (route.request().url().endsWith("/roots")) {
      await route.fulfill({ json: ["D:\\"] });
      return;
    }
    expect(route.request().headers()["x-domus-csrf"]).toBe("1");
    const body = route.request().postDataJSON();
    if (route.request().url().endsWith("/close")) {
      closes++;
      await route.fulfill({ status: 204 });
      return;
    }
    if (body.path === album) {
      expect(body.expected_identity).toBe("album-id");
      await route.fulfill({
        json: {
          path: album,
          identity: "album-id",
          parent_path: root,
          entries: [],
          next_cursor: null,
        },
      });
    } else {
      expect(body.path).toBe(root);
      await route.fulfill({
        json: {
          path: root,
          identity: "root-id",
          parent_path: "D:\\",
          entries: body.cursor
            ? [
                {
                  name: "Backup",
                  path: `${root}\\Backup`,
                  identity: "backup-id",
                  unavailable_reason: null,
                },
              ]
            : [
                { name: "Albums", path: album, identity: "album-id", unavailable_reason: null },
                {
                  name: "Private storage",
                  path: `${root}\\private`,
                  identity: null,
                  unavailable_reason: "Private application storage.",
                },
              ],
          next_cursor: body.cursor ? null : "page-2",
        },
      });
    }
  });
  await page.goto("/#/settings?section=music_roots");
  await page.getByRole("button", { name: "Change…", exact: true }).click();
  await page.getByRole("button", { name: "Browse…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Choose a folder" });
  await dialog.getByLabel("Host folder path").fill(root);
  await dialog.getByRole("button", { name: "Open folder", exact: true }).click();
  await expect(
    dialog.getByRole("treeitem", { name: "Private storage", exact: false }),
  ).toHaveAttribute("aria-disabled", "true");
  await dialog.getByRole("button", { name: "Load more folders" }).click();
  await expect(dialog.getByRole("treeitem", { name: "Backup", exact: true })).toBeVisible();
  await expect(dialog.getByRole("treeitem", { name: "Albums", exact: true })).toBeInViewport({
    ratio: 1,
  });
  await expect(dialog.getByRole("button", { name: "Use this folder", exact: true })).toBeInViewport(
    { ratio: 1 },
  );
  await page.screenshot({ path: test.info().outputPath("folder-picker.png") });
  await dialog.getByRole("treeitem", { name: "Albums", exact: true }).click();
  await expect(dialog.getByLabel("Host folder path")).toHaveValue(album);
  await dialog.getByRole("button", { name: "Use this folder", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByLabel("Primary library folder")).toHaveValue(album);
  expect(inspections).toBe(0);
  expect(closes).toBe(0);
});

test("a failed refresh clears selection and closing releases the retained listing", async ({
  page,
}) => {
  await page.route("**/api/library/primary/control", (route) =>
    route.fulfill({ json: { available: true, reason: null, pending_change_id: null } }),
  );
  let reads = 0;
  let closes = 0;
  await page.route("**/api/system/folders/**", async (route) => {
    if (route.request().url().endsWith("/roots")) {
      await route.fulfill({ json: ["D:\\"] });
      return;
    }
    if (route.request().url().endsWith("/close")) {
      closes++;
      await route.fulfill({ status: 204 });
    } else if (reads++ === 0) {
      await route.fulfill({
        json: {
          path: "D:\\Generated",
          identity: "root-id",
          parent_path: "D:\\",
          entries: [],
          next_cursor: "page-2",
        },
      });
    } else {
      await route.fulfill({
        status: 409,
        json: { error: "Folder contents changed. Reload this folder." },
      });
    }
  });
  await page.goto("/#/settings?section=music_roots");
  await page.getByRole("button", { name: "Change…", exact: true }).click();
  await page.getByRole("button", { name: "Browse…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Choose a folder" });
  await dialog.getByLabel("Host folder path").fill("D:\\Generated");
  await dialog.getByRole("button", { name: "Open folder", exact: true }).click();
  await dialog.getByRole("button", { name: "Reload folder", exact: true }).click();
  await expect(
    dialog.getByText("Folder contents changed. Reload this folder.", { exact: true }),
  ).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Use this folder", exact: true })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("dialog", { name: "System Settings" })).toBeVisible();
  expect(closes).toBe(1);
});

for (const [root, separator, drive] of [
  ["\\\\generated-server\\Music", "\\", "D:\\"],
  ["/mnt/generated-music", "/", "/"],
] as const) {
  test(`open, retry and select a host-visible share: ${root}`, async ({ page }) => {
    const child = `${root}${separator}Albums`;
    const requests: { path: string; expected_identity: string | null }[] = [];
    let inspections = 0;
    await page.route("**/api/library/primary/**", async (route) => {
      if (route.request().url().endsWith("/control")) {
        await route.fulfill({ json: { available: true, reason: null, pending_change_id: null } });
      } else {
        inspections++;
        await route.fulfill({ status: 409, json: { error: "Separate inspection required." } });
      }
    });
    await page.route("**/api/system/folders/roots", (route) => route.fulfill({ json: [drive] }));
    await page.route("**/api/system/folders/browse", async (route) => {
      const { path, expected_identity } = route.request().postDataJSON();
      requests.push({ path, expected_identity });
      expect(route.request().headers()["x-domus-csrf"]).toBe("1");
      if (requests.length === 1) {
        await route.fulfill({ status: 422, json: { error: "The share is offline." } });
        return;
      }
      const atRoot = path === root;
      await route.fulfill({
        json: {
          path,
          identity: atRoot ? "root-id" : "album-id",
          parent_path: atRoot ? null : root,
          entries: atRoot
            ? [{ name: "Albums", path: child, identity: "album-id", unavailable_reason: null }]
            : [],
          next_cursor: null,
        },
      });
    });
    await page.goto("/#/settings?section=music_roots");
    await page.getByRole("button", { name: "Change…", exact: true }).click();
    await page.getByRole("button", { name: "Browse…", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Choose a folder" });
    const path = dialog.getByLabel("Host folder path");
    await expect(path).toHaveAccessibleDescription(/Windows SMB.*Linux SMB or NFS/);
    await path.fill(root);
    const open = dialog.getByRole("button", { name: "Open folder", exact: true });
    await open.click();
    await expect(dialog.getByText("The share is offline.", { exact: true })).toBeVisible();
    await expect(path).toHaveValue(root);
    await expect(dialog.getByRole("button", { name: "Use this folder" })).toHaveCount(0);
    await open.click();
    await dialog.getByRole("treeitem", { name: "Albums", exact: true }).click();
    await dialog.getByRole("button", { name: "Use this folder", exact: true }).click();
    await expect(page.getByLabel("Primary library folder")).toHaveValue(child);
    expect(requests).toEqual([
      { path: root, expected_identity: null },
      { path: root, expected_identity: null },
      { path: child, expected_identity: "album-id" },
      { path: child, expected_identity: "album-id" },
    ]);
    expect(inspections).toBe(0);
  });
}
