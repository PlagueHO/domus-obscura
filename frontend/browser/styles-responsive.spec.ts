import { readFile } from "node:fs/promises";
import { expect, test, type Page } from "@playwright/test";

const stylesheet = await readFile(new URL("../app/styles.css", import.meta.url), "utf8");
const font = await readFile(new URL("../app/assets/BebasNeue-Regular.ttf", import.meta.url));
const css = stylesheet.replace(
  "./assets/BebasNeue-Regular.ttf",
  `data:font/ttf;base64,${font.toString("base64")}`,
);

async function collapsedTray(page: Page, current: number, count: number) {
  await page.setContent(`
    <style>${css}</style>
    <div style="height: 150vh"></div>
    <p id="last-content">Last page content</p>
    <section class="album-selection">
      <div class="tray-spacer" aria-hidden="true"></div>
      <div class="tray" data-open="false">
        <div class="tray-bar">
          <h2>Selected albums · ${count} / 25</h2>
          <ol class="step-rail" aria-label="Request steps">
            ${["Choose", "Prepare", "Approve each", "Start"]
              .map(
                (label, index) => `
                  <li data-state="${index === current ? "current" : index < current ? "done" : "next"}">
                    <span>${index < current ? "✓" : index + 1}</span>${label}
                  </li>`,
              )
              .join("")}
          </ol>
          <button class="button">Open selection</button>
        </div>
        <div class="tray-drawer" hidden></div>
      </div>
    </section>
  `);
  await page.evaluate(() => document.fonts.ready);
}

test("narrow tray overrides win the cascade and reserve the collapsed height", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "narrow", "Narrow stylesheet checks");
  for (const width of [320, 390, 700]) {
    await page.setViewportSize({ width, height: 844 });
    for (const count of [1, 25]) {
      for (let current = 0; current < 4; current++) {
        await collapsedTray(page, current, count);
        await expect(page.locator(".tray-bar")).toHaveCSS("flex-wrap", "nowrap");
        await expect(page.locator(".tray-bar h2")).toHaveCSS("font-size", "12.75px");
        await expect(page.locator(".step-rail")).toHaveCSS("flex-basis", "0px");
        await expect(page.locator(".step-rail")).toHaveCSS("overflow-x", "auto");
        await expect(page.locator(".step-rail li").first()).toHaveCSS("padding-left", "4px");
        await expect(page.locator('.step-rail li:not([data-state="current"])').first()).toHaveCSS(
          "font-size",
          "0px",
        );
        const geometry = await page.evaluate(() => {
          window.scrollTo(0, document.documentElement.scrollHeight);
          const bounds = (selector: string) =>
            document.querySelector(selector)!.getBoundingClientRect();
          const tray = bounds(".tray");
          const spacer = bounds(".tray-spacer");
          const title = bounds(".tray-bar h2");
          const rail = bounds(".step-rail");
          const button = bounds(".tray-bar .button");
          return {
            trayHeight: tray.height,
            spacerHeight: spacer.height,
            titleRight: title.right,
            railLeft: rail.left,
            railRight: rail.right,
            buttonLeft: button.left,
            buttonRight: button.right,
            viewportWidth: innerWidth,
            documentWidth: document.documentElement.scrollWidth,
            lastContentBottom: bounds("#last-content").bottom,
            trayTop: tray.top,
          };
        });
        expect(geometry.trayHeight).toBe(76);
        expect(geometry.spacerHeight).toBe(geometry.trayHeight);
        expect(geometry.titleRight).toBeLessThanOrEqual(geometry.railLeft);
        expect(geometry.railRight).toBeLessThanOrEqual(geometry.buttonLeft);
        expect(geometry.buttonRight).toBeLessThanOrEqual(width);
        expect(geometry.documentWidth).toBe(geometry.viewportWidth);
        expect(geometry.lastContentBottom).toBeLessThanOrEqual(geometry.trayTop);
      }
    }
  }
});

test("desktop tray keeps its incumbent typography and rail layout", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "desktop", "Desktop stylesheet checks");
  await collapsedTray(page, 2, 25);
  await expect(page.locator(".tray-bar")).toHaveCSS("flex-wrap", "wrap");
  await expect(page.locator(".tray-bar h2")).toHaveCSS("font-size", "22px");
  await expect(page.locator(".step-rail")).toHaveCSS("flex-basis", "320px");
  await expect(page.locator(".tray-spacer")).toHaveCSS("height", "84px");
  await expect(page.locator(".step-rail li").first()).toHaveCSS("padding-left", "10px");
});

test("the connected selection tray keeps its collapsed page clearance at 390px", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "narrow", "Connected narrow tray geometry");
  const reset = await page.request.post("/__fixture/reset", {
    headers: { Origin: "http://127.0.0.1:4174", "x-domus-csrf": "1" },
  });
  expect(reset.ok()).toBe(true);
  await page.route("**/*", (route) =>
    new URL(route.request().url()).origin === "http://127.0.0.1:4174"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/");
  await page.addStyleTag({ content: css });
  await page.getByRole("searchbox").fill("Browser");
  await page.getByRole("searchbox").press("Enter");
  await page.getByRole("button", { name: "Select Browser Album 1", exact: true }).click();
  await expect(page.getByRole("button", { name: "Open selection", exact: true })).toBeVisible();
  await expect(page.locator(".tray-drawer")).toBeHidden();
  await page.evaluate(() => document.fonts.ready);
  const tray = await page.locator(".tray").boundingBox();
  const spacer = await page.locator(".tray-spacer").boundingBox();
  expect(tray).not.toBeNull();
  expect(spacer).not.toBeNull();
  expect(tray!.height).toBe(76);
  expect(spacer!.height).toBe(tray!.height);
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  const selection = await page.locator(".album-choice").last().boundingBox();
  expect(selection).not.toBeNull();
  expect(selection!.y + selection!.height).toBeLessThanOrEqual(tray!.y);
});
