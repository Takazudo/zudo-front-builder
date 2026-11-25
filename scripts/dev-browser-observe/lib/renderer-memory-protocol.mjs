export const RENDERER_MEMORY_PROTOCOL = "dev-browser-observe.renderer-memory.v1";
export const RENDERER_MEMORY_FEATURE = "renderer-memory-observe";

export const RENDERER_MEMORY_LIMITS = {
  edits: 12,
  idleMs: 60_000,
  liveChecks: 6,
  readyTimeoutMs: 90_000,
  timeouts: {
    attemptMs: 60_000,
    supplementalMs: 60_000,
    restoreMs: 120_000,
    quietMs: 3_000,
    stopTermMs: 5_000,
    stopKillMs: 3_000,
  },
  limits: { rssCeilingKiB: 4_194_304, wallCeilingMs: 300_000, sampleEveryMs: 1_000 },
};

export function rendererMemoryBuild(receipt) {
  const features = receipt?.build?.features;
  return (
    Array.isArray(features) &&
    features.some(
      (feature) =>
        feature === RENDERER_MEMORY_FEATURE || feature === `zfb/${RENDERER_MEMORY_FEATURE}`,
    )
  );
}

export function rendererMemoryProfileErrors(profile) {
  if (!profile.rendererMemoryStudy) return [];
  const require = (condition, message) => {
    if (!condition) errors.push(`$.rendererMemoryStudy: ${message}`);
  };
  const errors = [];
  const limits = RENDERER_MEMORY_LIMITS;
  require(profile.diagnostic === true, "requires diagnostic: true");
  require(profile.mode === "source-built" &&
    profile.fixture?.kind === "field-notes" &&
    profile.fixture?.adapter === "direct-read" &&
    profile.fixture?.scale ===
      100, "requires 100-article source-built Field Notes with the direct-read adapter");
  require(!profile.resourceStudy, "cannot be combined with resourceStudy");
  require(profile.server?.env?.ZFB_DEV_PROVENANCE_TRACE ===
    "1", 'requires server.env.ZFB_DEV_PROVENANCE_TRACE: "1"');
  require(profile.browser?.headless === true, "requires a headless browser");
  require(profile.server?.port?.mode === "ephemeral" &&
    profile.server?.host === "127.0.0.1", "requires an ephemeral loopback server");
  require(profile.restoration?.writeMethod === "in-place", "requires in-place restoration");
  require(profile.server?.readyTimeoutMs ===
    limits.readyTimeoutMs, `readyTimeoutMs must be ${limits.readyTimeoutMs}`);
  for (const group of ["limits", "timeouts"])
    for (const [key, value] of Object.entries(limits[group]))
      require(profile[group]?.[key] === value, `${group}.${key} must be ${value}`);

  const scenarios = profile.scenarios ?? [];
  const scenario = scenarios[0];
  const op = scenario?.ops?.[0];
  require(scenarios.length === 1 &&
    (scenario?.set ?? "workload") === "workload" &&
    (scenario?.warmOrder ?? "none") === "none" &&
    scenario?.ops?.length === 1 &&
    op?.op === "edit-body" &&
    op?.article === "article-00005" &&
    op?.writeMethod === "in-place" &&
    !op.targetPath &&
    !op.inject &&
    (op.expect ?? "success") === "success" &&
    (op.repeat?.warmups ?? 0) + (op.repeat?.measured ?? 0) ===
      limits.edits, "requires one scenario with no warming and 12 consecutive in-place edit-body attempts on article-00005");
  return errors;
}
