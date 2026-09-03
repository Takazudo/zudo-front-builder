import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import {
  ARTIFACTS,
  checkCeilings,
  compareManifest,
  formatFindings,
} from "../assert-zfb-md-wasm-budgets.mjs";

const scriptPath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../assert-zfb-md-wasm-budgets.mjs",
);

// In-test manifest, never the live crates/zfb-md-wasm/shipped-sizes.json:
// compareManifest and checkCeilings are pure functions of their arguments.
const documented = {
  root: { finalWasm: 3_394_144, gzip9: 1_514_540, glue: 14_998, glueGzip9: 4_199 },
  highlight: { finalWasm: 1_539_186, gzip9: 817_922, glue: 8_758, glueGzip9: 2_637 },
  render: { finalWasm: 2_189_671, gzip9: 1_088_858, glue: 8_772, glueGzip9: 2_661 },
  parse: { finalWasm: 693_479, gzip9: 281_394, glue: 11_159, glueGzip9: 3_797 },
};

function matchingMeasuredByArtifact() {
  return {
    default: { ...documented.root },
    "highlight-only": { ...documented.highlight },
    "render-only": { ...documented.render },
    "parse-only": { ...documented.parse },
  };
}

describe("compareManifest", () => {
  it("returns no findings when every artifact and column matches", () => {
    expect(compareManifest(matchingMeasuredByArtifact(), documented)).toEqual([]);
  });

  it("reports a render mismatch and a parse mismatch from one call (zfb#2879)", () => {
    const measuredByArtifact = matchingMeasuredByArtifact();
    measuredByArtifact["render-only"] = {
      ...documented.render,
      gzip9: documented.render.gzip9 + 1,
    };
    measuredByArtifact["parse-only"] = {
      ...documented.parse,
      finalWasm: documented.parse.finalWasm + 2,
    };

    expect(compareManifest(measuredByArtifact, documented)).toEqual([
      expect.objectContaining({ artifact: "render-only", column: "gzip9" }),
      expect.objectContaining({ artifact: "parse-only", column: "finalWasm" }),
    ]);
  });

  it("reports two drifted columns on the same artifact", () => {
    const measuredByArtifact = matchingMeasuredByArtifact();
    measuredByArtifact["highlight-only"] = {
      ...documented.highlight,
      finalWasm: documented.highlight.finalWasm + 3,
      glueGzip9: documented.highlight.glueGzip9 + 4,
    };

    expect(compareManifest(measuredByArtifact, documented)).toEqual([
      expect.objectContaining({ artifact: "highlight-only", column: "finalWasm" }),
      expect.objectContaining({ artifact: "highlight-only", column: "glueGzip9" }),
    ]);
  });

  it("carries severity error and a message naming the artifact and column", () => {
    const measuredByArtifact = matchingMeasuredByArtifact();
    measuredByArtifact["parse-only"] = { ...documented.parse, glue: documented.parse.glue + 5 };

    expect(compareManifest(measuredByArtifact, documented)).toEqual([
      expect.objectContaining({
        code: "manifest-mismatch",
        severity: "error",
        artifact: "parse-only",
        column: "glue",
        measured: documented.parse.glue + 5,
        documented: documented.parse.glue,
        delta: 5,
        message: expect.stringContaining("parse-only glue mismatch"),
      }),
    ]);
  });
});

describe("checkCeilings", () => {
  const underCeiling = Object.fromEntries(
    ARTIFACTS.map((artifact) => [artifact.label, { gzip9: 1_000 }]),
  );

  it("reports no findings when every artifact is under its ceiling", () => {
    expect(checkCeilings(underCeiling)).toEqual([]);
  });

  it("reports breaches on two artifacts and leaves the rest alone", () => {
    const measuredByArtifact = {
      ...underCeiling,
      "highlight-only": { gzip9: ARTIFACTS.find((a) => a.label === "highlight-only").ceiling + 1 },
      "parse-only": { gzip9: ARTIFACTS.find((a) => a.label === "parse-only").ceiling + 2 },
    };

    expect(checkCeilings(measuredByArtifact)).toEqual([
      expect.objectContaining({
        code: "ceiling-exceeded",
        severity: "error",
        artifact: "highlight-only",
        message: expect.stringContaining("highlight-only gzip-9 size"),
      }),
      expect.objectContaining({
        code: "ceiling-exceeded",
        severity: "error",
        artifact: "parse-only",
        message: expect.stringContaining("parse-only gzip-9 size"),
      }),
    ]);
  });
});

describe("formatFindings", () => {
  it("lists two manifest errors and two ceiling errors from different artifacts, each once, with repair instructions once", () => {
    const measuredByArtifact = matchingMeasuredByArtifact();
    measuredByArtifact["render-only"] = {
      ...documented.render,
      gzip9: documented.render.gzip9 + 1,
    };
    measuredByArtifact["parse-only"] = {
      ...documented.parse,
      finalWasm: documented.parse.finalWasm + 2,
    };

    const manifestFindings = compareManifest(measuredByArtifact, documented);
    const ceilingFindings = checkCeilings({
      default: { gzip9: ARTIFACTS.find((a) => a.label === "default").ceiling + 1 },
      "highlight-only": { gzip9: ARTIFACTS.find((a) => a.label === "highlight-only").ceiling + 1 },
      "render-only": { gzip9: 1_000 },
      "parse-only": { gzip9: 1_000 },
    });
    expect(manifestFindings).toHaveLength(2);
    expect(ceilingFindings).toHaveLength(2);

    const output = formatFindings([...ceilingFindings, ...manifestFindings]);
    const outputLines = output.split("\n");

    for (const finding of [...ceilingFindings, ...manifestFindings]) {
      expect(outputLines.filter((line) => line === finding.message)).toHaveLength(1);
    }
    expect(
      outputLines.filter((line) => line.includes("re-run with --update-manifest")),
    ).toHaveLength(1);
  });
});

describe("--self-test", () => {
  it("prints the OK line regardless of the runner's working directory", () => {
    // cwd is deliberately NOT the repo root: scriptPath is resolved from
    // import.meta.url, so the process must not depend on process.cwd().
    const output = execFileSync(process.execPath, [scriptPath, "--self-test"], {
      cwd: tmpdir(),
      encoding: "utf8",
    });
    expect(output).toContain("OK: build summary metric parser self-test");
  });
});
