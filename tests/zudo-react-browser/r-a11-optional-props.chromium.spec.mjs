// @ts-check
import { test, expect } from "@playwright/test";
import { expectNoBrowserErrors, openHeldScenario, releaseScenario } from "./browser-helpers.mjs";

const expectedState = {
  locale: false,
  currentSlug: false,
  summary: { present: false, value: "omitted" },
  records: [
    {
      id: "omitted",
      present: false,
      value: "omitted",
      details: [
        { label: "nested-omitted", present: false, value: "omitted" },
        { label: "nested-null", present: true, value: null },
      ],
    },
    {
      id: "explicit-null",
      present: true,
      value: null,
      details: [],
    },
  ],
};

test("optional record props agree across packed SSR, JSON parsing, and hydration", async ({
  page,
}) => {
  const state = await openHeldScenario(page, "r-a11-optional-props");
  try {
    const wrapper = page.locator('[data-zudo-browser-root="root-0"]');
    const serverText = await page.locator("#optional-props-state").textContent();
    expect(JSON.parse(serverText)).toEqual(expectedState);

    const serializedProps = await wrapper.getAttribute("data-props");
    expect(JSON.parse(serializedProps)).toEqual({
      summary: { title: "Summary" },
      records: [
        {
          id: "omitted",
          details: [{ label: "nested-omitted" }, { label: "nested-null", description: null }],
        },
        { id: "explicit-null", description: null, details: [] },
      ],
    });

    await releaseScenario(page, state);
    const clientState = await page.evaluate(() => {
      const root = window.__zudoReactBrowser.roots[0];
      const props = root.node.props;
      const recordState = (record) => ({
        present: Object.hasOwn(record, "description"),
        value: Object.hasOwn(record, "description") ? record.description : "omitted",
      });
      return {
        active: Boolean(root.root),
        diagnostics: root.diagnostics,
        locale: Object.hasOwn(props, "locale"),
        currentSlug: Object.hasOwn(props, "currentSlug"),
        summary: recordState(props.summary),
        records: props.records.map((record) => ({
          id: record.id,
          ...recordState(record),
          details: record.details.map((detail) => ({
            label: detail.label,
            ...recordState(detail),
          })),
        })),
      };
    });
    expect(clientState).toEqual({ active: true, diagnostics: [], ...expectedState });
    expect(await page.locator("#optional-props-state").textContent()).toBe(serverText);
    expectNoBrowserErrors(state.errors);
  } finally {
    await releaseScenario(page, state).catch(() => undefined);
    await state.heldModule.remove();
  }
});
