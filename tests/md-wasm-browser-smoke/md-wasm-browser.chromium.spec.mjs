/**
 * Browser-only acceptance proof for the packed md-wasm browser entry:
 *
 * - nothing fetches glue/wasm before a user click;
 * - the click lazily loads real emitted glue + wasm resources with HTTP 200
 *   and correct MIME;
 * - direct semantic highlighting stays class-based and escaped;
 * - fallback and incomplete editor input retain their documented behavior;
 * - a forced real wasm trap changes the glue generation and uses a fresh
 *   instance without recompiling/fetching the wasm payload.
 */

import { expect, test } from "@playwright/test";

function resourceKind(rawUrl) {
  const url = new URL(rawUrl);
  if (
    url.pathname.includes("islands-resource-zfb_md_wasm_glue.zfb-resource-") &&
    url.pathname.endsWith(".mjs")
  ) {
    return "glue";
  }
  if (url.pathname.includes("islands-resource-zfb_md_wasm_bg-") && url.pathname.endsWith(".wasm")) {
    return "wasm";
  }
  return null;
}

test("packed md-wasm stays lazy, emits semantic markup, and recovers with a fresh glue generation", async ({
  page,
}) => {
  const pageErrors = [];
  const consoleErrors = [];
  const resourceRequests = [];
  const resourceResponses = [];

  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("request", (request) => {
    const kind = resourceKind(request.url());
    if (kind) resourceRequests.push({ kind, url: request.url() });
  });
  page.on("response", (response) => {
    const kind = resourceKind(response.url());
    if (kind) {
      resourceResponses.push({
        kind,
        url: response.url(),
        status: response.status(),
        contentType: response.headers()["content-type"] ?? "",
      });
    }
  });

  await page.goto("/", { waitUntil: "networkidle" });
  await expect(page.getByRole("button", { name: "Run semantic highlights" })).toBeVisible();
  expect(resourceRequests, "glue/wasm must stay unloaded before the user action").toEqual([]);

  await page.getByRole("button", { name: "Run semantic highlights" }).click();
  await expect(page.locator("#status")).toHaveText("Highlights complete.");

  for (const [id, expectedClass] of [
    ["highlight-html", "hi-tag"],
    ["highlight-css", "hi-prop"],
    ["highlight-javascript", "hi-kw"],
  ]) {
    const result = page.locator(`#${id}`);
    await expect(result.locator("pre.hi-root")).toHaveCount(1);
    await expect(result.locator(".line")).toHaveCount(1);
    expect(
      await result.locator(`.${expectedClass}`).count(),
      `${id} semantic role`,
    ).toBeGreaterThan(0);
    const markup = await result.innerHTML();
    expect(markup, `${id} must not carry inline colours`).not.toMatch(/style=|--shiki-|shiki-/i);
  }

  const htmlResult = page.locator("#highlight-html");
  await expect(htmlResult).toContainText('<main data-x="a & b">hello</main>');
  await expect(htmlResult.locator("main")).toHaveCount(0);
  await expect(page.locator("#highlight-javascript")).toContainText("'<tag>'");

  const fallback = JSON.parse((await page.locator("#fallback-diagnostic").textContent()) ?? "{}");
  expect(fallback.html).toContain("&lt;tag&gt;&amp;");
  expect(fallback.diagnostics).toEqual([
    expect.objectContaining({ severity: "warning", source: "highlight", line: null, column: null }),
  ]);
  expect(fallback.diagnostics[0].message).toContain("not-a-bundled-syntax");
  expect(fallback.diagnostics[0].message).toContain("fallback");

  const incomplete = JSON.parse(
    (await page.locator("#incomplete-diagnostics").textContent()) ?? "{}",
  );
  for (const language of ["html", "css", "javascript"]) {
    await expect(page.locator(`#incomplete-${language} pre.hi-root`)).toHaveCount(1);
    expect(incomplete[language], `incomplete ${language} input need not warn`).toEqual([]);
  }

  const glueResponsesBeforeRecovery = resourceResponses.filter(({ kind }) => kind === "glue");
  const wasmResponsesBeforeRecovery = resourceResponses.filter(({ kind }) => kind === "wasm");
  expect(glueResponsesBeforeRecovery).toHaveLength(1);
  expect(wasmResponsesBeforeRecovery).toHaveLength(1);
  expect(glueResponsesBeforeRecovery[0]?.status).toBe(200);
  expect(wasmResponsesBeforeRecovery[0]?.status).toBe(200);
  expect(glueResponsesBeforeRecovery[0]?.contentType).toMatch(/^application\/javascript(?:;|$)/);
  expect(wasmResponsesBeforeRecovery[0]?.contentType).toMatch(/^application\/wasm(?:;|$)/);
  expect(
    new URL(glueResponsesBeforeRecovery[0]?.url ?? "http://invalid").searchParams.get(
      "zfbMdWasmGen",
    ),
  ).toBe("0");

  await page.getByRole("button", { name: "Force trap and recover" }).click();
  await expect(page.locator("#status")).toHaveText("Trap recovery complete.");
  await expect(page.locator("#recovery-highlight pre.hi-root")).toHaveCount(1);
  await expect(page.locator("#recovery-highlight .hi-kw")).toHaveCount(1);

  const recovery = JSON.parse((await page.locator("#recovery-state").textContent()) ?? "{}");
  expect(recovery.trapName).toBe("ZfbMdWasmTrapError");
  expect(recovery.after.currentGeneration).toBe(recovery.before.currentGeneration + 1);
  expect(recovery.after.freshInstanceStarts).toBe(recovery.before.freshInstanceStarts + 1);
  expect(recovery.after.compiledModuleLoads).toBe(recovery.before.compiledModuleLoads);

  const glueResponses = resourceResponses.filter(({ kind }) => kind === "glue");
  const wasmResponses = resourceResponses.filter(({ kind }) => kind === "wasm");
  expect(glueResponses).toHaveLength(2);
  expect(wasmResponses, "the compiled wasm module is cached through recovery").toHaveLength(1);
  expect(glueResponses.every(({ status }) => status === 200)).toBe(true);
  const glueGenerations = glueResponses.map(({ url }) =>
    new URL(url).searchParams.get("zfbMdWasmGen"),
  );
  expect(glueGenerations).toEqual(["0", "1"]);
  expect(new Set(glueResponses.map(({ url }) => url)).size).toBe(2);

  expect(pageErrors, `page errors: ${pageErrors.join("; ")}`).toEqual([]);
  expect(consoleErrors, `console.error calls: ${consoleErrors.join("; ")}`).toEqual([]);
});
