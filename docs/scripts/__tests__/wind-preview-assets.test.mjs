import { spawnSync } from "node:child_process";
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  assetUrl,
  assertDiagnostics,
  assertFresh,
  exampleAssetPath,
  exampleSource,
  exampleInput,
  exampleSources,
  exampleCompilerConfig,
  generateAssets,
  loadRecords,
  mergeConfig,
  REPO_ROOT,
  validateRecord,
} from "../wind-preview-assets.mjs";

const seed = () => ({
  schemaVersion: 1,
  family: "padding",
  examples: [
    {
      id: "all-sides",
      kind: "positive",
      html: '<div class="wind-demo p-4">Padding</div>',
      utilities: ["p-4"],
      authoredClasses: ["wind-demo"],
      scaffoldCss: ".wind-demo { color: #203046; }",
    },
  ],
});
const diagnosticSeed = () => ({
  schemaVersion: 1,
  family: "diagnostics",
  examples: [
    {
      id: "missing-spacing-unit",
      kind: "expected-diagnostic",
      html: '<div class="p-4">Missing token</div>',
      utilities: ["p-4"],
      authoredClasses: [],
      scaffoldCss: "",
      config: { wind: { tokens: { spacingUnit: null } } },
      expectedDiagnostics: [{ code: "ZW006", severity: "error" }],
    },
    {
      id: "unknown-explicit",
      kind: "expected-diagnostic",
      html: '<div class="unknown-utility">Unknown</div>',
      utilities: ["unknown-utility"],
      authoredClasses: [],
      scaffoldCss: "",
      expectedDiagnostics: [{ code: "ZW008", severity: "error" }],
    },
    {
      id: "foreign-explicit",
      kind: "expected-diagnostic",
      html: '<div class="line-clamp-2">Unsupported</div>',
      utilities: ["line-clamp-2"],
      authoredClasses: [],
      scaffoldCss: "",
      expectedDiagnostics: [{ code: "ZW014", severity: "error" }],
    },
  ],
});
test("seed schema and compiler/source intent are explicit", () => {
  for (const family of ["gap", "padding"])
    assert.ok(
      loadRecords()
        .find((record) => record.family === family)
        ?.examples.some((example) => example.kind === "positive"),
    );
  const config = JSON.parse(readFileSync(join(REPO_ROOT, "docs/wind-examples/base-config.json")));
  assert.equal(config.wind.tokens.spacingUnit, "0.25rem");
  assert.equal(config.wind.reset, "none");
  assert.equal(config.wind.strict, true);
  for (const record of loadRecords()) assert.equal(validateRecord(record), record);
});
test("undeclared ordinary classes cannot silently pass a positive sample", () => {
  const record = seed();
  record.examples[0].html = '<div class="p-4 ordinary-name"></div>';
  assert.throws(() => validateRecord(record), /declared utility\/scaffold intent/);
  record.examples[0].utilities.push("ordinary-name");
  record.examples[0].authoredClasses = [];
  record.examples[0].scaffoldCss = "";
  assert.doesNotThrow(() => validateRecord(record)); // strict compiler checks it
  assert.throws(
    () => assertDiagnostics(record.examples[0], { status: 1, stderr: "ZW008 ordinary-name" }),
    /positive sample failed/,
  );
});
test("uppercase HTML class attributes cannot evade candidate intent", () => {
  const record = seed();
  record.examples[0].html = '<div CLASS="wind-demo p-4 unknown-class">Example</div>';
  assert.throws(() => validateRecord(record), /declared utility\/scaffold intent/);
  record.examples[0].html = '<div CLASS="wind-demo p-4">Example</div>';
  assert.doesNotThrow(() => validateRecord(record));
});
test("scaffolding cannot emulate utilities or overlap utility intent", () => {
  const record = seed();
  record.examples[0].scaffoldCss = ".p-4 { padding: 1rem; }";
  assert.throws(() => validateRecord(record), /scaffold selector/);
  record.examples[0].authoredClasses.push("p-4");
  assert.throws(() => validateRecord(record), /overlapping/);
});
test("static source and candidate-source configuration cannot bypass validation", () => {
  const record = seed();
  record.examples[0].config = { wind: { strict: false } };
  assert.throws(() => validateRecord(record), /strictness/);
  delete record.examples[0].config;
  record.examples[0].html += "<style>.p-4 {padding:0}</style>";
  assert.throws(() => validateRecord(record), /no embedded styles/);
});
test("fragment metadata is restricted to the native no-script preview base", () => {
  const record = seed();
  record.examples[0].head = '<base href="about:srcdoc">';
  assert.doesNotThrow(() => validateRecord(record));
  for (const head of [
    "<script>alert(1)</script>",
    "<style>body{margin:0}</style>",
    '<base href="/">',
  ]) {
    record.examples[0].head = head;
    assert.throws(() => validateRecord(record), /about:srcdoc fragment base/);
  }
  const diagnostic = diagnosticSeed();
  diagnostic.examples[0].head = '<base href="about:srcdoc">';
  assert.throws(() => validateRecord(diagnostic), /only positive examples/);
});
test("only negative fixtures may select source origin and non-strict warning behavior", () => {
  const record = seed();
  record.examples[0].candidateOrigin = "source";
  assert.throws(() => validateRecord(record), /only diagnostic fixtures/);
  const negative = diagnosticSeed();
  negative.examples[0].candidateOrigin = "source";
  negative.examples[0].config = { wind: { strict: false } };
  assert.doesNotThrow(() => validateRecord(negative));
  const warning = {
    ...negative.examples[0],
    expectedDiagnostics: [{ code: "ZW014", severity: "warning" }],
  };
  assert.doesNotThrow(() =>
    assertDiagnostics(warning, { status: 0, stderr: "ZW014 line-clamp-2" }),
  );
  assert.throws(
    () => assertDiagnostics(warning, { status: 1, stderr: "ZW014 line-clamp-2" }),
    /mismatch/,
  );
});
test("CSS and source-plan lessons cannot relax positive validation", () => {
  const record = seed();
  record.examples[0].diagnosticStylesheet = "@apply p-4;";
  assert.throws(() => validateRecord(record), /diagnosticStylesheet/);
  delete record.examples[0].diagnosticStylesheet;
  record.examples[0].sourceExclusion = "partial";
  assert.throws(() => validateRecord(record), /sourceExclusion/);
  const negative = diagnosticSeed();
  const example = negative.examples[0];
  example.diagnosticStylesheet = "@apply p-4;";
  example.expectedDiagnostics = [{ code: "ZW009", severity: "error" }];
  assert.doesNotThrow(() => validateRecord(negative));
  assert.equal(exampleInput(example), "@apply p-4;\n");
  example.diagnosticStylesheet += '\n@import "foreign.css";';
  assert.throws(() => validateRecord(negative), /without imports/);
  delete example.diagnosticStylesheet;
  for (const mode of ["all", "partial"]) {
    example.sourceExclusion = mode;
    example.expectedDiagnostics = [
      { code: "ZW010", severity: mode === "all" ? "error" : "warning" },
    ];
    assert.doesNotThrow(() => validateRecord(negative));
    const base = { wind: { strict: true, tokens: { spacingUnit: "0.25rem" } } };
    assert.deepEqual(exampleCompilerConfig(base, example).wind.sources, {
      exclude: ["excluded.html"],
    });
    assert.equal(base.wind.sources, undefined);
    assert.deepEqual(exampleSources(example), [mode === "all" ? "excluded.html" : "*.html"]);
    example.expectedDiagnostics[0].severity = mode === "all" ? "warning" : "error";
    assert.throws(() => validateRecord(negative), /matching diagnostic-only/);
  }
});
test("positive warning-only ZW014 and missing-token failures are rejected", () => {
  const example = seed().examples[0];
  assert.throws(
    () => assertDiagnostics(example, { status: 0, stderr: "Warning: ZW014 container" }),
    /positive sample failed/,
  );
  assert.throws(
    () => assertDiagnostics(example, { status: 1, stderr: "ZW006 p-4 missing spacingUnit" }),
    /positive sample failed/,
  );
});
test("diagnostic fixtures require exact code and error/warning outcome", () => {
  const example = diagnosticSeed().examples[0];
  assert.doesNotThrow(() => assertDiagnostics(example, { status: 1, stderr: "ZW006 p-4" }));
  for (const result of [
    { status: 0, stderr: "ZW006" },
    { status: 1, stderr: "ZW008" },
    { status: 1, stderr: "ZW006 ZW014" },
  ])
    assert.throws(() => assertDiagnostics(example, result), /mismatch/);
  assert.throws(
    () => assertDiagnostics(example, { status: null, signal: "SIGTERM" }),
    /process failed/,
  );
});
test("scoped overrides remove a missing token without mutating shared config", () => {
  const base = { wind: { tokens: { spacingUnit: "0.25rem" }, strict: true } };
  assert.deepEqual(mergeConfig(base, { wind: { tokens: { spacingUnit: null } } }), {
    wind: { tokens: {}, strict: true },
  });
  assert.equal(base.wind.tokens.spacingUnit, "0.25rem");
});
test("asset identity works under a deployed base path", () => {
  assert.equal(exampleSource({ html: "<div>Exact source</div>" }), "<div>Exact source</div>\n");
  const path = exampleAssetPath("gap", "grid-gap");
  assert.equal(assetUrl(path), "/wind-examples/gap/grid-gap.css");
  assert.equal(assetUrl(path, "/project/"), "/project/wind-examples/gap/grid-gap.css");
  assert.throws(() => exampleAssetPath("../gap", "grid-gap"), /Invalid/);
});

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "wind-preview-test-"));
  mkdirSync(join(root, "docs/wind-examples"), { recursive: true });
  const negatives = diagnosticSeed();
  for (const strict of [false, true])
    negatives.examples.push({
      id: strict ? "source-strict" : "source-warning",
      kind: "expected-diagnostic",
      candidateOrigin: "source",
      html: '<div class="line-clamp-2">line-clamp-2</div>',
      utilities: ["line-clamp-2"],
      authoredClasses: [],
      scaffoldCss: "",
      config: { wind: { strict } },
      expectedDiagnostics: [{ code: "ZW014", severity: strict ? "error" : "warning" }],
    });
  const fixed = {
    "base-config": {
      wind: { spec: 1, reset: "none", strict: true, tokens: { spacingUnit: "0.25rem" } },
    },
    compiler: { version: "0.0.0" },
    padding: seed(),
    diagnostics: negatives,
  };
  for (const [name, record] of Object.entries(fixed))
    writeFileSync(join(root, "docs/wind-examples", `${name}.json`), JSON.stringify(record));
  mkdirSync(join(root, "crates/compiler"), { recursive: true });
  writeFileSync(join(root, "crates/compiler/lib.rs"), "// compiler source\n");
  for (const name of ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"])
    writeFileSync(join(root, name), "test\n");
  const calls = [];
  const run = (_compiler, args) => {
    if (args[0] === "-V") return { status: 0, stdout: "zfb 0.0.0\n" };
    calls.push(args);
    const config = JSON.parse(readFileSync(args[args.indexOf("--config") + 1]));
    const candidates = config.wind.safelist["docs-preview"] ?? [];
    if (!config.wind.tokens.spacingUnit) return { status: 1, stderr: "ZW006 p-4" };
    if (candidates.includes("unknown-utility"))
      return { status: 1, stderr: "ZW008 unknown-utility" };
    const sourceHtml = readFileSync(
      join(args[args.indexOf("--project-root") + 1], "sample.html"),
      "utf8",
    );
    if (candidates.includes("line-clamp-2") || sourceHtml.includes("line-clamp-2"))
      return {
        status: candidates.length || config.wind.strict ? 1 : 0,
        stderr: "ZW014 line-clamp-2",
      };
    writeFileSync(args[args.indexOf("--output") + 1], ".mock-compiled { display: block; }\n\n");
    return { status: 0, stdout: "", stderr: "" };
  };
  return { root, run, calls };
}
test("fragment metadata changes freshness even when compiled CSS is unchanged", () => {
  const { root, run } = fixture();
  try {
    const options = { root, compiler: "/workspace/target/debug/zfb", run };
    const first = generateAssets(options);
    const recordPath = join(root, "docs/wind-examples/padding.json");
    const record = JSON.parse(readFileSync(recordPath, "utf8"));
    record.examples[0].head = '<base href="about:srcdoc">';
    writeFileSync(recordPath, JSON.stringify(record));
    assert.throws(() => generateAssets({ ...options, check: true }), /manifest.json/);
    const second = generateAssets(options);
    const before = first.examples.find((entry) => entry.family === "padding");
    const after = second.examples.find((entry) => entry.family === "padding");
    assert.equal(before.cssSha256, after.cssSha256);
    assert.equal(before.htmlSha256, after.htmlSha256);
    assert.notEqual(before.headSha256, after.headSha256);
    assert.doesNotThrow(() => generateAssets({ ...options, check: true }));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("source exclusion fixtures use exact shared files and stay freshness checked", () => {
  const { root, run } = fixture();
  try {
    const path = join(root, "docs/wind-examples/source-plan.json");
    const record = {
      schemaVersion: 1,
      family: "source-plan",
      examples: [
        {
          ...seed().examples[0],
          id: "excluded",
          kind: "expected-diagnostic",
          sourceExclusion: "all",
          expectedDiagnostics: [{ code: "ZW010", severity: "error" }],
        },
      ],
    };
    writeFileSync(path, JSON.stringify(record));
    const inspect = (compiler, args) => {
      if (args[0] === "-V") return run(compiler, args);
      const config = JSON.parse(readFileSync(args[args.indexOf("--config") + 1], "utf8"));
      if (!config.wind.sources) return run(compiler, args);
      const workspace = args[args.indexOf("--project-root") + 1];
      assert.equal(
        readFileSync(join(workspace, "sample.html"), "utf8"),
        exampleSource(record.examples[0]),
      );
      assert.equal(
        readFileSync(join(workspace, "excluded.html"), "utf8"),
        exampleSource(record.examples[0]),
      );
      assert.deepEqual(config.wind.sources, { exclude: ["excluded.html"] });
      const source = args[args.indexOf("--source") + 1];
      assert.equal(
        source,
        record.examples[0].sourceExclusion === "all" ? "excluded.html" : "*.html",
      );
      return { status: source === "excluded.html" ? 1 : 0, stderr: "ZW010 excluded source" };
    };
    const options = { root, run: inspect, compiler: "/workspace/target/debug/zfb" };
    const first = generateAssets(options);
    assert.equal(
      first.examples.find((entry) => entry.family === "source-plan").sourceExclusion,
      "all",
    );
    assert.doesNotThrow(() => generateAssets({ ...options, check: true }));
    record.examples[0].sourceExclusion = "partial";
    record.examples[0].expectedDiagnostics[0].severity = "warning";
    writeFileSync(path, JSON.stringify(record));
    assert.throws(() => generateAssets({ ...options, check: true }), /manifest.json/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("mock compiler pins flags and deterministic provenance; stale CSS fails without rewriting", () => {
  const { root, run, calls } = fixture();
  try {
    const options = { root, compiler: "/workspace/target/debug/zfb", run };
    const first = generateAssets(options);
    const second = generateAssets({ ...options, check: true });
    for (const id of ["source-warning", "source-strict"])
      assert.equal(first.examples.find((entry) => entry.id === id).candidateOrigin, "source");
    assert.deepEqual(first, second);
    assert.equal(calls.length, first.examples.length * 2);
    for (const args of calls) {
      assert.ok(args.includes("--no-auto-source"));
      assert.ok(args.includes("--no-default-highlight-styles"));
      for (const flag of ["--input", "--output", "--config", "--project-root", "--source"])
        assert.ok(args.includes(flag));
    }
    const output = join(root, "docs/public/wind-examples/padding/all-sides.css");
    assert.equal(readFileSync(output, "utf8"), ".mock-compiled { display: block; }\n");
    writeFileSync(output, "stale\n");
    assert.throws(() => generateAssets({ ...options, check: true }), /Stale wind preview asset/);
    assert.equal(readFileSync(output, "utf8"), "stale\n");
    generateAssets(options);
    writeFileSync(join(root, "docs/public/wind-examples/removed.css"), "orphan\n");
    assert.throws(() => generateAssets({ ...options, check: true }), /inventory/);
    assert.equal(first.examples.find((entry) => entry.family === "padding").htmlSha256.length, 64);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("compiler changes require new provenance; published docs compiler cannot satisfy pin", () => {
  const { root, run } = fixture();
  try {
    const options = { root, compiler: "/workspace/target/debug/zfb", run };
    generateAssets(options);
    writeFileSync(join(root, "crates/compiler/lib.rs"), "// changed compiler\n");
    assert.throws(() => generateAssets({ ...options, check: true }), /manifest.json/);
    generateAssets(options);
    writeFileSync(join(root, "crates/compiler/reset.css"), "body { margin: 0; }\n");
    assert.throws(() => generateAssets({ ...options, check: true }), /manifest.json/);
    generateAssets(options);
    mkdirSync(join(root, "crates/compiler/target"));
    writeFileSync(join(root, "crates/compiler/target/ignored.json"), "ignored generated state");
    assert.doesNotThrow(() => generateAssets({ ...options, check: true }));
    assert.throws(() => generateAssets({ ...options, compiler: "zfb" }), /absolute workspace/);
    assert.throws(
      () => generateAssets({ ...options, run: () => ({ status: 0, stdout: "zfb 2.20.2\n" }) }),
      /Expected workspace/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
test("missing artifact fails freshness", () => {
  assert.throws(
    () => assertFresh("/nonexistent-wind-preview-test-path", new Map([["sample.css", "css"]])),
    /inventory/,
  );
});

test("required Docs gate fails closed for preview failures, cancellation and unexplained skips", () => {
  const workflow = readFileSync(join(REPO_ROOT, ".github/workflows/docs-checks.yml"), "utf8");
  assert.match(workflow, /needs: \[changed-files, build, wind-previews\]/);
  assert.match(workflow, /wind-previews:[\s\S]*?--compiler "\$PWD\/target\/debug\/zfb" --check/);
  const gate = workflow.split("      - name: Report gate verdict")[1];
  assert.ok(gate);
  const script = gate
    .split("        run: |\n")[1]
    .split("\n")
    .map((line) => line.replace(/^          /, ""))
    .join("\n");
  const evaluate = (overrides) =>
    spawnSync("bash", ["-c", script], {
      encoding: "utf8",
      env: {
        ...process.env,
        DETECTOR_RESULT: "success",
        DOCS_CHANGED: "false",
        BUILD_RESULT: "skipped",
        WIND_CHANGED: "true",
        WIND_RESULT: "success",
        ...overrides,
      },
    }).status;
  assert.equal(evaluate({}), 0);
  assert.equal(evaluate({ WIND_CHANGED: "false", WIND_RESULT: "skipped" }), 0);
  for (const overrides of [
    { WIND_RESULT: "failure" },
    { WIND_RESULT: "cancelled" },
    { WIND_RESULT: "skipped" },
    { WIND_CHANGED: "", WIND_RESULT: "skipped" },
    { DETECTOR_RESULT: "failure" },
    { DETECTOR_RESULT: "cancelled" },
    { BUILD_RESULT: "failure" },
    { BUILD_RESULT: "cancelled" },
  ])
    assert.equal(evaluate(overrides), 1, JSON.stringify(overrides));
});
