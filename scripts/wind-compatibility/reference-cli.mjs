#!/usr/bin/env node
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  acquire,
  canonicalBootstrapCandidate,
  fromRoot,
  identity,
  makePlan,
  outsideCheckout,
  paths,
  probe,
  readJson,
  requireChannel,
  resolveCatalog,
  validateMetadata,
  validatePlan,
} from "./reference.mjs";
import { assessUpstream } from "./upstream.mjs";
import {
  compareOutputs,
  executeReferenceRun,
  requirePassingCurrent,
  validateRun,
} from "./reference-comparison.mjs";
import {
  assertStableTransition,
  recoverTransition,
  validateAssessment,
  validateComparison,
  transition,
  upstreamReviewTransition,
  writeTransitionAtomically,
} from "./reference-promotion.mjs";
import { testedInputIdentity } from "./reference-identity.mjs";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";

function options(argv) {
  const result = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i]?.startsWith("--") || argv[i + 1] === undefined || argv[i + 1].startsWith("--"))
      throw Error(`Expected --key value, got ${argv[i]}`);
    result[argv[i].slice(2)] = argv[i + 1];
  }
  return result;
}
export async function metadata(version, opts, bootstrap, fetcher = fetch) {
  requireChannel(version, opts.channel ?? "stable");
  if (version === bootstrap.version && !opts.live && !opts.catalog)
    return canonicalBootstrapCandidate(bootstrap, {
      package: bootstrap.package,
      version,
      integrity: bootstrap.integrity,
      tarball: bootstrap.tarball,
      sha1: bootstrap.sha1,
      source: bootstrap.source,
    });
  let resolved;
  if (opts.catalog)
    resolved = resolveCatalog(
      await readJson(resolve(opts.catalog)),
      version,
      opts.channel ?? "stable",
    );
  else {
    const response = await fetcher(
      `https://registry.npmjs.org/tailwindcss/${encodeURIComponent(version)}`,
    );
    if (!response.ok) throw Error(`Registry metadata HTTP ${response.status}`);
    resolved = validateMetadata(await response.json(), version);
  }
  return version === bootstrap.version
    ? canonicalBootstrapCandidate(bootstrap, resolved)
    : resolved;
}
async function pinSource(candidate, bootstrap) {
  if (candidate.version === bootstrap.version) return candidate;
  const tag = `v${candidate.version}`;
  const response = await fetch(
    `https://api.github.com/repos/tailwindlabs/tailwindcss/git/ref/tags/${encodeURIComponent(tag)}`,
    { headers: { accept: "application/vnd.github+json" } },
  );
  if (!response.ok) throw Error(`Git tag HTTP ${response.status}`);
  const ref = await response.json();
  if (
    ref.ref !== `refs/tags/${tag}` ||
    ref.object?.type !== "commit" ||
    !/^[0-9a-f]{40}$/.test(ref.object.sha)
  )
    throw Error("Unverifiable candidate Git tag");
  return {
    ...candidate,
    source: {
      ...candidate.source,
      tag,
      observedTagCommit: ref.object.sha,
      status: "tag-observed-artifact-link-unverified",
    },
  };
}
async function main() {
  const [command, ...args] = process.argv.slice(2),
    opts = options(args);
  if (command === "recover") {
    process.stdout.write(`${JSON.stringify(await recoverTransition())}\n`);
    return;
  }
  await assertStableTransition();
  const bootstrap = await readJson(fromRoot(paths.bootstrap));
  const profile = await readJson(fromRoot(paths.profile));
  await assertStableTransition();
  if (
    bootstrap.schemaVersion !== 1 ||
    bootstrap.package !== "tailwindcss" ||
    bootstrap.version !== profile.referencePolicy.initialCandidateVersion ||
    bootstrap.integrity !== profile.referencePolicy.planningIntegrityHint.value
  )
    throw Error("Bootstrap/profile identity mismatch");
  if (command === "plan") {
    const version = opts.candidate ?? bootstrap.version;
    const candidate = await pinSource(await metadata(version, opts, bootstrap), bootstrap);
    const state = {
      profile,
      accepted: await readJson(fromRoot(paths.accepted)),
      reviewed: await readJson(fromRoot(paths.reviewed)),
    };
    const hashes = await identity();
    const plan = makePlan({
      candidate,
      state,
      hashes,
      channel: opts.channel ?? "stable",
      toolchain: { node: process.version, lockfile: paths.lock, lockfileSha256: hashes.lock },
    });
    process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`);
  } else if (command === "assess") {
    if (!opts.plan || !opts.cache)
      throw Error("assess requires --plan and --cache outside checkout");
    const state = {
      profile,
      accepted: await readJson(fromRoot(paths.accepted)),
      reviewed: await readJson(fromRoot(paths.reviewed)),
    };
    const inputPlan = await readJson(resolve(opts.plan)),
      hashes = await identity();
    const expectedCandidate = await pinSource(
      await metadata(
        inputPlan.candidate?.version,
        {
          channel: inputPlan.channel,
          live: inputPlan.candidate?.version === bootstrap.version ? undefined : "yes",
        },
        bootstrap,
      ),
      bootstrap,
    );
    const plan = validatePlan(inputPlan, hashes, state, expectedCandidate, {
      node: process.version,
      lockfile: paths.lock,
      lockfileSha256: hashes.lock,
    });
    process.stdout.write(`${JSON.stringify(await assessUpstream(plan, opts.cache), null, 2)}\n`);
  } else if (["compare", "promote", "review", "verify-current"].includes(command)) {
    const state = {
      profile,
      accepted: await readJson(fromRoot(paths.accepted)),
      reviewed: await readJson(fromRoot(paths.reviewed)),
    };
    if (command === "verify-current") {
      if (!state.accepted.acceptedReference) throw Error("No accepted reference to verify");
      if (!opts.output || !opts.cache || !opts["wind-binary"] || !opts["wind-build-manifest"])
        throw Error("verify-current requires output, cache, Wind binary and build manifest");
      const output = await outsideCheckout(resolve(opts.output));
      await mkdir(output, { recursive: true });
      const referencePath = resolve(output, "accepted-reference.json");
      await writeFile(referencePath, JSON.stringify(state.accepted.acceptedReference) + "\n", {
        flag: "wx",
      });
      await executeReferenceRun({
        output,
        referenceFile: referencePath,
        binary: opts["wind-binary"],
        buildManifest: opts["wind-build-manifest"],
        cache: opts.cache,
      });
      const manifest = await readJson(fromRoot("tests/wind-compatibility/corpus/manifest.json"));
      requirePassingCurrent(
        await validateRun(
          output,
          state.accepted.acceptedReference,
          await identity(),
          profile,
          manifest,
          { cache: opts.cache, strictArtifacts: true },
        ),
      );
      process.stdout.write(`${output}\n`);
    } else {
      if (!opts.plan || !opts.assessment || !opts.cache || (command !== "review" && !opts.output))
        throw Error(
          `${command} requires plan, assessment, cache${command === "review" ? "" : " and output"}`,
        );
      const inputPlan = await readJson(resolve(opts.plan));
      const hashes = await identity();
      const plan = validatePlan(inputPlan, hashes, state, inputPlan.candidate, {
        node: process.version,
        lockfile: paths.lock,
        lockfileSha256: hashes.lock,
      });
      const assessment = await readJson(resolve(opts.assessment));
      const artifactSha256 = await validateAssessment(assessment, plan, opts.cache);
      const candidate = { ...plan.candidate, artifactSha256 };
      const accepted = state.accepted.acceptedReference;
      if (command === "review") {
        if (!opts.classification) throw Error("review requires --classification");
        const classification = await readJson(resolve(opts.classification));
        const finalSha = execFileSync("git", ["rev-parse", "HEAD"], {
          cwd: fromRoot("."),
          encoding: "utf8",
        }).trim();
        const next = upstreamReviewTransition({
          plan,
          assessment,
          classification,
          state,
          finalSha,
          testedInputs: await testedInputIdentity(),
        });
        if (opts.apply === "yes") await writeTransitionAtomically(next, state);
        else if (opts.apply !== undefined) throw Error("--apply must be yes when supplied");
        process.stdout.write(
          `${JSON.stringify({ dryRun: opts.apply !== "yes", next }, null, 2)}\n`,
        );
      } else {
        const output = await outsideCheckout(
          resolve(command === "promote" ? (opts["artifact-root"] ?? opts.output) : opts.output),
        );
        if (command === "compare") {
          if (!opts["wind-binary"] || !opts["wind-build-manifest"])
            throw Error("compare requires Wind binary and build manifest");
          await mkdir(output, { recursive: true });
          const candidatePath = resolve(output, "candidate-reference.json");
          await writeFile(candidatePath, JSON.stringify(candidate) + "\n", { flag: "wx" });
          await executeReferenceRun({
            output: resolve(output, "candidate"),
            referenceFile: candidatePath,
            binary: opts["wind-binary"],
            buildManifest: opts["wind-build-manifest"],
            cache: opts.cache,
            assessmentMode: true,
          });
          if (accepted) {
            const acceptedPath = resolve(output, "accepted-reference.json");
            await writeFile(acceptedPath, JSON.stringify(accepted) + "\n", { flag: "wx" });
            await executeReferenceRun({
              output: resolve(output, "accepted"),
              referenceFile: acceptedPath,
              binary: opts["wind-binary"],
              buildManifest: opts["wind-build-manifest"],
              cache: opts.cache,
            });
          }
          const comparison = await compareOutputs({
            plan,
            assessment,
            output,
            candidate,
            accepted,
            input: hashes,
            cache: opts.cache,
          });
          await writeFile(
            resolve(output, "comparison.json"),
            JSON.stringify(comparison, null, 2) + "\n",
            { flag: "wx" },
          );
          process.stdout.write(`${resolve(output, "comparison.json")}\n`);
        } else {
          if (!opts.classification) throw Error("promote requires --classification");
          const comparison = await readJson(resolve(output, "comparison.json"));
          await validateComparison({
            comparison,
            plan,
            assessment,
            output,
            candidate,
            accepted,
            input: hashes,
            cache: opts.cache,
            strictArtifacts:
              comparison.candidate.passing === true &&
              (!comparison.accepted || comparison.accepted.passing === true),
          });
          const classification = await readJson(resolve(opts.classification));
          const shipping = opts.shipping ? await readJson(resolve(opts.shipping)) : null;
          const finalSha = execFileSync("git", ["rev-parse", "HEAD"], {
            cwd: fromRoot("."),
            encoding: "utf8",
          }).trim();
          const next = await transition({
            plan,
            assessment,
            comparison,
            classification,
            shipping,
            shippingOutput: opts["shipping-output"],
            state,
            finalSha,
          });
          if (opts.apply === "yes") await writeTransitionAtomically(next, state);
          else if (opts.apply !== undefined) throw Error("--apply must be yes when supplied");
          process.stdout.write(
            `${JSON.stringify(
              { dryRun: opts.apply !== "yes", next, testedInputs: await testedInputIdentity() },
              null,
              2,
            )}\n`,
          );
        }
      }
    }
  } else if (command === "acquire") {
    if (!opts.cache) throw Error("acquire requires --cache outside checkout");
    const version = opts.candidate ?? bootstrap.version;
    const candidate = await metadata(version, opts, bootstrap);
    process.stdout.write(`${JSON.stringify(await acquire(candidate, opts.cache), null, 2)}\n`);
  } else if (command === "probe") {
    if (!opts.cache || !opts.candidates)
      throw Error("probe requires --cache outside checkout and --candidates comma,separated");
    const version = opts.candidate ?? bootstrap.version;
    const candidate = await metadata(version, opts, bootstrap);
    const acquisition = await acquire(candidate, opts.cache);
    process.stdout.write(
      `${JSON.stringify(await probe(acquisition, opts.candidates.split(",")), null, 2)}\n`,
    );
  } else {
    throw Error(
      "Usage: reference-cli.mjs plan [--candidate exact] [--channel stable|prerelease] [--catalog complete.json] [--live yes] | assess --plan plan.json --cache /outside/checkout | compare --plan plan.json --assessment assessment.json --cache /outside/checkout --wind-binary path --wind-build-manifest path --output /outside/checkout | promote --plan plan.json --assessment assessment.json --cache /outside/checkout --output original --classification reviewed.json [--artifact-root restored] [--shipping report.json --shipping-output root] [--apply yes] | verify-current --cache /outside/checkout --wind-binary path --wind-build-manifest path --output /outside/checkout | recover",
    );
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
