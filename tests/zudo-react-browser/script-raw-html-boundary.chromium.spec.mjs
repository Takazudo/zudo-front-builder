// @ts-check
import { test, expect } from "@playwright/test";

// A full HTML document is required: fragment innerHTML parsing alone does not
// prove that the serialized closing script token leaves the next sibling intact.
const CASES = [
  ["plain data", `window.template = "<script>";`, true],
  ["comment opener", `<!--`, true],
  ["closed escape", `<!-- <script> -->`, true],
  ["double escape with mixed case and space", `<!--<ScRiPt >`, false],
  ["double escape with slash", `<!--<script/>`, false],
  ["near-miss name", `<!--<scripture>`, true],
  ["double escape exits", `<!--<script></SCRIPT>`, true],
  ["reentered double escape", `<!--<script></script><script>`, false],
  ["incomplete double escape name", `<!--<script`, true],
  ["incomplete double escape exit", `<!--<script></scr`, false],
];

test("serialized script boundary leaves a trusted following sibling exactly when safe", async ({
  page,
}) => {
  for (const [name, payload, safe] of CASES) {
    await page.setContent(
      `<!doctype html><html><head><script type="application/json">${payload}</script></head>` +
        `<body><p id="after">trusted</p></body></html>`,
    );
    expect(await page.locator("#after").count(), name).toBe(safe ? 1 : 0);
    if (safe) expect(await page.locator("#after").textContent(), name).toBe("trusted");
  }
});
