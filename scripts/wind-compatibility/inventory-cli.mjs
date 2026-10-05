#!/usr/bin/env node
import { readFile, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { archiveSources, extractSurface, makeInventory, validateInventory, loadJson, digest, verifyRuntime } from './inventory.mjs';

const root = new URL('../../', import.meta.url);
const at = (path) => new URL(path, root);
const command = process.argv[2];
const archive = process.argv[3];
const sourcePath = at('tests/wind-compatibility/upstream-surface.v1.json');
const inventoryPath = at('tests/wind-compatibility/inventory.v1.json');
const runtime = await loadJson(at('tests/wind-compatibility/runtime-keysets.v1.json'));
const catalog = await loadJson(at('crates/zudo-wind/catalog/zudo-wind-catalog.v1.json'));
const profile = await loadJson(at('tests/wind-compatibility/profile.json'));
const variantSource = await readFile(at('crates/zudo-wind/src/variant.rs'), 'utf8');
const docsEvidence = await Promise.all(['docs/wind-examples/guide-coming-from-tailwind.json', 'docs/wind-examples/guide-utility-grammar.json'].map(async (path) => ({ path, sha256: digest(await readFile(at(path))) })));
if (command === 'extract') {
  if (!archive) throw Error('Usage: inventory-cli.mjs extract /path/to/pinned-source-archive');
  if (!process.argv[4] || !process.argv[5]) throw Error('extract requires npm tarball and verified extracted dist/lib.mjs paths');
  await verifyRuntime(runtime, process.argv[4], process.argv[5]);
  const upstream = extractSurface(archiveSources(await readFile(archive)));
  const windSha = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root }).toString().trim();
  const inventory = makeInventory(upstream, catalog, profile, windSha, variantSource, docsEvidence, runtime);
  validateInventory(inventory, upstream, catalog, profile, variantSource, docsEvidence, runtime);
  await writeFile(sourcePath, JSON.stringify(upstream, null, 2) + '\n');
  await writeFile(inventoryPath, JSON.stringify(inventory, null, 2) + '\n');
  process.stdout.write(`Extracted ${inventory.rows.length} registration rows and ${inventory.profileCases.length} profile cases\n`);
} else if (command === 'validate') {
  const upstream = await loadJson(sourcePath), inventory = await loadJson(inventoryPath);
  if (process.argv[4] && process.argv[5]) await verifyRuntime(runtime, process.argv[4], process.argv[5]);
  if (archive) {
    const extracted = extractSurface(archiveSources(await readFile(archive)));
    if (JSON.stringify(extracted) !== JSON.stringify(upstream)) throw Error('Pinned upstream extraction drift');
  }
  process.stdout.write(JSON.stringify(validateInventory(inventory, upstream, catalog, profile, variantSource, docsEvidence, runtime)) + '\n');
} else throw Error('Usage: inventory-cli.mjs extract|validate [pinned-source-archive]');
