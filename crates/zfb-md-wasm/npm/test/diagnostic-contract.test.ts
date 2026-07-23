// Public diagnostic-location contract (zfb#1915). This deliberately imports
// the built package: consumers receive the wasm JSON boundary and TypeScript
// wrapper, not the source files. Packaged-artifact README/.d.ts inspection is
// a separate concern from this built-runtime contract test.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, it, expect } from "vitest";

import { compile, parseToAst, renderHtml } from "../dist/index.js";

interface DiagnosticFixture {
  slug: string;
  source: string;
  options: { filename: string };
  line: number;
  column: number;
}

const diagnosticFixtures = (
  JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "..",
        "..",
        "tests",
        "fixtures",
        "parse-to-ast",
        "diagnostics.json",
      ),
      "utf8",
    ),
  ) as { fixtures: DiagnosticFixture[] }
).fixtures;

describe("Diagnostic structured-coordinate contract", () => {
  for (const fixture of diagnosticFixtures) {
    it(`${fixture.slug}: uses only original-source UTF-16 structured coordinates`, async () => {
      const [compiled, rendered, parsed] = await Promise.all([
        compile(fixture.source, fixture.options),
        renderHtml(fixture.source, fixture.options),
        parseToAst(fixture.source, fixture.options),
      ]);

      for (const [entrypoint, output] of [
        ["compile", compiled],
        ["renderHtml", rendered],
        ["parseToAst", parsed],
      ] as const) {
        expect(output.diagnostics, `${entrypoint} must return one diagnostic`).toHaveLength(1);
        const diagnostic = output.diagnostics[0]!;

        // `message` is intentionally not inspected: upstream prose may carry
        // a secondary byte-based coordinate, which is not a public location.
        expect(diagnostic.source).toBe("markdown");
        expect(diagnostic.line).toBe(fixture.line);
        expect(diagnostic.column).toBe(fixture.column);
      }
    });
  }
});
