import { expect, test } from "@playwright/test";
import { openFixture } from "./helpers.mjs";

const resetFixtures = [
  { fixture: "w-a07-none", mode: "none" },
  { fixture: "w-a07-minimal", mode: "minimal-v1" },
  { fixture: "w-a07-owned", mode: "owned-v1" },
];

test("W-A07 checks native controls, headings, lists, focus, and border for every reset", async ({
  page,
}) => {
  for (const { fixture, mode } of resetFixtures) {
    await openFixture(page, fixture);

    const body = page.locator("body");
    const heading = page.locator("#heading");
    const list = page.locator("#list");
    const controls = [page.locator("#control"), page.locator("#input")];
    const bodyStyle = await body.evaluate((element) => {
      const style = getComputedStyle(element);
      return {
        fontFamily: style.fontFamily,
        fontSize: style.fontSize,
        fontWeight: style.fontWeight,
        letterSpacing: style.letterSpacing,
        color: style.color,
      };
    });

    if (mode === "owned-v1") {
      await expect(body).toHaveCSS("margin-top", "0px");
      await expect(heading).toHaveCSS("font-size", "20px");
      await expect(heading).toHaveCSS("font-weight", "400");
      await expect(heading).toHaveCSS("margin-block-start", "0px");
      await expect(list).toHaveCSS("list-style-type", "none");
      await expect(list).toHaveCSS("padding-left", "0px");
    } else {
      const headingStyle = await heading.evaluate((element) => {
        const style = getComputedStyle(element);
        return { fontSize: style.fontSize, fontWeight: style.fontWeight };
      });
      const listStyle = await list.evaluate((element) => {
        const style = getComputedStyle(element);
        return { marker: style.listStyleType, paddingLeft: style.paddingLeft };
      });
      expect(headingStyle.fontSize).not.toBe(bodyStyle.fontSize);
      expect(headingStyle.fontWeight).not.toBe(bodyStyle.fontWeight);
      expect(listStyle.marker).not.toBe("none");
      expect(listStyle.paddingLeft).not.toBe("0px");
      if (mode === "none") {
        const bodyMarginTop = await body.evaluate((element) => getComputedStyle(element).marginTop);
        expect(bodyMarginTop).not.toBe("0px");
      } else {
        await expect(body).toHaveCSS("margin-top", "0px");
      }
    }

    for (const control of controls) {
      if (mode === "none") {
        const controlStyle = await control.evaluate((element) => {
          const style = getComputedStyle(element);
          return { fontFamily: style.fontFamily, fontSize: style.fontSize };
        });
        expect(controlStyle.fontFamily).not.toBe(bodyStyle.fontFamily);
        expect(controlStyle.fontSize).not.toBe(bodyStyle.fontSize);
      } else {
        await expect(control).toHaveCSS("font-family", bodyStyle.fontFamily);
        await expect(control).toHaveCSS("font-size", bodyStyle.fontSize);
        await expect(control).toHaveCSS("font-weight", bodyStyle.fontWeight);
      }
      if (mode === "owned-v1") {
        await expect(control).toHaveCSS("letter-spacing", bodyStyle.letterSpacing);
        await expect(control).toHaveCSS("color", bodyStyle.color);
      }
    }

    const buttonBackground = await page.locator("#control").evaluate((element) => {
      const style = getComputedStyle(element);
      return { color: style.backgroundColor, image: style.backgroundImage };
    });
    if (mode === "owned-v1") {
      expect(buttonBackground.color).toBe("rgba(0, 0, 0, 0)");
      expect(buttonBackground.image).toBe("none");
    } else {
      expect(buttonBackground.color).not.toBe("rgba(0, 0, 0, 0)");
    }

    await page.keyboard.press("Tab");
    const focused = page.locator("#control");
    await expect(focused).toBeFocused();
    const outline = await focused.evaluate((element) => {
      const style = getComputedStyle(element);
      return { width: style.outlineWidth, style: style.outlineStyle };
    });
    expect(outline.style).not.toBe("none");
    expect(Number.parseFloat(outline.width)).toBeGreaterThan(0);

    const border = page.locator("#border");
    await expect(border).toHaveCSS("border-left-width", "1px");
    await expect(border).toHaveCSS("border-left-style", "solid");
    await expect(border).toHaveCSS("border-left-color", "rgb(17, 34, 51)");
  }
});
