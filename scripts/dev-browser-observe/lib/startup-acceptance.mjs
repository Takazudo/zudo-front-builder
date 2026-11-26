import { readdirSync } from "node:fs";
import { LINUX_RSS_SAMPLER_VERSION, RENDERER_MEMORY_LIMITS } from "./renderer-memory-protocol.mjs";

export const STARTUP_ACCEPTANCE = "dev-browser-observe.renderer-memory.startup-acceptance.v1";
export const TINY_ACCEPTANCE = "dev-browser-observe.renderer-memory.tiny-acceptance.v1";
export const TINY_HELPER_CAP = 160;
export const STARTUP_WALL_MS = 180_000;
const REQUEST_MS = 10_000;
const REQUEST_BYTES = 128 * 1024;

export function startupProfileErrors(profile) {
  const tiny = profile.tinyAcceptance;
  if (!profile.startupAcceptance && !tiny) return [];
  const errors = [];
  const check = (condition, why) => {
    if (!condition) errors.push((tiny ? "$.tinyAcceptance: " : "$.startupAcceptance: ") + why);
  };
  if (tiny) {
    check(
      !profile.startupAcceptance && tiny.protocol === TINY_ACCEPTANCE,
      "requires its own tiny protocol",
    );
    check(["A", "B"].includes(tiny.probe), "unknown probe");
    if (tiny.probe === "B") {
      const [scenario] = profile.scenarios ?? [];
      const op = scenario?.ops?.[0];
      check(
        profile.scenarios?.length === 1 &&
          (scenario.set ?? "workload") === "workload" &&
          (scenario.warmOrder ?? "none") === "none" &&
          scenario.ops?.length === 1 &&
          op?.op === "edit-body" &&
          op.article === "a" &&
          op.writeMethod === "in-place" &&
          op.repeat?.warmups === 0 &&
          op.repeat?.measured === 12 &&
          (op.expect ?? "success") === "success" &&
          Object.keys(op).every((key) =>
            ["id", "op", "article", "writeMethod", "repeat", "expect"].includes(key),
          ),
        "requires exactly 12 serial default article-a body edits in a single scenario",
      );
      check(profile.restoration?.writeMethod === "in-place", "requires in-place restoration");
      check(
        profile.browser?.headless === true &&
          tiny.browserExecutable?.startsWith("/") &&
          /^[0-9a-f]{64}$/.test(tiny.browserExecutableSha256),
        "requires a pinned headless browser",
      );
    } else {
      check(!tiny.browserExecutable && !tiny.browserExecutableSha256, "probe A uses no browser");
    }
  } else
    check(profile.startupAcceptance.protocol === STARTUP_ACCEPTANCE, "unknown startup protocol");
  check(
    profile.mode === "source-built" &&
      (profile.fixture?.kind ?? "tiny") === "tiny" &&
      profile.fixture?.placement === "internal" &&
      profile.fixture?.adapter === "A0" &&
      !profile.fixture.trailingSlash &&
      (profile.fixture.configFormat ?? "json") === "json",
    "requires the internal two-article A0 fixture with its JSON config",
  );
  check(
    profile.diagnostic === true &&
      !profile.rendererMemoryStudy &&
      !profile.resourceStudy &&
      !profile.calibration &&
      (tiny?.probe === "B" || profile.scenarios?.length === 0),
    "requires a diagnostic profile with no other workloads",
  );
  check(
    profile.server?.host === "127.0.0.1" &&
      profile.server?.port?.mode === "ephemeral" &&
      !profile.server.extraArgs?.length &&
      profile.server.env?.ZFB_DEV_PROVENANCE_TRACE === "1" &&
      Object.keys(profile.server.env).length === 1,
    "requires ephemeral loopback with provenance tracing only",
  );
  check(
    profile.server?.readyTimeoutMs === RENDERER_MEMORY_LIMITS.readyTimeoutMs,
    "requires 90 s readiness",
  );
  check(
    profile.limits?.wallCeilingMs === STARTUP_WALL_MS &&
      profile.limits?.rssCeilingKiB === RENDERER_MEMORY_LIMITS.limits.rssCeilingKiB &&
      profile.limits?.sampleEveryMs === RENDERER_MEMORY_LIMITS.limits.sampleEveryMs,
    "requires 180 s total, 4 GiB sampled owned RSS and 1 Hz sampling",
  );
  return errors;
}

export function checkStartupArticles({ contentDir, manifest }) {
  const entries = readdirSync(contentDir, { withFileTypes: true });
  return (
    manifest.name === "tiny-two-article" &&
    manifest.articles.join(",") === "a,b" &&
    entries.every((entry) => entry.isFile()) &&
    entries
      .map((entry) => entry.name)
      .sort()
      .join(",") === "a.md,b.md"
  );
}

// Exactly one request; verify an actual body marker, bound the input while streaming,
// and include both headers and body consumption in the enclosing work deadline.
async function requestArticle(baseUrl, signal, timeoutMs, fetchImpl) {
  const url = new URL("/articles/a", baseUrl);
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port)
    throw new Error("startup acceptance requires the owned loopback readiness URL");
  const startedHrtimeNs = String(process.hrtime.bigint());
  const response = await fetchImpl(url, {
    redirect: "error",
    headers: { connection: "close" },
    signal: AbortSignal.any([signal, AbortSignal.timeout(timeoutMs)]),
  });
  if (response.status !== 200 || !response.body) {
    await response.body?.cancel();
    throw new Error("startup live request returned HTTP " + response.status);
  }
  const chunks = [];
  let bytes = 0;
  for await (const chunk of response.body) {
    bytes += chunk.byteLength;
    if (bytes > REQUEST_BYTES) throw new Error("startup live response exceeded its byte cap");
    chunks.push(chunk);
  }
  if (!Buffer.concat(chunks).includes("Body marker body-a-v0 for article a."))
    throw new Error("startup live response lacked the original article body marker");
  return {
    url: String(url),
    status: response.status,
    bytes,
    markerMatched: true,
    startedHrtimeNs,
    endedHrtimeNs: String(process.hrtime.bigint()),
  };
}

export async function verifyStartupSample({ server, ready, firstSample, records, study }) {
  await firstSample;
  study.signal.throwIfAborted();
  const sample = records.find((r) => r.type === "renderer-memory-sample");
  const result = sample?.result;
  const root = server.handle.native;
  const observed = result?.processes.find((p) => p.pid === root.pid);
  if (
    !sample ||
    BigInt(sample.startedHrtimeNs) > ready.readyNs ||
    result.samplerVersion !== LINUX_RSS_SAMPLER_VERSION ||
    !/^\d+$/.test(result.helper?.spawnHrtimeNs) ||
    BigInt(result.helper.spawnHrtimeNs) < BigInt(sample.startedHrtimeNs) ||
    BigInt(result.helper.spawnHrtimeNs) > ready.readyNs ||
    result.helper?.closed !== true ||
    result.helper?.pipesClosed !== true ||
    observed?.disposition !== "live" ||
    observed.startTicks !== root.startTicks ||
    observed.bootId !== root.bootId ||
    observed.pgid !== root.pgid ||
    !Number.isSafeInteger(observed.rssKiB) ||
    observed.rssKiB <= 0 ||
    !Number.isSafeInteger(result.summedRssKiB) ||
    result.summedRssKiB < observed.rssKiB
  )
    throw new Error("missing a bounded live root sample initiated before readiness");
  return {
    readyHrtimeNs: String(ready.readyNs),
    firstSampleSeq: sample.seq,
    firstSampleStartedHrtimeNs: sample.startedHrtimeNs,
    firstHelperSpawnHrtimeNs: result.helper.spawnHrtimeNs,
  };
}

export async function verifyStartup({
  server,
  ready,
  firstSample,
  records,
  study,
  fetchImpl = fetch,
}) {
  const anchor = await verifyStartupSample({ server, ready, firstSample, records, study });
  const live = await study.runStage(
    "startup live request",
    REQUEST_MS,
    process.hrtime.bigint(),
    (timeoutMs) => requestArticle(ready.baseUrl, study.signal, timeoutMs, fetchImpl),
  );
  return { ...anchor, live };
}
