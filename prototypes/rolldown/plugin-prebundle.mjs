// @verification: bounded option/resolution probe, not production adoption evidence.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const native = resolve("target/debug/examples/plugin_prebundle");
const esbuild = process.env.ZFB_ESBUILD_BIN;
assert.ok(esbuild, "ZFB_ESBUILD_BIN must name the pinned executable");
const out = resolve("target/rolldown-plugin-probe");
mkdirSync(out, { recursive: true });
const root = mkdtempSync(join(out, "fixture-"));
const banner =
  'import { createRequire as __zfbCreateRequire } from "node:module"; const require = __zfbCreateRequire(import.meta.url);';
const put = (path, text) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
};
const config = (path, compilerOptions, extra = {}) =>
  put(join(root, path), JSON.stringify({ ...extra, compilerOptions }));
const pkg = (path, value) => {
  put(
    join(path, "package.json"),
    JSON.stringify({
      name: "probe-dep",
      type: "module",
      exports: { ".": "./index.js", "./sub": "./index.js" },
    }),
  );
  put(join(path, "index.js"), `export const value = ${value};\n`);
};
const rootDep = join(root, "node_modules/probe-dep");
const nestedDep = join(root, "node_modules/installed-plugin/node_modules/probe-dep");
const lateDep = join(root, "node_modules/late-probe-dep");
pkg(rootDep, 31);
pkg(nestedDep, 73);
put(
  join(root, "package.json"),
  JSON.stringify({ type: "module", imports: { "#local": "./src/value.ts" } }),
);
put(
  join(root, "node_modules/installed-plugin/package.json"),
  '{"name":"installed-plugin","type":"module"}\n',
);
config("tsconfig.json", { baseUrl: ".", paths: { "@local/*": ["./src/*"] } });
put(join(root, "src/value.ts"), "export const value = 59;\n");
config("tsx/tsconfig.json", { jsx: "react", jsxFactory: "h" });
config("inherited/tsconfig.json", {}, { extends: "../tsconfig.json" });
config("base-url/tsconfig.json", { baseUrl: "./lib" });
put(join(root, "base-url/lib/local-value.ts"), "export const value = 61;\n");
config("shadow/tsconfig.json", { paths: { "probe-dep": ["./value.ts"] } });
put(join(root, "shadow/value.ts"), "export const value = 67;\n");
config("mapped-package/tsconfig.json", {
  paths: { "@mapped/source": ["../node_modules/mapped-source/value.ts"] },
});
put(join(root, "node_modules/mapped-source/value.ts"), "export const value = 79;\n");
put(join(root, "bad-config/tsconfig.json"), '{"compilerOptions": ');

const rows = [
  { name: "ts", entry: "plugin.ts", source: "export default () => 17;\n", expected: 17 },
  {
    name: "mts-package-subpath",
    entry: "plugin.mts",
    source: 'import { value } from "probe-dep/sub"; export default () => value;\n',
    expected: 31,
    mutation: { path: rootDep, value: 37, original: 31 },
  },
  {
    name: "cts-require",
    entry: "plugin.cts",
    source: 'const path = require("node:path"); export = () => path.basename("/a/b");\n',
    expected: "b",
  },
  {
    name: "own-tsconfig-jsx",
    entry: "tsx/plugin.tsx",
    source: "const h = (tag: string) => tag; export default () => <probe />;\n",
    expected: "probe",
  },
  {
    name: "installed-nested-package",
    entry: "node_modules/installed-plugin/plugin.ts",
    source: 'import { value } from "probe-dep"; export default () => value;\n',
    expected: 73,
    mutation: { path: nestedDep, value: 83, original: 73 },
  },
  {
    name: "tsconfig-paths",
    entry: "alias.ts",
    source: 'import { value } from "@local/value"; export default () => value;\n',
    expected: 59,
  },
  {
    name: "mapped-throw",
    entry: "throw.ts",
    source:
      "export default function probe() {\n  const value: number = 1;\n  throw new Error(`plugin-probe-${value}`);\n}\n",
    throws: /plugin-probe-1/,
  },
  {
    name: "missing-relative-import",
    entry: "missing.ts",
    source: 'import "./absent-source.ts"; export default () => 0;\n',
    buildError: /absent-source/,
  },
  {
    name: "inherited-paths",
    entry: "inherited/plugin.ts",
    source: 'import { value } from "@local/value"; export default () => value;\n',
    expected: 59,
  },
  {
    name: "base-url-local",
    entry: "base-url/plugin.ts",
    source: 'import { value } from "local-value"; export default () => value;\n',
    expected: 61,
  },
  {
    name: "alias-shadows-package",
    entry: "shadow/plugin.ts",
    source: 'import { value } from "probe-dep"; export default () => value;\n',
    expected: 67,
  },
  {
    name: "alias-into-node-modules",
    entry: "mapped-package/plugin.ts",
    source: 'import { value } from "@mapped/source"; export default () => value;\n',
    expected: 79,
  },
  {
    name: "dynamic-alias",
    entry: "dynamic.ts",
    source: 'export default async () => (await import("@local/value")).value;\n',
    expected: 59,
  },
  {
    name: "cts-alias",
    entry: "alias.cts",
    source: 'const { value } = require("@local/value"); export = () => value;\n',
    expected: 59,
  },
  {
    name: "late-installed-package",
    entry: "late.ts",
    source: 'import { value } from "late-probe-dep"; export default () => value;\n',
    expected: 89,
    latePackage: true,
  },
  {
    name: "package-import-local",
    entry: "package-import.ts",
    source: 'import { value } from "#local"; export default () => value;\n',
    expected: 59,
  },
  {
    name: "missing-package-import",
    entry: "missing-package-import.ts",
    source: 'import { value } from "#absent"; export default () => value;\n',
    buildError: /#absent/,
  },
  {
    name: "invalid-tsconfig",
    entry: "bad-config/plugin.ts",
    source: 'import { value } from "probe-dep"; export default () => value;\n',
    buildError: /tsconfig\.json/,
  },
];
const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
const report = {
  source: process.env.ZFB_PROBE_SOURCE ?? process.env.GITHUB_SHA ?? "local-unrecorded",
  node: process.version,
  platform: process.platform,
  arch: process.arch,
  nativeSha256: hash(native),
  esbuildSha256: hash(esbuild),
  cargoLockSha256: hash("Cargo.lock"),
  fixture: root,
  scope: "isolated native options; unchanged production caller and Node host",
  lifecycle: "not verified: successful close is not active-build cancellation/quiescence",
  rows: [],
};
const run = (executable, args, timeout, log) => {
  const result = spawnSync(executable, args, {
    cwd: root,
    encoding: "utf8",
    timeout,
    killSignal: "SIGKILL",
    maxBuffer: 4 * 1024 * 1024,
  });
  put(join(out, log), `${result.stdout ?? ""}\n${result.stderr ?? ""}\n${result.error ?? ""}`);
  assert.ifError(result.error);
  return result;
};
let failures = 0;
for (const test of rows) {
  const { name, source, expected } = test;
  const entry = join(root, test.entry);
  put(entry, source);
  for (const backend of ["esbuild", "rolldown"]) {
    const outfile = join(dirname(entry), `.zfb-plugin-probe-${backend}-${name}.mjs`);
    const row = { name, backend, expected, result: "failed" };
    try {
      if (test.latePackage) rmSync(lateDep, { recursive: true, force: true });
      const args =
        backend === "esbuild"
          ? [
              entry,
              "--bundle",
              "--format=esm",
              "--platform=node",
              "--target=node22",
              "--packages=external",
              "--sourcemap=inline",
              "--log-level=warning",
              "--color=false",
              `--banner:js=${banner}`,
              `--outfile=${outfile}`,
            ]
          : [entry, outfile];
      const build = run(
        backend === "esbuild" ? esbuild : native,
        args,
        30000,
        `${name}-${backend}-build.log`,
      );
      row.buildStatus = build.status;
      if (test.buildError) {
        assert.notEqual(build.status, 0, "invalid input must fail at build time");
        assert.match(build.stderr, test.buildError);
      } else {
        assert.equal(build.status, 0, build.stderr);
        if (backend === "rolldown")
          assert.match(build.stderr, /zfb-plugin-probe: native rolldown 24bc2d0/);
        assert.match(readFileSync(outfile, "utf8"), /sourceMappingURL=data:/);
        if (test.latePackage) pkg(lateDep, expected);
        const expression = `const m = await import(${JSON.stringify(pathToFileURL(outfile).href)}); console.log(JSON.stringify(typeof m.default === 'function' ? await m.default() : m.default));`;
        const execute = (suffix) =>
          run(
            process.execPath,
            ["--enable-source-maps", "--input-type=module", "-e", expression],
            10000,
            `${name}-${backend}-${suffix}.log`,
          );
        const execution = execute("execute");
        row.executeStatus = execution.status;
        if (test.throws) {
          assert.notEqual(execution.status, 0);
          assert.match(execution.stderr, test.throws);
          assert.match(execution.stderr, /throw\.ts:3:/, "mapped authored line required");
        } else {
          assert.equal(execution.status, 0, execution.stderr);
          row.actual = JSON.parse(execution.stdout.trim());
          assert.deepEqual(row.actual, expected);
          if (test.mutation) {
            // Change the dependency AFTER bundling, then use a fresh Node host.
            // Equality before mutation alone would allow accidental inlining.
            pkg(test.mutation.path, test.mutation.value);
            const changed = execute("after-package-edit");
            assert.equal(changed.status, 0, changed.stderr);
            row.afterPackageEdit = JSON.parse(changed.stdout.trim());
            assert.deepEqual(row.afterPackageEdit, test.mutation.value);
          }
        }
      }
      row.result = "passed";
    } catch (error) {
      failures += 1;
      row.error = String(error);
    } finally {
      if (test.mutation) pkg(test.mutation.path, test.mutation.original);
      if (test.latePackage) rmSync(lateDep, { recursive: true, force: true });
    }
    report.rows.push(row);
    console.log(`${backend} ${name}: ${row.result}`);
  }
}
report.failures = failures;
writeFileSync(join(out, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
// A failed candidate is a retained capability gap, never a successful migration.
process.exitCode = failures === 0 ? 0 : 1;
