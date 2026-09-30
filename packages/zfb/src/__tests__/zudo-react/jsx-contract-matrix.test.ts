// @vitest-environment node
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { renderToString } from "../../zudo-react/server.js";
import { jsxContractMatrix } from "./jsx-contract-matrix.js";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../../");

beforeEach(() => {
  (globalThis as typeof globalThis & { __zfb?: unknown }).__zfb = {
    zudoReactBuild: "fixture-build",
    zudoReactIslands: ["Widget"],
  };
});
afterEach(() => {
  delete (globalThis as typeof globalThis & { __zfb?: unknown }).__zfb;
});

describe("zfb JSX producer contract matrix", () => {
  it("anchors every row to an existing source path and symbol", () => {
    for (const row of jsxContractMatrix) {
      const sourcePath = resolve(repoRoot, row.producerPath);
      expect(existsSync(sourcePath), row.producerPath).toBe(true);
      expect(readFileSync(sourcePath, "utf8"), `${row.producerPath} #${row.symbol}`).toContain(
        row.symbol,
      );
    }
  });

  it.each(jsxContractMatrix)("$producerPath #$symbol: $id", (row) => {
    const node = row.build();

    if (row.expectation.kind === "accepted") {
      expect(renderToString(node)).toBe(row.expectation.html);
      if (row.reuse) {
        expect(row.build()).toBe(node);
        expect(renderToString(node)).toBe(row.expectation.html);
      }
      return;
    }

    expect(() => renderToString(node)).toThrow(row.expectation.code);
    if (row.expectation.detail) expect(() => renderToString(node)).toThrow(row.expectation.detail);
  });
});
