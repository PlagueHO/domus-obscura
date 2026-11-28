import { expect, test } from "@playwright/test";

test("Soulseek account is saved once, cleared, and applied only by a separate restart", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({
    width: testInfo.project.name === "narrow" ? 375 : 1280,
    height: 900,
  });
  let saved = false;
  let active = false;
  let saves = 0;
  let activations = 0;
  const change = () => ({
    id: 18,
    state: active ? "active" : "approved",
    configuration_revision: active ? "b".repeat(64) : null,
    last_error: null,
  });
  await page.route("**/api/system/soulseek-account**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/soulseek-account")) {
      await route.fulfill({
        json: {
          available: true,
          reason: null,
          configured: active,
          active_revision: (active ? "b" : "a").repeat(64),
          pending_change_id: saved && !active ? 18 : null,
        },
      });
    } else if (path.endsWith("/activate")) {
      activations++;
      active = true;
      await route.fulfill({ status: 202, json: null });
    } else if (path.endsWith("/changes")) {
      saves++;
      expect(route.request().headers()["x-domus-csrf"]).toBe("1");
      expect(route.request().postDataJSON()).toEqual({
        expected_revision: "a".repeat(64),
        username: "generated-browser-user",
        password: "generated-browser-password",
      });
      saved = true;
      await route.fulfill({ json: change() });
    } else {
      await route.fulfill({ json: change() });
    }
  });
  await page.goto("/#/settings");
  if (testInfo.project.name === "narrow") {
    const picker = page.getByRole("combobox", { name: "Settings section" });
    await expect(picker.locator('option[value="music_soulseek"]')).toHaveCount(0);
    await picker.selectOption("music_acquisition");
  } else {
    const nav = page.getByRole("navigation", { name: "Configuration sections" });
    await expect(nav.getByRole("button", { name: "Soulseek account" })).toHaveCount(0);
    await nav.getByRole("button", { name: "Acquisition", exact: true }).click();
  }
  await expect(page.getByRole("heading", { name: "Acquisition", exact: true })).toBeVisible();
  const card = page.locator('[data-slot="header-card"]').filter({
    has: page.getByRole("heading", { name: "Soulseek account", exact: true }),
  });
  const form = card.getByRole("region", { name: "Soulseek account credentials" });
  const save = page
    .getByRole("group", { name: "Settings save actions" })
    .getByRole("button", { name: "Save account", exact: true });
  await expect(form.getByLabel("Soulseek username", { exact: true })).toBeEnabled();
  await expect(form.getByLabel("Soulseek password", { exact: true })).toHaveAttribute(
    "type",
    "password",
  );
  await expect(form.getByRole("button", { name: "Check account status" })).toHaveCount(0);
  await expect(save).toBeDisabled();
  await form.getByLabel("Soulseek username", { exact: true }).fill("generated-browser-user");
  await expect(save).toBeDisabled();
  await form.getByLabel("Soulseek password", { exact: true }).fill("generated-browser-password");
  await expect(save).toBeEnabled();
  await page.screenshot({
    path: test.info().outputPath("soulseek-account-form.png"),
    fullPage: true,
  });
  expect((await save.boundingBox())?.width).toBe(224);
  await save.click();
  await expect(form.getByText("Account saved, not yet active", { exact: true })).toBeVisible();
  await expect(form.getByLabel("Soulseek password", { exact: true })).toHaveCount(0);
  expect(saves).toBe(1);
  expect(activations).toBe(0);
  await form.getByRole("button", { name: "Restart and apply account", exact: true }).click();
  await expect(
    form.getByText("Account applied. No login was started.", { exact: true }),
  ).toBeVisible();
  await expect(form.getByLabel("Soulseek password", { exact: true })).toHaveValue("");
  await expect(save).toBeDisabled();
  expect(activations).toBe(1);
  expect(saves).toBe(1);
  if (testInfo.project.name === "narrow") {
    await page.getByRole("combobox", { name: "Settings section" }).selectOption("general");
  } else {
    await page
      .getByRole("navigation", { name: "Configuration sections" })
      .getByRole("button", { name: "General", exact: true })
      .click();
  }
  await expect(card).toHaveCount(0);
  await expect(save).toHaveCount(0);
});

test("lost account-save response is reconciled without resubmitting the password", async ({
  page,
}) => {
  let saved = false;
  let saves = 0;
  let state = "approved";
  await page.route("**/api/system/soulseek-account**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/soulseek-account")) {
      await route.fulfill({
        json: {
          available: true,
          reason: null,
          configured: false,
          active_revision: "a".repeat(64),
          pending_change_id: saved && state === "approved" ? 21 : null,
        },
      });
    } else if (path.endsWith("/changes")) {
      saves++;
      saved = true;
      await route.abort("connectionreset");
    } else {
      await route.fulfill({
        json: { id: 21, state, configuration_revision: null, last_error: null },
      });
    }
  });
  await page.goto("/#/settings?section=music_soulseek");
  const form = page.getByRole("region", { name: "Soulseek account credentials" });
  await expect(form.getByLabel("Soulseek username", { exact: true })).toBeEnabled();
  await form.getByLabel("Soulseek username", { exact: true }).fill("generated-loss-user");
  await form.getByLabel("Soulseek password", { exact: true }).fill("generated-loss-password");
  await page
    .getByRole("group", { name: "Settings save actions" })
    .getByRole("button", { name: "Save account", exact: true })
    .click();
  await expect(form.getByText("The save result is not confirmed.", { exact: false })).toBeVisible();
  await form.getByRole("button", { name: "Check account status", exact: true }).click();
  await expect(form.getByText("Account saved, not yet active", { exact: true })).toBeVisible();
  await expect(
    form.getByRole("button", { name: "Restart and apply account", exact: true }),
  ).toBeEnabled();
  expect(saves).toBe(1);
  state = "failed";
  await form.getByRole("button", { name: "Check account status", exact: true }).click();
  await expect(
    form.getByText("Account change failed or was cancelled", { exact: true }),
  ).toBeVisible();
  await expect(form.getByLabel("Soulseek password", { exact: true })).toHaveValue("");
  expect(saves).toBe(1);
});
