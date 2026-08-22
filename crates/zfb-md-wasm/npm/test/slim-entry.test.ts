import { describe, expect, it } from "vitest";

import * as parseEntry from "../dist/parse.js";
import * as renderEntry from "../dist/render.js";

describe("built slim entries", () => {
  it("render round-trips only its own capability", async () => {
    expect("compile" in renderEntry).toBe(false);
    expect("highlightCode" in renderEntry).toBe(false);
    expect("parseToAst" in renderEntry).toBe(false);
    await expect(renderEntry.renderHtml("# Render")).resolves.toMatchObject({
      html: "<h1>Render</h1>",
      diagnostics: [],
    });
  });

  it("parse round-trips only its own capability and keeps independent state", async () => {
    expect("compile" in parseEntry).toBe(false);
    expect("highlightCode" in parseEntry).toBe(false);
    expect("renderHtml" in parseEntry).toBe(false);
    await expect(parseEntry.parseToAst("# Parse")).resolves.toMatchObject({
      ast: { type: "root" },
      diagnostics: [],
    });
    expect(parseEntry.__getTrapRecoveryStateForTests().currentGeneration).toBe(0);
    await expect(renderEntry.__forceTrapForTests()).rejects.toBeInstanceOf(
      renderEntry.ZfbMdWasmTrapError,
    );
    expect(renderEntry.__getTrapRecoveryStateForTests().currentGeneration).toBe(1);
    expect(parseEntry.__getTrapRecoveryStateForTests().currentGeneration).toBe(0);
  });
});
