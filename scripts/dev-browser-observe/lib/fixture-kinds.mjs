import { existsSync } from "node:fs";
import { join } from "node:path";
import {
  FN_SEL,
  fieldNotesOps,
  fnAdapterPath,
  loadFieldNotesFixture,
  materializeFieldNotes,
  overlayChecks,
  sourceTreeHashes,
} from "./fieldnotes.mjs";
import {
  adapterPath,
  blockWrite,
  hashOf,
  loadFixture,
  materialize,
  missingSibling,
  originalConfigApplies,
  originalLiveChecks,
  performWrite,
  performWrites,
  prepareAttempt,
  restoreSnapshot,
  snapshotSources,
  unblockWrite,
  warmPaths,
} from "./fixture.mjs";
import { sha256File } from "./util.mjs";

export function fixtureKind(profile) {
  return profile.fixture.kind ?? "tiny";
}

const injectionFsHelpers = {
  hashOf,
  missingSibling,
  blockWrite,
  unblockWrite,
  performWrite,
  performWrites,
};

const tiny = {
  loadManifest: (dir) => loadFixture(dir),
  appDirs: (dir, manifest) => [join(dir, manifest.app)],
  adapterPath: (dir, manifest, id) => adapterPath(dir, manifest, id),
  adapterVariant: (manifest, id) => manifest.adapters.variants[id],
  extraChecks(profile, resolved, manifest) {
    const checks = [];
    if (
      originalConfigApplies(manifest, {
        configFormat: profile.fixture.configFormat ?? "json",
        placement: profile.fixture.placement,
        trailingSlash: profile.fixture.trailingSlash === true,
      })
    ) {
      const actual = sha256File(join(resolved.fixtureDir, manifest.configOriginal.file));
      checks.push({
        name: "config-provenance:original",
        ok: actual === manifest.configOriginal.sha256,
        expected: manifest.configOriginal.sha256,
        actual,
        provenance: manifest.configOriginal.provenance,
      });
    }
    return checks;
  },
  materialize: ({ profile, resolved, workDir }) =>
    materialize({
      fixtureDir: resolved.fixtureDir,
      workDir,
      placement: profile.fixture.placement,
      adapter: profile.fixture.adapter,
      trailingSlash: profile.fixture.trailingSlash === true,
      configFormat: profile.fixture.configFormat ?? "json",
    }),
  receiptFixture: (m, manifest, profile) => ({
    name: manifest.name,
    kind: "tiny",
    placement: profile.fixture.placement,
    adapter: profile.fixture.adapter,
    adapterLabel: manifest.adapters.variants[profile.fixture.adapter].label,
    adapterProvenance: manifest.adapters.variants[profile.fixture.adapter].provenance ?? null,
    adapterSourceSha256: manifest.adapters.variants[profile.fixture.adapter].sha256 ?? null,
    trailingSlash: m.trailingSlash,
    configFormat: m.configFormat,
    configFile: m.configFile,
    configProvenance: m.configProvenance,
    configOriginalSha256: m.configOriginalSha256,
    workDir: m.workDir,
    appDir: m.appDir,
    contentDir: m.contentDir,
    configSha256: m.configSha256,
    adapterSha256: m.adapterSha256,
    materializedTreeSha256: m.materializedTreeSha256,
    managedCss: manifest.css,
  }),
  ops: (m, profile) => ({
    prepareAttempt: (op) => prepareAttempt(m, op),
    snapshot: () => snapshotSources(m),
    restore: (s) => restoreSnapshot(s, { writeMethod: profile.restoration.writeMethod }),
    liveChecks: (s) => originalLiveChecks(m, s),
    warmPaths: (scenario) => warmPaths(m, scenario),
    route: m.route,
    ...injectionFsHelpers,
  }),
  calibrationPage: (m) => m.route("independent"),
};

const fieldNotes = {
  loadManifest: (dir) => loadFieldNotesFixture(dir),
  appDirs: (_dir, manifest, _profile, resolved) =>
    resolved.sourceDir ? [join(resolved.sourceDir, manifest.source.app)] : [],
  adapterPath: (dir, manifest, id) => fnAdapterPath(dir, manifest, id),
  adapterVariant: (manifest, id) => manifest.adapters.variants[id],
  extraChecks(profile, resolved, manifest) {
    const src = profile.fixture.source ?? {};
    const dir = resolved.sourceDir;
    const checks = [];
    if (!dir || !existsSync(dir)) {
      checks.push({
        name: "field-notes-source-dir",
        ok: false,
        expected: "existing dir",
        actual: dir ?? null,
      });
      return checks;
    }
    let actual;
    try {
      actual = sourceTreeHashes(dir, manifest);
    } catch (error) {
      checks.push({
        name: "field-notes-source-readable",
        ok: false,
        expected: "readable",
        actual: String(error?.message ?? error),
      });
      return checks;
    }
    for (const key of [
      "appsZfbTreeSha256",
      "contentTreeSha256",
      "sharedTreeSha256",
      "datasetSha256",
    ]) {
      const pinned = src[key] ?? null;
      checks.push({
        name: `source-pin:${key}`,
        ok: pinned !== null && pinned === actual[key],
        expected: pinned,
        actual: actual[key],
      });
    }
    checks.push({
      name: "source-dataset-matches-manifest-declaration",
      ok: actual.datasetSha256 === manifest.source.datasetSha256,
      expected: manifest.source.datasetSha256,
      actual: actual.datasetSha256,
    });
    const placement = profile.fixture.placement ?? "external";
    try {
      checks.push(...overlayChecks(resolved.fixtureDir, dir, manifest, placement));
    } catch (error) {
      checks.push({
        name: `placement:${placement}`,
        ok: false,
        expected: "declared placement",
        actual: String(error?.message ?? error),
      });
    }
    if (profile.fixture.managedCss) {
      const a = sha256File(join(resolved.fixtureDir, manifest.managedCss.file));
      checks.push({
        name: "managed-css-addition",
        ok: a === manifest.managedCss.sha256,
        expected: manifest.managedCss.sha256,
        actual: a,
      });
    }
    return checks;
  },
  materialize: ({ profile, resolved, workDir }) =>
    materializeFieldNotes({
      fixtureDir: resolved.fixtureDir,
      sourceDir: resolved.sourceDir,
      workDir,
      adapter: profile.fixture.adapter,
      managedCss: profile.fixture.managedCss === true,
      placement: profile.fixture.placement ?? "external",
    }),
  receiptFixture: (m, manifest, profile) => ({
    name: manifest.name,
    kind: "field-notes",
    adapter: profile.fixture.adapter,
    adapterLabel: manifest.adapters.variants[profile.fixture.adapter].label,
    adapterProvenance: manifest.adapters.variants[profile.fixture.adapter].provenance ?? null,
    adapterSourceSha256: manifest.adapters.variants[profile.fixture.adapter].sha256 ?? null,
    adapterFacts: manifest.adapters.variants[profile.fixture.adapter].facts ?? null,
    sourceDir: profile.fixture.source.dir,
    sourcePins: profile.fixture.source,
    managedCss:
      profile.fixture.managedCss === true
        ? { ...manifest.managedCss, declaredAddition: true }
        : { declaredAddition: false },
    publicCss: manifest.publicCss,
    trailingSlash: true,
    placement: m.placement,
    layout: m.layout,
    materializedDatasetSha256: m.materializedDatasetSha256,
    configFile: m.configFile,
    configSha256: m.configSha256,
    configProvenance: m.configProvenance,
    configBaseSha256: m.configBaseSha256,
    configOverlay: m.configOverlay,
    workDir: m.workDir,
    appDir: m.appDir,
    contentDir: m.contentDir,
    adapterSha256: m.adapterSha256,
    selectors: Object.fromEntries(
      Object.entries(FN_SEL).map(([k, v]) => [k, typeof v === "function" ? v("{slug}") : v]),
    ),
  }),
  ops: (m, profile) => ({
    ...injectionFsHelpers,
    ...fieldNotesOps(m, { restorationWriteMethod: profile.restoration.writeMethod }),
  }),
  calibrationPage: () => "/archive/1/",
};

export const FIXTURE_KINDS = { tiny, "field-notes": fieldNotes };

export function kindFor(profile) {
  const k = FIXTURE_KINDS[fixtureKind(profile)];
  if (!k) throw new Error(`unknown fixture kind ${fixtureKind(profile)}`);
  return k;
}
