import { describe, it, expect } from "vitest";
import { stripMarkdown } from "../content-files";

describe("stripMarkdown", () => {
  describe("snake_case identifiers pass through unchanged", () => {
    it("preserves worker_only_routes", () => {
      expect(stripMarkdown("worker_only_routes")).toBe("worker_only_routes");
    });

    it("preserves ZFB_ESBUILD_BIN", () => {
      expect(stripMarkdown("ZFB_ESBUILD_BIN")).toBe("ZFB_ESBUILD_BIN");
    });

    it("preserves write_entry_module", () => {
      expect(stripMarkdown("write_entry_module")).toBe("write_entry_module");
    });

    it("preserves spawn_blocking", () => {
      expect(stripMarkdown("spawn_blocking")).toBe("spawn_blocking");
    });
  });

  describe("space-flanked underscore emphasis strips", () => {
    it("strips space-flanked _word_", () => {
      expect(stripMarkdown(" _word_ ")).toBe("word");
    });

    it("strips _emphasis_ inside sentence", () => {
      expect(stripMarkdown("foo _word_ bar")).toBe("foo word bar");
    });

    it("strips __strong__ flanked by spaces", () => {
      expect(stripMarkdown(" __strong__ ")).toBe("strong");
    });
  });

  describe("asterisk emphasis still strips", () => {
    it("strips **bold**", () => {
      expect(stripMarkdown("**bold**")).toBe("bold");
    });

    it("strips *italic*", () => {
      expect(stripMarkdown("*italic*")).toBe("italic");
    });
  });
});
