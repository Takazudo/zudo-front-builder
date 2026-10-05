#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { acquire, assessBundle, fromRoot, identity, makePlan, paths, probe, readJson, resolveCatalog, validateMetadata, validatePlan } from './reference.mjs';

function options(argv) {
  const result = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i]?.startsWith('--') || argv[i + 1] === undefined || argv[i + 1].startsWith('--')) throw Error(`Expected --key value, got ${argv[i]}`);
    result[argv[i].slice(2)] = argv[i + 1];
  }
  return result;
}
async function metadata(version, opts, bootstrap) {
  if (opts.catalog) return resolveCatalog(await readJson(resolve(opts.catalog)), version, opts.channel ?? 'stable');
  if (version === bootstrap.version && !opts.live) return {
    package: bootstrap.package, version, integrity: bootstrap.integrity, tarball: bootstrap.tarball, sha1: bootstrap.sha1, source: bootstrap.source,
  };
  const response = await fetch(`https://registry.npmjs.org/tailwindcss/${encodeURIComponent(version)}`);
  if (!response.ok) throw Error(`Registry metadata HTTP ${response.status}`);
  return validateMetadata(await response.json(), version);
}
async function main() {
  const [command, ...args] = process.argv.slice(2), opts = options(args);
  const bootstrap = await readJson(fromRoot(paths.bootstrap));
  const profile = await readJson(fromRoot(paths.profile));
  if (bootstrap.schemaVersion !== 1 || bootstrap.package !== 'tailwindcss' ||
      bootstrap.version !== profile.referencePolicy.initialCandidateVersion ||
      bootstrap.integrity !== profile.referencePolicy.planningIntegrityHint.value) throw Error('Bootstrap/profile identity mismatch');
  if (command === 'plan') {
    const version = opts.candidate ?? bootstrap.version;
    const candidate = await metadata(version, opts, bootstrap);
    const state = { profile, accepted: await readJson(fromRoot(paths.accepted)), reviewed: await readJson(fromRoot(paths.reviewed)) };
    const plan = makePlan({ candidate, state, hashes: await identity(), channel: opts.channel ?? 'stable',
      toolchain: { node: process.version, lockfile: paths.lock, lockfileSha256: (await identity()).lock } });
    process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`);
  } else if (command === 'assess') {
    if (!opts.plan || !opts.bundle) throw Error('assess requires --plan and --bundle');
    const plan = validatePlan(await readJson(resolve(opts.plan)), await identity());
    process.stdout.write(`${JSON.stringify(assessBundle(await readJson(resolve(opts.bundle)), plan), null, 2)}\n`);
  } else if (command === 'acquire') {
    if (!opts.cache) throw Error('acquire requires --cache outside checkout');
    const version = opts.candidate ?? bootstrap.version;
    const candidate = await metadata(version, opts, bootstrap);
    process.stdout.write(`${JSON.stringify(await acquire(candidate, opts.cache), null, 2)}\n`);
  } else if (command === 'probe') {
    if (!opts.cache || !opts.candidates) throw Error('probe requires --cache outside checkout and --candidates comma,separated');
    const version = opts.candidate ?? bootstrap.version;
    const candidate = await metadata(version, opts, bootstrap);
    const acquisition = await acquire(candidate, opts.cache);
    process.stdout.write(`${JSON.stringify(await probe(acquisition, opts.candidates.split(',')), null, 2)}\n`);
  } else {
    throw Error('Usage: reference-cli.mjs plan [--candidate exact] [--channel stable|prerelease] [--catalog complete.json] [--live yes] | assess --plan plan.json --bundle bundle.json | acquire --candidate exact --cache /outside/checkout [--catalog complete.json] [--live yes] | probe --cache /outside/checkout --candidates block,hidden');
  }
}
main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
