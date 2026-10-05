import assert from "node:assert/strict";
import { test } from "node:test";
import { auditWindDocsCoverage } from "../audit-wind-docs-coverage.mjs";

test("settled bilingual wind docs coverage, source syntax, and asset inventories are complete", async () => {
  assert.deepEqual(await auditWindDocsCoverage(), {
    adjacentRoutes: 42,
    adjacentSources: 43,
    catalogEntries: 197,
    editorialFamilies: 47,
    examples: 168,
    guidePages: 10,
    guideRecords: 9,
    historicalWindRoutes: 116,
    negativeExamples: 30,
    parsedSources: 159,
    positiveExamples: 138,
    utilityFamilies: 47,
  });
});
