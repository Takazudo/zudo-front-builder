import { writeFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import { colors, createFixture, layerOrders, models, placements } from "./fixture.mjs";

for (const model of models) {
  for (const placement of placements) {
    for (const layerOrder of Object.keys(layerOrders)) {
      test(`${model} / ${placement} / ${layerOrder} @verification`, async ({
        page,
        browser,
      }, testInfo) => {
        const fixture = createFixture(model, placement, layerOrder);
        const measurements = [];
        const diagnostics = [];
        page.on("pageerror", (error) => diagnostics.push(String(error)));
        page.on("console", (message) => {
          if (message.type() === "error") diagnostics.push(message.text());
        });
        // Body-only attachments can be discarded by the list reporter on success.
        // Keep standalone files as well as reporter attachment references.
        const retain = async (name, body, contentType) => {
          const path = testInfo.outputPath(name);
          await writeFile(path, body);
          await testInfo.attach(name, { path, contentType });
        };
        await retain("synthetic-fixture.html", fixture.html, "text/html");
        await retain("synthetic-styles.css", fixture.css, "text/css");
        await page.setViewportSize({ width: 800, height: 720 });
        await page.setContent(fixture.html);
        const assertRows = async (phase, rows, active) => {
          const observed = await page.evaluate(
            (ids) =>
              Object.fromEntries(
                ids.map((id) => [id, getComputedStyle(document.getElementById(id)).borderTopColor]),
              ),
            rows.map((row) => row.id),
          );
          measurements.push({ phase, observed });
          for (const row of rows) {
            const expected = active ? row.expected : colors.authored;
            expect.soft(observed[row.id], `${phase}: ${row.id}`).toBe(expected);
          }
        };
        const relation = (name) => fixture.rows.filter((row) => row.relation === name);
        try {
          expect(await page.evaluate(() => matchMedia("(hover: hover)").matches)).toBe(true);
          await page.locator("#neutral").hover();
          await page.locator("#neutral").focus();
          await assertRows("plain", relation("plain"), true);
          await assertRows("group inactive", relation("group"), false);
          await assertRows("peer inactive", relation("peer"), false);
          await assertRows("media wide", relation("media"), true);
          await assertRows("dark inactive", relation("dark"), false);
          await page.locator("#group-owner").hover();
          await assertRows("group hover", relation("group"), true);
          await page.locator("#neutral").hover();
          await assertRows("group cleared", relation("group"), false);
          await page.locator("#peer-owner").focus();
          await assertRows("peer focus", relation("peer"), true);
          await page.locator("#neutral").focus();
          await assertRows("peer blur", relation("peer"), false);
          await page.setViewportSize({ width: 639, height: 720 });
          await assertRows("media narrow", relation("media"), false);
          await page.setViewportSize({ width: 640, height: 720 });
          await assertRows("media boundary", relation("media"), true);
          await page
            .locator("#theme")
            .evaluate((element) => element.setAttribute("data-theme", "dark"));
          await assertRows("dark ancestor", relation("dark"), true);
          await page.locator("#theme").evaluate((element) => element.removeAttribute("data-theme"));
          await assertRows("dark removed", relation("dark"), false);
          await page
            .locator("#theme > span")
            .evaluateAll((elements) =>
              elements.forEach((element) => element.setAttribute("data-theme", "dark")),
            );
          await assertRows("dark self", relation("dark"), true);
          await page
            .locator("#theme > span")
            .evaluateAll((elements) =>
              elements.forEach((element) => element.removeAttribute("data-theme")),
            );
          await assertRows("dark self removed", relation("dark"), false);
          await page.locator("#group-self").hover();
          await page.locator("#peer-self").focus();
          for (const id of fixture.controls) {
            await expect(page.locator(`#${id}`), `relation negative: ${id}`).toHaveCSS(
              "border-top-color",
              colors.inactive,
            );
          }
          await page.locator("#peer-owner").focus();
          for (const id of ["peer-before", "peer-nested"]) {
            await expect(page.locator(`#${id}`)).toHaveCSS("border-top-color", colors.inactive);
          }
          expect(diagnostics).toEqual([]);
        } finally {
          await retain(
            "computed-results.json",
            JSON.stringify(
              {
                synthetic: true,
                browser: browser.version(),
                executable:
                  process.env.CHROMIUM_EXECUTABLE_PATH ?? browser.browserType().executablePath(),
                platform: process.platform,
                ...fixture,
                measurements,
                diagnostics,
              },
              null,
              2,
            ),
            "application/json",
          );
          await retain("final-state.png", await page.screenshot({ fullPage: true }), "image/png");
        }
      });
    }
  }
}
