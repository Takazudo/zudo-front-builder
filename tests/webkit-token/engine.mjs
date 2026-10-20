// Shared Playwright project selection: ZFB_BROWSER_ENGINE=webkit runs the same
// specs in WebKit instead of Chromium (#4231 item 2, `pnpm test:webkit-token`).
// Unset, the project is the unchanged Chromium one.
import { devices } from "@playwright/test";

const ENGINE_DEVICES = {
  chromium: devices["Desktop Chrome"],
  webkit: devices["Desktop Safari"],
};

export const ENGINE = process.env.ZFB_BROWSER_ENGINE ?? "chromium";

if (!(ENGINE in ENGINE_DEVICES)) {
  throw new Error(`unsupported ZFB_BROWSER_ENGINE '${ENGINE}' (expected chromium or webkit)`);
}

export const engineProject = { name: ENGINE, use: { ...ENGINE_DEVICES[ENGINE] } };
