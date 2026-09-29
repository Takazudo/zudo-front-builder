import { expect, test } from "@playwright/test";
import { openFixture } from "./helpers.mjs";

test("W-A02 keeps padding, margin, and gap values across every class permutation", async ({
  page,
}) => {
  await openFixture(page, "w-a02");

  const cases = [
    {
      kind: "padding",
      properties: {
        "padding-top": "16px",
        "padding-right": "8px",
        "padding-bottom": "16px",
        "padding-left": "4px",
      },
    },
    {
      kind: "margin",
      properties: {
        "margin-top": "16px",
        "margin-right": "8px",
        "margin-bottom": "16px",
        "margin-left": "4px",
      },
    },
    {
      kind: "gap",
      properties: {
        "column-gap": "8px",
        "row-gap": "4px",
      },
    },
  ];

  for (const { kind, properties } of cases) {
    const elements = page.locator(`[data-kind="${kind}"]`);
    await expect(elements).toHaveCount(6);
    for (const element of await elements.all()) {
      for (const [property, value] of Object.entries(properties)) {
        await expect(element).toHaveCSS(property, value);
      }
    }
  }
});
