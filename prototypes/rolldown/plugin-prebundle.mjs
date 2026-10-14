// @verification: bounded option/resolution probe, not production adoption evidence.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const native = resolve('target/debug/examples/plugin_prebundle');
const esbuild = process.env.ZFB_ESBUILD_BIN;
assert.ok(esbuild, 'ZFB_ESBUILD_BIN must name the pinned executable');
const out = resolve('target/rolldown-plugin-probe');
mkdirSync(out, { recursive: true });
const root = mkdtempSync(join(out, 'fixture-'));
const banner = 'import { createRequire as __zfbCreateRequire } from "node:module"; const require = __zfbCreateRequire(import.meta.url);';
const put = (path, text) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
};
const pkg = (path, value) => {
  put(join(path, 'package.json'), JSON.stringify({ name: 'probe-dep', type: 'module', exports: { '.': './index.js', './sub': './index.js' } }));
  put(join(path, 'index.js'), `export const value = ${value};\n`);
};
pkg(join(root, 'node_modules/probe-dep'), 31);
pkg(join(root, 'node_modules/installed-plugin/node_modules/probe-dep'), 73);
put(join(root, 'package.json'), '{"type":"module"}\n');
put(join(root, 'node_modules/installed-plugin/package.json'), '{"name":"installed-plugin","type":"module"}\n');
put(join(root, 'tsconfig.json'), JSON.stringify({ compilerOptions: { baseUrl: '.', paths: { '@local/*': ['./src/*'] } } }));
put(join(root, 'src/value.ts'), 'export const value = 59;\n');
put(join(root, 'tsx/tsconfig.json'), JSON.stringify({ compilerOptions: { jsx: 'react', jsxFactory: 'h' } }));
const rows = [
  ['ts', 'plugin.ts', 'export default () => 17;\n', 17],
  ['mts-package-subpath', 'plugin.mts', 'import { value } from "probe-dep/sub"; export default () => value;\n', 31],
  ['cts-require', 'plugin.cts', 'const path = require("node:path"); export = () => path.basename("/a/b");\n', 'b'],
  ['own-tsconfig-jsx', 'tsx/plugin.tsx', 'const h = (tag: string) => tag; export default () => <probe />;\n', 'probe'],
  ['installed-nested-package', 'node_modules/installed-plugin/plugin.ts', 'import { value } from "probe-dep"; export default () => value;\n', 73],
  ['tsconfig-paths', 'alias.ts', 'import { value } from "@local/value"; export default () => value;\n', 59],
  ['mapped-throw', 'throw.ts', 'export default function probe() {\n  const value: number = 1;\n  throw new Error(`plugin-probe-${value}`);\n}\n', null],
  ['missing-relative-import', 'missing.ts', 'import "./absent-source.ts"; export default () => 0;\n', null],
];
const report = {
  source: process.env.GITHUB_SHA ?? 'local-unrecorded',
  node: process.version,
  platform: process.platform,
  arch: process.arch,
  nativeSha256: createHash('sha256').update(readFileSync(native)).digest('hex'),
  esbuildSha256: createHash('sha256').update(readFileSync(esbuild)).digest('hex'),
  fixture: root,
  scope: 'isolated native options; unchanged production caller and Node host',
  lifecycle: 'not verified: successful close is not active-build cancellation/quiescence',
  rows: [],
};
let failures = 0;
for (const [name, relative, source, expected] of rows) {
  const entry = join(root, relative);
  put(entry, source);
  for (const backend of ['esbuild', 'rolldown']) {
    const outfile = join(dirname(entry), `.zfb-plugin-probe-${backend}-${name}.mjs`);
    const args = backend === 'esbuild'
      ? [entry, '--bundle', '--format=esm', '--platform=node', '--target=node22', '--packages=external', '--sourcemap=inline', '--log-level=warning', '--color=false', `--banner:js=${banner}`, `--outfile=${outfile}`]
      : [entry, outfile];
    const build = spawnSync(backend === 'esbuild' ? esbuild : native, args, { cwd: root, encoding: 'utf8', timeout: 30000, maxBuffer: 4 * 1024 * 1024 });
    put(join(out, `${name}-${backend}-build.log`), `${build.stdout ?? ''}\n${build.stderr ?? ''}\n${build.error ?? ''}`);
    const row = { name, backend, buildStatus: build.status, expected, result: 'failed' };
    try {
      assert.ifError(build.error);
      if (name === 'missing-relative-import') {
        assert.notEqual(build.status, 0, 'missing relative source must fail');
        assert.match(build.stderr, /absent-source/);
      } else {
        assert.equal(build.status, 0, build.stderr);
        if (backend === 'rolldown') assert.match(build.stderr, /zfb-plugin-probe: native rolldown 24bc2d0/);
        assert.match(readFileSync(outfile, 'utf8'), /sourceMappingURL=data:/, 'real inline source map required');
        const expression = `const m = await import(${JSON.stringify(pathToFileURL(outfile).href)}); console.log(JSON.stringify(typeof m.default === 'function' ? m.default() : m.default));`;
        const execute = spawnSync(process.execPath, ['--enable-source-maps', '--input-type=module', '-e', expression], { cwd: root, encoding: 'utf8', timeout: 10000, maxBuffer: 1024 * 1024 });
        put(join(out, `${name}-${backend}-execute.log`), `${execute.stdout ?? ''}\n${execute.stderr ?? ''}\n${execute.error ?? ''}`);
        assert.ifError(execute.error);
        row.executeStatus = execute.status;
        if (name === 'mapped-throw') {
          assert.notEqual(execute.status, 0);
          assert.match(execute.stderr, /plugin-probe-1/);
          assert.match(execute.stderr, /throw\.ts:3:/, 'mapped authored line required');
        } else {
          assert.equal(execute.status, 0, execute.stderr);
          row.actual = JSON.parse(execute.stdout.trim());
          assert.deepEqual(row.actual, expected);
        }
      }
      row.result = 'passed';
    } catch (error) {
      failures += 1;
      row.error = String(error);
    }
    report.rows.push(row);
    console.log(`${backend} ${name}: ${row.result}`);
  }
}
report.failures = failures;
writeFileSync(join(out, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
// A failed candidate is a retained capability gap, never a successful migration.
process.exitCode = failures === 0 ? 0 : 1;
