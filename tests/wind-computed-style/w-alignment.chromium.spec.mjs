import { expect, test } from "@playwright/test";
import { openFixture, readReport } from "./helpers.mjs";

test("alignment roots apply longhands and position real grid items", async ({ page }) => {
  await openFixture(page, "w-alignment");

  const computed = await page.evaluate(() => {
    const style = (selector) => getComputedStyle(document.querySelector(selector));
    const box = (selector, container) => {
      const elementRect = document.querySelector(selector).getBoundingClientRect();
      const containerRect = document.querySelector(container).getBoundingClientRect();
      return {
        x: elementRect.left - containerRect.left,
        y: elementRect.top - containerRect.top,
        width: elementRect.width,
      };
    };
    return {
      justifyItems: style("#justify-items-grid").justifyItems,
      justifyItemsMarker: box("#justify-items-marker", "#justify-items-grid"),
      justifySelf: [
        style("#justify-self-start").justifySelf,
        style("#justify-self-end").justifySelf,
        style("#justify-self-center").justifySelf,
        style("#justify-self-stretch").justifySelf,
      ],
      justifySelfBoxes: [
        box("#justify-self-start", "#justify-self-start-grid"),
        box("#justify-self-end", "#justify-self-end-grid"),
        box("#justify-self-center", "#justify-self-center-grid"),
        box("#justify-self-stretch", "#justify-self-stretch-grid"),
      ],
      placeItems: [style("#place-items-grid").alignItems, style("#place-items-grid").justifyItems],
      placeItemsMarker: box("#place-items-marker", "#place-items-grid"),
      placeSelf: [style("#place-self-end").alignSelf, style("#place-self-end").justifySelf],
      placeSelfBox: box("#place-self-end", "#place-self-grid"),
      placeContent: [
        style("#place-content-grid").alignContent,
        style("#place-content-grid").justifyContent,
      ],
      contentBoxes: [
        box("#content-first", "#place-content-grid"),
        box("#content-second", "#place-content-grid"),
        box("#content-third", "#place-content-grid"),
        box("#content-fourth", "#place-content-grid"),
      ],
    };
  });

  expect(computed.justifyItems).toBe("center");
  expect(computed.justifyItemsMarker.x).toBe(40);
  expect(computed.justifySelf).toEqual(["start", "end", "center", "stretch"]);
  expect(computed.justifySelfBoxes.map(({ x }) => x)).toEqual([0, 80, 40, 0]);
  expect(computed.justifySelfBoxes[3].width).toBe(100);
  expect(computed.placeItems).toEqual(["center", "center"]);
  expect(computed.placeItemsMarker).toMatchObject({ x: 40, y: 25 });
  expect(computed.placeSelf).toEqual(["end", "end"]);
  expect(computed.placeSelfBox).toMatchObject({ x: 80, y: 50 });
  expect(computed.placeContent).toEqual(["space-between", "space-between"]);
  expect(computed.contentBoxes.map(({ x, y }) => [x, y])).toEqual([
    [0, 0],
    [80, 0],
    [0, 50],
    [80, 50],
  ]);
});

test("the alignment fixture resolves the added roots without diagnostics", async () => {
  const report = await readReport("w-alignment");
  for (const [candidate, entryIdentifier] of [
    ["justify-items-center", "v1.justify-items"],
    ["justify-self-start", "v1.justify-self"],
    ["justify-self-end", "v1.justify-self"],
    ["justify-self-center", "v1.justify-self"],
    ["justify-self-stretch", "v1.justify-self"],
    ["place-items-center", "v1.place-items"],
    ["place-self-end", "v1.place-self"],
    ["place-content-between", "v1.place-content"],
  ]) {
    expect(report.explanations[candidate]).toMatchObject({
      outcome: "resolved_utility",
      entryIdentifier,
    });
    expect(report.diagnostics.some((diagnostic) => diagnostic.candidate === candidate)).toBe(false);
  }
});
