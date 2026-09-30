import { beforeEach, describe, expect, it } from "vitest";
import { For, Show, computed, flush, getScope, h, signal } from "../../zudo-react/index.js";
import type { Diagnostic } from "../../zudo-react/index.js";
import { hydrate } from "../../zudo-react/client.js";
import { islandRoot, renderToString } from "../../zudo-react/server.js";

type Row = { id: string; name: string };
type TableData = { rev: number; rows: Row[] };

const identity = { component: "ResultsTable", build: "b1" };

beforeEach(() => document.body.replaceChildren());

function assertRows(root: ParentNode, names: string[]) {
  const table = root.querySelector("table");
  expect(table).not.toBeNull();
  expect(root.querySelectorAll("table > tbody > tr")).toHaveLength(names.length);
  expect(
    [...root.querySelectorAll("table > tbody > tr > td:first-child")].map(
      (cell) => cell.textContent,
    ),
  ).toEqual(names);
}

describe("keyed table remount", () => {
  it("renders intrinsic rows on the server and remounts the table twice after hydration", async () => {
    const data = signal<TableData>({ rev: 0, rows: [{ id: "a", name: "First" }] });
    const cleanups: number[] = [];
    function ResultsTable() {
      const table = computed(() => {
        const { rev, rows } = data.value;
        return [{ rev, rows }];
      });
      return h(
        "section",
        null,
        For({
          each: table,
          by: (item) => item.rev,
          children: (item) => {
            const { rev, rows } = item.value;
            getScope().onCleanup(() => cleanups.push(rev));
            return h(
              "table",
              null,
              h(
                "tbody",
                null,
                rows.map((row) =>
                  h(
                    "tr",
                    null,
                    h("td", null, row.name),
                    h("td", null, h("input", { "aria-label": row.name })),
                  ),
                ),
              ),
            );
          },
        }),
      );
    }

    const html = renderToString(islandRoot(h(ResultsTable, null), { identity }));
    const serverHost = document.createElement("div");
    serverHost.innerHTML = html;
    assertRows(serverHost, ["First"]);

    const root = serverHost.firstElementChild!;
    document.body.append(root);
    const oldTable = root.querySelector("table")!;
    const diagnostics: Diagnostic[] = [];
    cleanups.length = 0;
    const handle = hydrate(h(ResultsTable, null), root, {
      identity,
      report: (diagnostic) => diagnostics.push(diagnostic),
    });
    expect(handle).not.toBeNull();
    expect(root.querySelector("table")).toBe(oldTable);
    expect(diagnostics).toEqual([]);

    const oldInput = root.querySelector("input")!;
    oldInput.focus();
    expect(document.activeElement).toBe(oldInput);
    data.value = {
      rev: 1,
      rows: [
        { id: "b", name: "Second" },
        { id: "c", name: "Third" },
      ],
    };
    await flush();
    assertRows(root, ["Second", "Third"]);
    expect(root.querySelector("table")).not.toBe(oldTable);
    expect(document.activeElement).toBe(document.body);
    expect(cleanups).toEqual([0]);

    const secondTable = root.querySelector("table")!;
    data.value = { rev: 2, rows: [{ id: "d", name: "Fourth" }] };
    await flush();
    assertRows(root, ["Fourth"]);
    expect(root.querySelector("table")).not.toBe(secondTable);
    expect(cleanups).toEqual([0, 1]);
    expect(diagnostics).toEqual([]);

    handle!.dispose();
    expect(cleanups).toEqual([0, 1, 2]);
  });

  it("does not rerun a Show table factory while when stays true", async () => {
    const data = signal<TableData>({ rev: 0, rows: [{ id: "a", name: "First" }] });
    const visible = computed(() => data.value.rows.length > 0);
    function ShowTable() {
      return h(
        "section",
        null,
        Show({
          when: visible,
          children: () =>
            h(
              "table",
              null,
              h(
                "tbody",
                null,
                data.value.rows.map((row) => h("tr", null, h("td", null, row.name))),
              ),
            ),
        }),
      );
    }
    const showIdentity = { component: "ShowTable", build: "b1" };
    const host = document.createElement("div");
    host.innerHTML = renderToString(islandRoot(h(ShowTable, null), { identity: showIdentity }));
    const root = host.firstElementChild!;
    document.body.append(root);
    const diagnostics: Diagnostic[] = [];
    const handle = hydrate(h(ShowTable, null), root, {
      identity: showIdentity,
      report: (diagnostic) => diagnostics.push(diagnostic),
    });
    expect(handle).not.toBeNull();
    assertRows(root, ["First"]);
    data.value = { rev: 1, rows: [{ id: "b", name: "Second" }] };
    await flush();
    assertRows(root, ["First"]);
    expect(diagnostics).toEqual([]);
    handle!.dispose();
  });
});
