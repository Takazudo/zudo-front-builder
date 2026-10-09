// Level-4 proof for a visible island retained inside an ancestor across router swaps.
// Route readiness is the armed zfb:page-load event, dispatched after the mount scan.
import { test, expect } from "@playwright/test";

for (const moveMode of ["native", "fallback"]) {
  test.describe(`pending persisted ancestor (${moveMode} moveBefore)`, () => {
    test.beforeEach(async ({ page }) => {
      await page.addInitScript(
        ({ fallback }) => {
          if (fallback) Element.prototype.moveBefore = undefined;
          window.__moveBeforeType = typeof Element.prototype.moveBefore;
          window.__pending = { activations: 0, cleanups: 0, events: [], timeline: [] };
          document.addEventListener("zfb:page-load", () => {
            const event = { phase: "page-load", path: location.pathname };
            window.__pending.events.push(event);
            window.__pending.timeline.push(event);
          });
        },
        { fallback: moveMode === "fallback" },
      );
    });

    for (const scenario of [
      { layout: "skip", names: ["a", "b"], labels: ["Skip A", "Skip B"] },
      { layout: "text", names: ["a", "b", "c"], labels: ["Text A", "Text B", "Text B"] },
    ]) {
      test(`${scenario.layout}: changed pending props mount only after visibility`, async ({
        page,
      }) => {
        const { layout, names, labels } = scenario;
        const path = (name) => `/real-islands-pending-${layout}-${name}.html`;
        const island = page.locator("#island-pending");
        const errors = [];
        page.on("pageerror", (error) => errors.push(error.message));
        page.on("console", (message) => {
          if (message.type() === "error") errors.push(message.text());
        });

        async function snapshot(phase) {
          return page.evaluate((phase) => {
            const pending = window.__pending;
            const node = document.querySelector("#island-pending");
            const state = {
              phase,
              path: location.pathname,
              sameDocument: document === pending.bootDocument,
              sameNode: node === pending.bootNode,
              timeOrigin: performance.timeOrigin,
              top: node.getBoundingClientRect().top,
              innerHeight,
              scrollY,
              props: JSON.parse(node.getAttribute("data-props")),
              text: node.textContent,
              mounted: node.hasAttribute("data-zfb-island-mounted"),
              activations: pending.activations,
              cleanups: pending.cleanups,
              mounts: window.__realIslands.mounts
                .filter((entry) => entry.id === "pending")
                .map((entry) => entry.mode),
            };
            pending.timeline.push(state);
            return state;
          }, phase);
        }

        function expectPending(state, name, label) {
          expect(state.path).toBe(path(name));
          expect(state.sameDocument).toBe(true);
          expect(state.sameNode).toBe(true);
          expect(state.timeOrigin).toBe(bootOrigin);
          expect(state.top).toBeGreaterThan(state.innerHeight);
          expect(state.scrollY).toBe(0);
          expect(state.props.label).toBe(label);
          expect(state.mounted).toBe(false);
          expect(state.activations).toBe(0);
          expect(state.cleanups).toBe(0);
          expect(state.mounts).toEqual([]);
          if (layout === "skip") expect(state.text).toContain("Await visibility");
          else expect(state.text).toContain(labels[0]);
        }

        let bootOrigin;
        try {
          await page.goto(path(names[0]));
          await page.waitForFunction(() =>
            window.__pending.events.some((event) => event.phase === "page-load"),
          );
          expect(await page.evaluate(() => window.__moveBeforeType)).toBe(
            moveMode === "native" ? "function" : "undefined",
          );
          bootOrigin = await page.evaluate(() => {
            const pending = window.__pending;
            pending.bootDocument = document;
            pending.bootNode = document.querySelector("#island-pending");
            pending.bootNode.__pendingIdentity = "boot-node";
            document.__pendingIdentity = "boot-document";
            return performance.timeOrigin;
          });
          expectPending(await snapshot("boot"), names[0], labels[0]);
          await expect(island).not.toHaveAttribute("data-zfb-island-mounted", "");

          for (let index = 1; index < names.length; index++) {
            const name = names[index];
            // Arm before the ordinary internal-link click. This specific event
            // follows the router's mount scan, so zero here proves deferral
            // through the navigation rather than an early polling instant.
            const previous = await page.evaluate(() => window.__pending.events.length);
            const routeReady = page.waitForFunction(
              ({ previous, target }) =>
                window.__pending.events.length > previous &&
                window.__pending.events.slice(previous).some((event) => event.path === target),
              { previous, target: path(name) },
            );
            await page.locator(`#to-pending-${name}`).click();
            await routeReady;
            const state = await snapshot(`post-${name}-pre-scroll`);
            expectPending(state, name, labels[index]);
            expect(
              await page.evaluate(() => ({
                documentIdentity: document.__pendingIdentity,
                nodeIdentity: document.querySelector("#island-pending").__pendingIdentity,
              })),
            ).toEqual({ documentIdentity: "boot-document", nodeIdentity: "boot-node" });
          }

          await island.scrollIntoViewIfNeeded();
          await expect.poll(() => page.evaluate(() => window.__pending.activations)).toBe(1);
          await expect(page.locator("#pending-label")).toHaveText(labels.at(-1));
          await expect(island).toHaveAttribute("data-zfb-island-mounted", "");
          const mounted = await snapshot("post-scroll");
          expect(mounted.scrollY).toBeGreaterThan(0);
          expect(mounted.activations).toBe(1);
          expect(mounted.cleanups).toBe(0);
          expect(mounted.mounts).toEqual(["render"]);
          expect(mounted.text).not.toContain("Await visibility");
          if (layout === "text") expect(mounted.text).not.toContain(labels[0]);
          expect(errors).toEqual([]);
        } catch (error) {
          const timeline = await page
            .evaluate(() => window.__pending?.timeline ?? [])
            .catch(() => []);
          console.error(`pending ${layout}/${moveMode} lifecycle: ${JSON.stringify(timeline)}`);
          throw error;
        }
      });
    }
  });
}
