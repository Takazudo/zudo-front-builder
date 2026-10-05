#!/usr/bin/env node
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { acquire, canonicalBootstrapCandidate, fromRoot, identity, makePlan, paths, probe, readJson, requireChannel, resolveCatalog, validateMetadata, validatePlan } from './reference.mjs';
import { assessUpstream } from './upstream.mjs';

function options(argv) {
  const result = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i]?.startsWith('--') || argv[i + 1] === undefined || argv[i + 1].startsWith('--')) throw Error(`Expected --key value, got ${argv[i]}`);
    result[argv[i].slice(2)] = argv[i + 1];
  }
  return result;
}
export async function metadata(version, opts, bootstrap, fetcher = fetch) {
  requireChannel(version, opts.channel ?? 'stable');
  if (version === bootstrap.version && !opts.live && !opts.catalog) return canonicalBootstrapCandidate(bootstrap, {
    package: bootstrap.package, version, integrity: bootstrap.integrity, tarball: bootstrap.tarball, sha1: bootstrap.sha1,
    source: bootstrap.source,
  });
  let resolved;
  if (opts.catalog) resolved = resolveCatalog(await readJson(resolve(opts.catalog)), version, opts.channel ?? 'stable');
  else {
    const response = await fetcher(`https://registry.npmjs.org/tailwindcss/${encodeURIComponent(version)}`);
    if (!response.ok) throw Error(`Registry metadata HTTP ${response.status}`);
    resolved = validateMetadata(await response.json(), version);
  }
  return version === bootstrap.version ? canonicalBootstrapCandidate(bootstrap, resolved) : resolved;
}
async function pinSource(candidate, bootstrap) {
  if (candidate.version === bootstrap.version) return candidate;
  const tag = `v${candidate.version}`;
  const response = await fetch(`https://api.github.com/repos/tailwindlabs/tailwindcss/git/ref/tags/${encodeURIComponent(tag)}`,
    { headers: { accept: 'application/vnd.github+json' } });
  if (!response.ok) throw Error(`Git tag HTTP ${response.status}`);
  const ref = await response.json();
  if (ref.ref !== `refs/tags/${tag}` || ref.object?.type !== 'commit' || !/^[0-9a-f]{40}$/.test(ref.object.sha)) throw Error('Unverifiable candidate Git tag');
  return { ...candidate, source: { ...candidate.source, tag, observedTagCommit: ref.object.sha,
    status: 'tag-observed-artifact-link-unverified' } };
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
    const candidate = await pinSource(await metadata(version, opts, bootstrap), bootstrap);
    const state = { profile, accepted: await readJson(fromRoot(paths.accepted)), reviewed: await readJson(fromRoot(paths.reviewed)) };
    const hashes = await identity();
    const plan = makePlan({ candidate, state, hashes, channel: opts.channel ?? 'stable',
      toolchain: { node: process.version, lockfile: paths.lock, lockfileSha256: hashes.lock } });
    process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`);
  } else if (command === 'assess') {
    if (!opts.plan || !opts.cache) throw Error('assess requires --plan and --cache outside checkout');
    const state = { profile, accepted: await readJson(fromRoot(paths.accepted)), reviewed: await readJson(fromRoot(paths.reviewed)) };
    const inputPlan = await readJson(resolve(opts.plan)), hashes = await identity();
    const expectedCandidate = await pinSource(await metadata(inputPlan.candidate?.version,
      { channel: inputPlan.channel, live: inputPlan.candidate?.version === bootstrap.version ? undefined : 'yes' }, bootstrap), bootstrap);
    const plan = validatePlan(inputPlan, hashes, state, expectedCandidate,
      { node: process.version, lockfile: paths.lock, lockfileSha256: hashes.lock });
    process.stdout.write(`${JSON.stringify(await assessUpstream(plan, opts.cache), null, 2)}\n`);
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
    throw Error('Usage: reference-cli.mjs plan [--candidate exact] [--channel stable|prerelease] [--catalog complete.json] [--live yes] | assess --plan plan.json --cache /outside/checkout | acquire --candidate exact --cache /outside/checkout [--catalog complete.json] [--live yes] | probe --cache /outside/checkout --candidates block,hidden');
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
