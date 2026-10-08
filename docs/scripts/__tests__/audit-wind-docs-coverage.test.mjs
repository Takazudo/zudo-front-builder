import assert from "node:assert/strict";
import { test } from "node:test";
import { auditWindDocsCoverage } from "../audit-wind-docs-coverage.mjs";

test("settled bilingual wind docs coverage, source syntax, and asset inventories are complete", async () => {
  assert.deepEqual(await auditWindDocsCoverage(), {
    adjacentRoutes: 42,
    adjacentSources: 43,
    catalogEntries: 219,
    editorialFamilies: 48,
    examples: 173,
    guidePages: 10,
    guideRecords: 9,
    historicalWindRoutes: 116,
    negativeExamples: 30,
    parsedSources: 177,
    utilityGroups: 7,
    compatibilityPages: 2,
    positiveExamples: 143,
    utilityFamilies: 48,
  });
});
