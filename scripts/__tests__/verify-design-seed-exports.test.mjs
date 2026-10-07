import assert from "node:assert/strict";
import { describe, it } from "vite-plus/test";
import {
  createState,
  getSeedFiles,
  variablesCSS,
  windObject,
  zipData,
} from "../../docs/src/components/playground/design-workshop/model.js";
import {
  expectedCandidates,
  verifyArchive,
  verifyCompilerOutput,
} from "../verify-design-seed-exports.mjs";

describe("design export verifier failure detection (logic only; real binary runs remain manager owned)", () => {
  it("checks actual generated archive entries and rejects damaged CRC or file content", () => {
    for (const preset of ["everyday", "editorial", "workbench"]) {
      const files = getSeedFiles(createState(preset)),
        bytes = zipData(files);
      assert.deepEqual(verifyArchive(bytes, files), Object.keys(files));
      const damaged = bytes.slice();
      damaged[14] ^= 1;
      assert.throws(() => verifyArchive(damaged, files), /local CRC/);
      assert.throws(
        () => verifyArchive(bytes, { ...files, "usage.tsx": "lost usage" }),
        /exact exported bytes/,
      );
    }
  });

  it("derives full candidates from actual usage literals plus exported vocabulary", () => {
    const state = createState(),
      files = getSeedFiles(state);
    const candidates = expectedCandidates(files, windObject(state.values));
    assert.ok(candidates.includes("text-on-accent"));
    assert.ok(candidates.includes("gap-x-hsp-gutter"));
    assert.ok(candidates.includes("text-heading"));
    assert.ok(candidates.includes("bg-background"));
    assert.ok(candidates.includes("wide:bg-accent"));
    assert.equal(candidates.length, new Set(candidates).size);
    assert.throws(
      () => expectedCandidates({ ...files, "usage.tsx": "no classes" }, windObject()),
      /actual exported usage/,
    );
  });

  function outputFixture() {
    const state = createState();
    return {
      state,
      candidates: ["bg-accent"],
      css:
        variablesCSS(state.values) +
        "\n:root{--zw-color-accent:var(--ds-accent)}\n.bg-accent{background-color:var(--zw-color-accent)}\n@media(min-width:960px){.wide\\:bg-accent{background-color:var(--zw-color-accent)}}",
      html: '<p class="bg-accent">Give the repeated decisions a name.</p>',
      audit: {
        schemaVersion: 1,
        command: "audit",
        report: {
          outcome: "complete",
          diagnostics: [],
          unrecognizedClasses: [],
          deadClasses: [],
          dynamicConstructions: [],
          adjacentInterpolations: [],
        },
      },
      explanations: {
        schemaVersion: 1,
        command: "explain",
        explanations: [
          {
            candidate: "bg-accent",
            outcome: "resolved_utility",
            selector: ".bg-accent",
            diagnostics: [],
            declarations: [{ property: "background-color", value: "var(--zw-color-accent)" }],
            tokenResolutions: [
              {
                tokenName: "accent",
                variable: "--zw-color-accent",
                configuredValue: "var(--ds-accent)",
              },
            ],
          },
        ],
      },
    };
  }

  it("rejects missing selectors, incorrect declarations, stale owned values and bindings", () => {
    const baseline = outputFixture();
    assert.doesNotThrow(() => verifyCompilerOutput(baseline));
    for (const [before, after, reason] of [
      [".bg-accent{", ".bg-accent-extra{", "exact rule"],
      [
        "background-color:var(--zw-color-accent)",
        "color:var(--zw-color-accent)",
        "background-color",
      ],
      ["#3159d8", "#000000", "current owned value"],
      ["--zw-color-accent:var(--ds-accent)", "--zw-color-accent:red", "binding"],
    ])
      assert.throws(
        () => verifyCompilerOutput({ ...baseline, css: baseline.css.replace(before, after) }),
        new RegExp(reason),
      );
    assert.throws(
      () => verifyCompilerOutput({ ...baseline, html: "not rendered" }),
      /actual DesignExample rendered/,
    );
    assert.throws(
      () =>
        verifyCompilerOutput({
          ...baseline,
          html: baseline.html.replace('class="bg-accent"', 'class="ordinary"'),
        }),
      /rendered HTML class/,
    );
  });

  it("rejects compiler unresolved candidates and audit findings even with plausible CSS", () => {
    const data = outputFixture();
    data.explanations.explanations[0].outcome = "ordinary";
    assert.throws(() => verifyCompilerOutput(data), /compiler resolution/);
    for (const field of [
      "diagnostics",
      "unrecognizedClasses",
      "deadClasses",
      "dynamicConstructions",
      "adjacentInterpolations",
    ]) {
      const data = outputFixture();
      data.audit.report[field].push({ candidate: "text-body" });
      assert.throws(() => verifyCompilerOutput(data), new RegExp(field));
    }
  });
});
