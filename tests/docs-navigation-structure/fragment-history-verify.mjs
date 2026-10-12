// @verification: issue #4167 / #4157 actual-host fragment-history acceptance; not automatically added to CI.
// A -> same-document fragment entry -> soft navigate to B -> Back -> Forward -> Back on the
// real docs host. Fails on a router that ignores a null-state popstate to a different
// document (#4165); passes once Back/Forward swap the content again.
import { chromium, expect, webkit } from "@playwright/test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { WIND_UTILITY_GROUPS } from "../../docs/src/config/navigation-groups.mjs";

const origin = process.env.DOCS_VERIFY_ORIGIN ?? "http://127.0.0.1:4333";
const output = process.env.DOCS_VERIFY_OUTPUT;
if (!output) throw new Error("DOCS_VERIFY_OUTPUT must name the evidence directory");
const label = process.env.DOCS_VERIFY_LABEL ?? "unlabeled";
mkdirSync(output, { recursive: true });
const browserName = process.env.DOCS_VERIFY_BROWSER ?? "chromium";
const browserType = { chromium, webkit }[browserName];
if (!browserType)
  throw new Error(`DOCS_VERIFY_BROWSER must be chromium or webkit, got ${browserName}`);

const evidence = {
  commit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  origin,
  label,
  browser: browserName,
  cases: [],
  routerAssets: {},
  errors: [],
};
const TIMEOUT = 20_000;
const VIEWPORTS = [
  { name: "desktop", width: 1440, height: 1000 },
  { name: "mobile", width: 390, height: 844 },
];
const typography = WIND_UTILITY_GROUPS.find((g) => g.id === "typography");
const routes = (locale) => ({
  a: `${locale}/docs/zudo-wind/utilities/background/`,
  b: `${locale}/docs/${typography.slug}/`,
});

const browser = await browserType.launch();

// Router lifecycle log on the live document: every zfb:* event with the URL it saw.
const installLifecycleProbe = () => {
  window.__lifecycle = [];
  for (const name of ["before-swap", "after-swap", "page-load", "navigation-aborted"])
    document.addEventListener(`zfb:${name}`, () =>
      window.__lifecycle.push({ name, at: location.pathname + location.hash }),
    );
};
const count = (page, name) =>
  page.evaluate((n) => window.__lifecycle.filter((e) => e.name === n).length, name);

async function waitForSwaps(page, before, step) {
  try {
    await page.waitForFunction(
      (expected) => window.__lifecycle.filter((e) => e.name === "after-swap").length > expected,
      before,
      { timeout: TIMEOUT },
    );
  } catch (error) {
    throw new Error(
      `${step}: router never swapped content (zfb:after-swap not fired): ${error.message}`,
    );
  }
}

// The target sits near its scroll-margin offset, or the page is already scrolled to its end.
async function expectAnchorSettled(page, id, step) {
  let last;
  await expect
    .poll(
      async () =>
        (last = await page.evaluate((id) => {
          const el = document.getElementById(id);
          if (!el) return { ok: false, reason: "missing" };
          const top = el.getBoundingClientRect().top;
          const margin = parseFloat(getComputedStyle(el).scrollMarginTop) || 0;
          const max = document.documentElement.scrollHeight - innerHeight;
          const atEnd = scrollY >= max - 1;
          const ok = top >= -2 && top < innerHeight && (atEnd || Math.abs(top - margin) <= 24);
          return { ok, top, margin, scrollY, max };
        }, id)),
      { message: `${step}: #${id} settled near its scroll margin`, timeout: TIMEOUT },
    )
    .toMatchObject({ ok: true })
    .catch((error) => {
      throw new Error(`${error.message}\nlast measurement: ${JSON.stringify(last)}`);
    });
}

async function expectArticle(page, path, h1, step, hash = "") {
  await expect(page, `${step}: url`).toHaveURL(origin + path + hash, { timeout: TIMEOUT });
  await expect(page.locator("main h1"), `${step}: exactly this article`).toHaveText([h1], {
    timeout: TIMEOUT,
  });
}

// Soft navigation through a real router link, like verify.mjs soft().
async function softTo(page, path, step) {
  const before = await count(page, "after-swap");
  await page.evaluate((path) => {
    const a = document.createElement("a");
    a.href = path;
    a.id = "verification-link";
    a.textContent = "verification navigation";
    // Out of flow: an in-flow link would shift A's layout before its scroll is saved.
    a.style.cssText = "position:fixed;top:0;left:0";
    document.querySelector("main").prepend(a);
    // Activate in place: Playwright's locator.click() would first scroll the prepended
    // link into view, so the router would correctly save scrollY 0 for the outgoing
    // fragment entry and Back would restore 0 instead of the anchor.
    a.click();
  }, path);
  await waitForSwaps(page, before, step);
  await page.locator("#verification-link").evaluateAll((nodes) => nodes.forEach((n) => n.remove()));
}

async function traverse(page, direction, step) {
  const before = await count(page, "after-swap");
  await (direction === "back" ? page.goBack() : page.goForward());
  await waitForSwaps(page, before, step);
}

async function runCase({ locale, viewport, variant }) {
  const name = `${locale || "en"}-${viewport.name}-${variant}`;
  const { a, b } = routes(locale);
  const context = await browser.newContext({
    viewport: { width: viewport.width, height: viewport.height },
    reducedMotion: "reduce",
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(`${name}: ${e.message}`));
  // Prove which bytes the browser executed: every served script holding the router.
  page.on("response", async (response) => {
    const url = response.url();
    if (!/\.m?js(\?|$)/.test(url)) return;
    const body = await response.body().catch(() => null);
    if (body && body.includes("zfb:before-swap"))
      evidence.routerAssets[url] = createHash("sha256").update(body).digest("hex");
  });
  await page.addInitScript(installLifecycleProbe);

  await page.goto(origin + a);
  await page.locator("main").waitFor();
  const h1A = (await page.locator("main h1").first().innerText()).trim();
  await page
    .locator("[data-zfb-island][data-zfb-island-mounted]")
    .first()
    .waitFor({ state: "attached" });

  // Two fragment targets far enough down to scroll; the second proves controls stay usable.
  const ids = await page.evaluate(() =>
    [...document.querySelectorAll("main h2[id], main h3[id]")]
      .filter((el) => el.getBoundingClientRect().top + scrollY > 300)
      .map((el) => el.id),
  );
  assert(ids.length >= 2, `${name}: need two fragment targets on ${a}`);
  const [target, second] = ids;
  const hash = `#${target}`;

  let activation = variant;
  if (variant === "toc") {
    const link = page.locator(`[data-zd-toc] a[href="${hash}"]`).first();
    await link.waitFor({ state: "attached" });
    if (!(await link.isVisible())) {
      await context.close();
      return { name, skipped: "TOC link not visible at this viewport" };
    }
    await link.click();
  } else {
    await page.evaluate((id) => {
      location.hash = id;
    }, target);
  }
  await expectArticle(page, a, h1A, `${name} fragment`, hash);
  await expectAnchorSettled(page, target, `${name} fragment`);
  const swapsAfterFragment = await count(page, "after-swap");

  await softTo(page, b, `${name} A->B`);
  const h1B = (await page.locator("main h1").first().innerText()).trim();
  assert.notEqual(h1A, h1B, `${name}: A and B need distinct headings`);
  await expectArticle(page, b, h1B, `${name} A->B`);

  const checkA = async (step) => {
    await expectArticle(page, a, h1A, step, hash);
    await expect(page.locator(`#${target}`), `${step}: A target present`).toHaveCount(1);
    await expectAnchorSettled(page, target, step);
  };
  const checkB = async (step) => {
    await expectArticle(page, b, h1B, step);
    await expect(page.locator("main h1"), `${step}: A heading absent`).not.toHaveText([h1A]);
  };

  await traverse(page, "back", `${name} Back to A#`);
  await checkA(`${name} Back to A#`);
  await traverse(page, "forward", `${name} Forward to B`);
  await checkB(`${name} Forward to B`);
  await traverse(page, "back", `${name} Back to A# (2)`);
  await checkA(`${name} Back to A# (2)`);

  // Controls still work after the sequence: mounted island, a second fragment entry.
  await page
    .locator("[data-zfb-island][data-zfb-island-mounted]")
    .first()
    .waitFor({ state: "attached" });
  await page.evaluate((id) => {
    location.hash = id;
  }, second);
  await expectArticle(page, a, h1A, `${name} second fragment`, `#${second}`);
  await expectAnchorSettled(page, second, `${name} second fragment`);

  const lifecycle = await page.evaluate(() => window.__lifecycle);
  await context.close();
  assert.deepEqual(errors, [], `${name}: page errors`);
  return {
    name,
    activation,
    a,
    b,
    target,
    swapsAfterFragment,
    swaps: lifecycle.filter((e) => e.name === "after-swap").length,
    lifecycle,
  };
}

let failure;
try {
  for (const locale of ["", "/ja"])
    for (const viewport of VIEWPORTS)
      for (const variant of ["toc", "hash"]) {
        const result = await runCase({ locale, viewport, variant });
        evidence.cases.push(result);
        console.log(
          result.skipped ? `SKIP ${result.name}: ${result.skipped}` : `PASS ${result.name}`,
        );
      }
  const ran = (n) => evidence.cases.filter((c) => !c.skipped && c.name === n).length;
  // Case names carry the locale path prefix: "en-…" for the root locale, "/ja-…" for Japanese.
  for (const locale of ["en", "/ja"]) {
    assert.equal(ran(`${locale}-desktop-toc`), 1, `${locale}: desktop TOC case must run`);
    assert.equal(ran(`${locale}-mobile-hash`), 1, `${locale}: mobile hash case must run`);
  }
  assert(
    Object.keys(evidence.routerAssets).length > 0,
    "browser fetched no script holding the router",
  );
} catch (error) {
  failure = error;
  evidence.errors.push(String(error?.stack ?? error));
} finally {
  writeFileSync(
    `${output}/fragment-history-evidence-${label}.json`,
    JSON.stringify(evidence, null, 2),
  );
  await browser.close();
}
if (failure) {
  console.error(failure);
  process.exit(1);
}
console.log(
  JSON.stringify(
    {
      label,
      commit: evidence.commit,
      cases: evidence.cases.map((c) => c.name + (c.skipped ? " (skipped)" : "")),
      routerAssets: evidence.routerAssets,
    },
    null,
    2,
  ),
);
