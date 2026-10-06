import { readFileSync, readdirSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { dirname, join, resolve, relative } from 'node:path';
import { tmpdir } from 'node:os';
import { gzipSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';

const args = Object.fromEntries(process.argv.slice(2).reduce((pairs, value, index, all) => {
  if (index % 2 === 0) pairs.push([value, all[index + 1]]);
  return pairs;
}, []));
for (const key of ['--before', '--after', '--baseline', '--candidate', '--candidate-version', '--out']) {
  if (!args[key]) throw new Error(`missing ${key}`);
}
for (const key of ['--baseline', '--candidate'])
  if (!/^[0-9a-f]{40}$/.test(args[key])) throw new Error(`invalid ${key}`);
if (!/^\d+\.\d+\.\d+(?:-next\.\d+)?$/.test(args['--candidate-version']))
  throw new Error('invalid --candidate-version');
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const read = (dir, name) => readFileSync(join(dir, name));
const line = (dir, name) => read(dir, name).toString().trim();
const parse = (dir, name) => JSON.parse(read(dir, name));
const walk = dir => readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
  const path = join(dir, entry.name);
  if (entry.isDirectory()) return walk(path);
  if (!entry.isFile()) throw new Error(`unexpected non-file: ${path}`);
  return [path];
}).sort();
const expectedToolchain = { node: 'v24.14.0', zlib: '1.3.1-e00f703', pnpm: '12.8.2',
  rust: '1.99.0', esbuild: '0.25.12', hono: '4.12.25' };
const sourceInputPaths = ['Cargo.lock', 'pnpm-lock.yaml', 'packages/zfb/package.json',
  'packages/zfb-runtime/package.json'];
const observedBinaryPaths = ['target/debug/zfb', 'crates/zfb/binaries/esbuild/esbuild'];
const errors = [];
if (process.version !== expectedToolchain.node || process.versions.zlib !== expectedToolchain.zlib ||
    process.platform !== 'linux' || process.arch !== 'x64') {
  throw new Error(`unsupported comparator runtime: ${process.version} zlib ${process.versions.zlib} ${process.platform}/${process.arch}`);
}
function examine(dir, expectedSha) {
  const sourceSha = line(dir, 'source-sha.txt');
  if (sourceSha !== expectedSha) errors.push(`${dir}: wrong product source SHA`);
  if (line(dir, 'original-status.txt')) errors.push(`${dir}: product source was dirty before instrumentation`);
  const patch = read(dir, 'instrumentation.patch');
  if (!patch.length) errors.push(`${dir}: empty instrumentation patch`);
  const patchHash = sha(patch);
  if (line(dir, 'patch-hash.txt').split(/\s+/)[0] !== patchHash)
    errors.push(`${dir}: patch hash mismatch`);
  const harnessHashes = line(dir, 'harness-hashes.txt');
  const harnessEntries = harnessHashes.split('\n').map(row => row.trim().split(/\s+/));
  const expectedHarness = ['compare-router-probe.mjs', 'instrument-router-probe.py', 'inventory-router-probe.mjs'];
  if (JSON.stringify(harnessEntries.map(row => row[1]).sort()) !== JSON.stringify(expectedHarness))
    errors.push(`${dir}: incomplete harness hashes`);
  const harnessDir = dirname(fileURLToPath(import.meta.url));
  for (const [recordedHash, name] of harnessEntries) {
    if (expectedHarness.includes(name) && recordedHash !== sha(readFileSync(join(harnessDir, name))))
      errors.push(`${dir}: ${name} differs from the reviewed probe branch`);
  }
  const probeIdentity = sha(Buffer.from(`${sourceSha}\n${line(dir, 'patch-hash.txt')}\n${harnessHashes}\n`));
  if (line(dir, 'probe-identity.txt') !== probeIdentity)
    errors.push(`${dir}: tested probe identity mismatch`);
  const instrumented = parse(dir, 'instrumentation.json');
  if (instrumented.path !== 'crates/zfb/tests/client_router_autoinclude_build.rs' ||
      !/^[0-9a-f]{64}$/.test(instrumented.originalSha256 ?? '') ||
      !/^[0-9a-f]{64}$/.test(instrumented.instrumentedSha256 ?? ''))
    errors.push(`${dir}: incomplete instrumentation identity`);
  const sourcePath = 'crates/zfb/tests/client_router_autoinclude_build.rs';
  const retainedOriginal = read(dir, 'source-test-original.rs');
  const retainedInstrumented = read(dir, 'source-test-instrumented.rs');
  let pinnedOriginal;
  try {
    pinnedOriginal = execFileSync('git', ['show', `${expectedSha}:${sourcePath}`],
      { maxBuffer: 10 * 1024 * 1024 });
  } catch (error) {
    errors.push(`${dir}: pinned original router test unavailable: ${error.message}`);
  }
  if (pinnedOriginal && !pinnedOriginal.equals(retainedOriginal))
    errors.push(`${dir}: retained router test differs from pinned product source`);
  if (instrumented.originalSha256 !== sha(retainedOriginal) ||
      instrumented.instrumentedSha256 !== sha(retainedInstrumented))
    errors.push(`${dir}: retained original/instrumented router test hash mismatch`);
  const replayRoot = mkdtempSync(join(tmpdir(), 'zfb-router-probe-replay-'));
  try {
    const target = join(replayRoot, sourcePath);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, retainedOriginal);
    const patchPath = resolve(dir, 'instrumentation.patch');
    execFileSync('git', ['apply', '--check', patchPath], { cwd: replayRoot });
    execFileSync('git', ['apply', patchPath], { cwd: replayRoot });
    if (!readFileSync(target).equals(retainedInstrumented))
      errors.push(`${dir}: retained instrumentation patch does not reproduce test bytes`);
  } catch (error) {
    errors.push(`${dir}: instrumentation patch replay failed: ${error.message}`);
  } finally {
    rmSync(replayRoot, { recursive: true, force: true });
  }
  const recordedInputs = new Map();
  for (const row of line(dir, 'input-hashes.txt').split('\n')) {
    const match = /^([0-9a-f]{64})  (.+)$/.exec(row);
    if (!match || recordedInputs.has(match[2])) {
      errors.push(`${dir}: malformed or duplicate input hash row`);
      continue;
    }
    recordedInputs.set(match[2], match[1]);
  }
  const expectedInputs = [...sourceInputPaths, ...observedBinaryPaths].sort();
  if (JSON.stringify([...recordedInputs.keys()].sort()) !== JSON.stringify(expectedInputs))
    errors.push(`${dir}: input hash inventory incomplete or unexpected`);
  for (const path of sourceInputPaths) {
    const retained = read(dir, `source-inputs/${path}`);
    let pinned;
    try {
      pinned = execFileSync('git', ['show', `${expectedSha}:${path}`],
        { maxBuffer: 10 * 1024 * 1024 });
    } catch (error) {
      errors.push(`${dir}: pinned input ${path} unavailable: ${error.message}`);
    }
    if (pinned && !pinned.equals(retained))
      errors.push(`${dir}: retained ${path} differs from pinned product source`);
    if (recordedInputs.get(path) !== sha(retained))
      errors.push(`${dir}: recorded ${path} hash differs from retained bytes`);
  }
  const sdk = parse(dir, 'source-inputs/packages/zfb/package.json');
  const runtime = parse(dir, 'source-inputs/packages/zfb-runtime/package.json');
  if (sdk.version !== runtime.version ||
      (expectedSha === args['--candidate'] && sdk.version !== args['--candidate-version']))
    errors.push(`${dir}: SDK/runtime version differs from intended source version`);
  const toolchain = parse(dir, 'toolchain.json');
  for (const [key, value] of Object.entries(expectedToolchain)) {
    if (toolchain[key] !== value) errors.push(`${dir}: ${key} was ${toolchain[key]}, expected ${value}`);
  }
  if (line(dir, 'pnpm-version.txt') !== toolchain.pnpm ||
      line(dir, 'esbuild-version.txt') !== toolchain.esbuild ||
      !line(dir, 'rust.txt').includes(`rustc ${toolchain.rust} `))
    errors.push(`${dir}: toolchain capture disagrees with toolchain.json`);
  const log = line(dir, 'test.log');
  if (!/test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured;/.test(log) ||
      /skipping\.|test result: ok\. 0 passed;/.test(log))
    errors.push(`${dir}: exact production test did not demonstrate one executed pass`);
  const inventory = parse(dir, 'inventory.json');
  const dist = join(dir, 'dist');
  const actualFiles = walk(dist).map(path => {
    const bytes = readFileSync(path);
    return { path: relative(dist, path).replaceAll('\\', '/'), raw: bytes.length,
      gzip9: gzipSync(bytes, { level: 9, mtime: 0 }).length, sha256: sha(bytes) };
  });
  if (JSON.stringify(actualFiles) !== JSON.stringify(inventory.files))
    errors.push(`${dir}: original dist files differ from reported inventory`);
  const js = actualFiles.filter(file => /\.(?:js|mjs|cjs)$/.test(file.path));
  const entryChunks = js.filter(file => /^assets\/islands(?:-[^/]+)?\.js$/.test(file.path) &&
    !/^assets\/islands-chunk-/.test(file.path));
  const sharedChunks = js.filter(file => /^assets\/islands-chunk-/.test(file.path));
  const totals = { raw: js.reduce((n, file) => n + file.raw, 0),
    gzip9: js.reduce((n, file) => n + file.gzip9, 0) };
  if (!entryChunks.length || JSON.stringify(entryChunks) !== JSON.stringify(inventory.entryChunks) ||
      JSON.stringify(sharedChunks) !== JSON.stringify(inventory.sharedChunks) ||
      JSON.stringify(totals) !== JSON.stringify(inventory.allJavaScript))
    errors.push(`${dir}: router entries/shared chunks or all-JavaScript totals differ`);
  const html = actualFiles.filter(file => file.path.endsWith('.html'))
    .map(file => readFileSync(join(dist, file.path), 'utf8')).join('\n');
  if (!html || entryChunks.some(file => !html.includes('/' + file.path)))
    errors.push(`${dir}: HTML does not reference each router entry`);
  return { sourceSha, packageVersion: sdk.version, harnessHashes, patchHash, probeIdentity,
    toolchain, runnerObservedBinaryHashes: Object.fromEntries(observedBinaryPaths.map(path =>
      [path, recordedInputs.get(path) ?? null])), files: actualFiles,
    entryChunks, sharedChunks, allJavaScript: totals };
}

const before = examine(resolve(args['--before']), args['--baseline']);
const after = examine(resolve(args['--after']), args['--candidate']);
if (before.harnessHashes !== after.harnessHashes) errors.push('baseline/final harness hashes differ');
if (JSON.stringify(before.toolchain) !== JSON.stringify(after.toolchain))
  errors.push('baseline/final toolchains differ');
const beforeByPath = new Map(before.files.map(file => [file.path, file]));
const afterByPath = new Map(after.files.map(file => [file.path, file]));
const fileDiffs = [...new Set([...beforeByPath.keys(), ...afterByPath.keys()])].sort().map(path => {
  const oldFile = beforeByPath.get(path);
  const newFile = afterByPath.get(path);
  return { path, before: oldFile ?? null, after: newFile ?? null,
    rawDelta: (newFile?.raw ?? 0) - (oldFile?.raw ?? 0),
    gzip9Delta: (newFile?.gzip9 ?? 0) - (oldFile?.gzip9 ?? 0),
    sha256Changed: oldFile?.sha256 !== newFile?.sha256 };
});
const report = { status: errors.length ? 'invalid' : 'compared', errors,
  baseline: { sourceSha: before.sourceSha, probeIdentity: before.probeIdentity,
    packageVersion: before.packageVersion,
    runnerObservedBinaryHashes: before.runnerObservedBinaryHashes,
    entryChunks: before.entryChunks, sharedChunks: before.sharedChunks,
    allJavaScript: before.allJavaScript },
  candidate: { sourceSha: after.sourceSha, probeIdentity: after.probeIdentity,
    packageVersion: after.packageVersion,
    runnerObservedBinaryHashes: after.runnerObservedBinaryHashes,
    entryChunks: after.entryChunks, sharedChunks: after.sharedChunks,
    allJavaScript: after.allJavaScript },
  allJavaScriptDelta: { raw: after.allJavaScript.raw - before.allJavaScript.raw,
    gzip9: after.allJavaScript.gzip9 - before.allJavaScript.gzip9 },
  fileDiffs, ceiling: null,
  provenanceLimit: 'Native zfb and staged esbuild binary hashes are observed by each runner; binary bytes are not retained for independent comparator rehash.' };
const output = resolve(args['--out']);
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, JSON.stringify(report, null, 2) + '\n');
if (errors.length) throw new Error(`router comparison invalid: ${errors.join('; ')}`);
console.log(`Production router comparison retained at ${output}; all-JS raw delta ${report.allJavaScriptDelta.raw}, gzip9 delta ${report.allJavaScriptDelta.gzip9}; no router ceiling`);
