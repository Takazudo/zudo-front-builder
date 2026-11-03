/** Real history traversal, explicitly released network/preparation and native VT callbacks. */
// @ts-check
import { test, expect } from "@playwright/test";

const responseHtml = `<!doctype html><html><head><meta name="zfb-view-transitions-enabled" content=""><title>B</title></head><body><h1>Obsolete B</h1></body></html>`;

async function boot(page) {
  await page.goto("/page-a.html");
  await page.evaluate(async () => {
    const router = await import("/dist/client-router/router.js");
    router.init();
    const state = {
      navigate: router.navigate,
      events: [],
      sentinel: {},
      release: undefined,
      entered: false,
      signal: undefined,
      navigation: undefined,
    };
    window.__nullRace = state;
    for (const name of [
      "zfb:navigation-aborted",
      "zfb:after-swap",
      "zfb:page-load",
      "zfb:before-preparation",
    ])
      document.addEventListener(name, () => state.events.push(name));
    document.querySelector("main").dataset.sentinel = "kept";
  });
  await page.evaluate(() => {
    location.hash = "first";
  });
  await expect(page).toHaveURL(/#first$/);
  expect(await page.evaluate(() => history.state)).toBeNull();
  await page.evaluate(() => {
    location.hash = "second";
  });
  await expect(page).toHaveURL(/#second$/);
  expect(await page.evaluate(() => history.state)).toBeNull();
  await page.evaluate(() => {
    window.__nullRace.events.length = 0;
  });
}

async function expectKept(page, historyLength) {
  await expect(page).toHaveURL(/\/page-a\.html#first$/);
  await expect(page.locator("main")).toHaveAttribute("data-sentinel", "kept");
  await expect(page.locator("h1")).toHaveText("Page A");
  expect(await page.evaluate(() => history.state)).toBeNull();
  expect(await page.evaluate(() => history.length)).toBe(historyLength);
  const events = await page.evaluate(() => window.__nullRace.events);
  expect(events.filter((name) => name === "zfb:navigation-aborted")).toHaveLength(1);
  expect(events).not.toContain("zfb:after-swap");
  expect(events).not.toContain("zfb:page-load");
}

for (const kind of ["json", "html"]) {
  test(`null-state Back suppresses a late ${kind} response, including document fallback`, async ({
    page,
  }) => {
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const requests = [];
    page.on("request", (request) => {
      if (request.url().includes("null-race-response")) requests.push(request.resourceType());
    });
    await page.route("**/null-race-response", (route) =>
      route.fulfill({
        contentType: kind === "json" ? "application/json" : "text/html",
        body: kind === "json" ? "{}" : responseHtml,
      }),
    );
    await boot(page);
    const historyLength = await page.evaluate(() => history.length);
    await page.evaluate(() => {
      const state = window.__nullRace;
      const nativeFetch = window.fetch.bind(window);
      window.fetch = async (input, options) => {
        if (!String(input).includes("null-race-response")) return nativeFetch(input, options);
        state.signal = options.signal;
        // Buffer a real HTTP response before holding its delivery. Cancellation
        // cannot retract bytes already received; stale continuations must check ownership.
        const response = await nativeFetch(input, options);
        const bytes = await response.arrayBuffer();
        const buffered = new Response(bytes, {
          status: response.status,
          headers: response.headers,
        });
        Object.defineProperty(buffered, "url", { value: response.url });
        await new Promise((resolve) => {
          state.release = resolve;
          state.entered = true;
        });
        return buffered;
      };
      state.navigation = state.navigate("/null-race-response");
    });
    await page.waitForFunction(() => window.__nullRace.entered);
    await page.goBack();
    await expect(page).toHaveURL(/#first$/);
    await page.evaluate(() => window.__nullRace.release());
    await page.evaluate(() => window.__nullRace.navigation);
    expect(await page.evaluate(() => window.__nullRace.signal.aborted)).toBe(true);
    await expectKept(page, historyLength);
    expect(requests).toEqual(["fetch"]);
    // Native Forward/Back remains intact and adds no router preparation.
    const preparations = await page.evaluate(
      () => window.__nullRace.events.filter((name) => name === "zfb:before-preparation").length,
    );
    await page.goForward();
    await expect(page).toHaveURL(/#second$/);
    await page.goBack();
    await expectKept(page, historyLength);
    expect(
      await page.evaluate(
        () => window.__nullRace.events.filter((name) => name === "zfb:before-preparation").length,
      ),
    ).toBe(preparations);
    expect(errors).toEqual([]);
  });
}

test("null-state Back wins over an already queued real View Transition callback", async ({
  page,
}) => {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/null-race-response", (route) =>
    route.fulfill({ contentType: "text/html", body: responseHtml }),
  );
  await boot(page);
  const originalLength = await page.evaluate(() => history.length);
  await page.evaluate(() => {
    const state = window.__nullRace;
    const nativeTransition = document.startViewTransition.bind(document);
    document.startViewTransition = (callback) =>
      nativeTransition(async () => {
        await new Promise((resolve) => {
          state.release = resolve;
          state.entered = true;
        });
        await callback();
      });
    state.navigation = state.navigate("/null-race-response");
  });
  await page.waitForFunction(() => window.__nullRace.entered);
  await expect(page).toHaveURL(/\/null-race-response$/);
  // B's early history entry is newer than A#second, so go back two entries.
  await page.evaluate(() => history.go(-2));
  await expect(page).toHaveURL(/#first$/);
  await page.evaluate(() => window.__nullRace.release());
  await page.evaluate(() => window.__nullRace.navigation);
  await expectKept(page, originalLength + 1);
  expect(errors).toEqual([]);
});

test("a managed-state traversal also cancels a pending request", async ({ page }) => {
  await boot(page);
  await page.evaluate(() => {
    const state = window.__nullRace;
    const nativeFetch = window.fetch.bind(window);
    window.fetch = async (input, options) => {
      if (!String(input).includes("null-race-response")) return nativeFetch(input, options);
      state.signal = options.signal;
      await new Promise((resolve) => {
        state.release = resolve;
        state.entered = true;
      });
      return new Response("{}", { headers: { "content-type": "application/json" } });
    };
    state.navigation = state.navigate("/null-race-response");
  });
  await page.waitForFunction(() => window.__nullRace.entered);
  await page.evaluate(() => history.go(-2));
  await expect(page).toHaveURL(/\/page-a\.html$/);
  expect(await page.evaluate(() => history.state.index)).toEqual(expect.any(Number));
  await page.evaluate(() => window.__nullRace.release());
  await page.evaluate(() => window.__nullRace.navigation);
  expect(await page.evaluate(() => window.__nullRace.signal.aborted)).toBe(true);
  await expect(page.locator("main")).toHaveAttribute("data-sentinel", "kept");
});

test("null-state Back aborts an actual pending HTTP fetch", async ({ page }) => {
  let releaseResponse;
  const responseGate = new Promise((resolve) => {
    releaseResponse = resolve;
  });
  let requestArrived;
  const arrival = new Promise((resolve) => {
    requestArrived = resolve;
  });
  await page.route("**/null-race-response", async (route) => {
    requestArrived();
    await responseGate;
    await route.fulfill({ contentType: "text/html", body: responseHtml });
  });
  await boot(page);
  const historyLength = await page.evaluate(() => history.length);
  const failedRequest = page.waitForEvent("requestfailed", (request) =>
    request.url().includes("null-race-response"),
  );
  await page.evaluate(() => {
    window.__nullRace.navigation = window.__nullRace.navigate("/null-race-response");
  });
  await arrival;
  await page.goBack();
  await expect(page).toHaveURL(/#first$/);
  releaseResponse();
  await page.evaluate(() => window.__nullRace.navigation);
  expect((await failedRequest).failure()?.errorText).toBe("net::ERR_ABORTED");
  await expectKept(page, historyLength);
});
