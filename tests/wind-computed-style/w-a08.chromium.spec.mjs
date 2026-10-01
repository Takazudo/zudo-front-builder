import { expect, test } from "@playwright/test";
import { openFixture, readReport, readStylesheet } from "./helpers.mjs";

test("W-A08 computes aspect, shadow transition, and default, configured, and explicit easing", async ({
  page,
}) => {
  await openFixture(page, "w-a08-default");
  const defaultTransition = page.locator("#motion-default");
  await expect(defaultTransition).toHaveCSS("transition-property", "box-shadow");
  await expect(defaultTransition).toHaveCSS("transition-timing-function", "ease");

  await openFixture(page, "w-a08");
  const aspect = page.locator("#aspect-target");
  await expect(aspect).toHaveCSS("aspect-ratio", "16 / 9");
  await expect(aspect).toHaveCSS("height", "180px");

  const shadow = page.locator("#shadow-target");
  await expect(shadow).toHaveCSS("transition-property", "box-shadow");
  await expect(shadow).toHaveCSS("transition-timing-function", "linear");
  await expect(shadow).toHaveCSS("transition-duration", "1s");
  await expect(shadow).not.toHaveCSS("box-shadow", "none");

  await expect(page.locator("#motion-configured")).toHaveCSS(
    "transition-timing-function",
    "linear",
  );
  await expect(page.locator("#motion-explicit")).toHaveCSS(
    "transition-timing-function",
    "ease-in-out",
  );

  const report = await readReport("w-a08");
  const stylesheet = await readStylesheet("w-a08");
  expect(report.rules).toContain("aspect-16/9");
  expect(stylesheet).toContain("aspect-ratio: 16 / 9;");
  expect(report.diagnostics).toContainEqual(
    expect.objectContaining({
      candidate: "aspect-1.5",
      suggestion: "aspect-[1.5/1]",
    }),
  );
});

test("W-A08 seeks the real focus-visible outline transition and retains visual states", async ({
  page,
}, testInfo) => {
  await openFixture(page, "w-a08");
  const focusOutline = page.locator("#focus-outline");
  const mediaState = await page.evaluate(() => ({
    reducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
  }));
  expect(mediaState.reducedMotion).toBe(false);

  const initialColor = await focusOutline.evaluate(
    (element) => getComputedStyle(element).outlineColor,
  );
  expect(initialColor).toBe("rgb(0, 0, 0)");
  await expect(focusOutline).toHaveCSS("transition-duration", "1s");
  await testInfo.attach("w-a08-outline-before-focus.png", {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });

  await page.keyboard.press("Tab");
  await expect(focusOutline).toBeFocused();
  await page.waitForFunction(() => {
    const element = document.getElementById("focus-outline");
    return element
      ?.getAnimations()
      .some(
        (animation) =>
          animation.constructor.name === "CSSTransition" &&
          animation.transitionProperty === "outline-color",
      );
  });

  const intermediate = await focusOutline.evaluate((element) => {
    const transition = element
      .getAnimations()
      .find(
        (animation) =>
          animation.constructor.name === "CSSTransition" &&
          animation.transitionProperty === "outline-color",
      );
    if (!transition) throw new Error("outline-color CSSTransition disappeared");
    transition.pause();
    transition.currentTime = 500;
    window.__wA08OutlineTransition = transition;
    return {
      currentTime: transition.currentTime,
      progress: transition.effect?.getComputedTiming().progress,
      color: getComputedStyle(element).outlineColor,
    };
  });
  expect(intermediate.currentTime).toBe(500);
  expect(intermediate.progress).toBeGreaterThan(0);
  expect(intermediate.progress).toBeLessThan(1);
  expect(intermediate.color).not.toBe(initialColor);
  expect(intermediate.color).not.toBe("rgb(255, 255, 255)");
  await testInfo.attach("w-a08-outline-transition-midpoint.png", {
    body: await focusOutline.screenshot(),
    contentType: "image/png",
  });

  const settled = await focusOutline.evaluate((element) => {
    const transition = window.__wA08OutlineTransition;
    if (!transition) throw new Error("outline-color CSSTransition disappeared before settle");
    transition.currentTime = transition.effect?.getTiming().duration ?? 1000;
    return {
      currentTime: transition.currentTime,
      progress: transition.effect?.getComputedTiming().progress,
      color: getComputedStyle(element).outlineColor,
    };
  });
  expect(settled.currentTime).toBe(1000);
  expect(settled.progress).toBe(1);
  expect(settled.color).toBe("rgb(255, 255, 255)");
  await testInfo.attach("w-a08-outline-settled.png", {
    body: await focusOutline.screenshot(),
    contentType: "image/png",
  });
});
