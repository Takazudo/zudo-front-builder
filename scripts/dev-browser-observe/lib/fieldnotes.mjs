import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import {
  addArticle,
  breakFrontmatter,
  computeIndex,
  createModel,
  generatedMetadata,
  getFrontmatter,
  href,
  labelOf,
  newArticleText,
  placement,
  referrersOf,
  relatedHrefs,
  removeArticle,
  renameSlug,
  setCategory,
  setField,
  setFrontmatter,
  setTitle,
  tokenInFirstParagraph,
} from "./fieldnotes-model.mjs";
import {
  newToken,
  performWrites,
  probeCaseSensitivity,
  restoreSnapshot,
  snapshotPaths,
} from "./fixture.mjs";
import { randomHex, readJson, sha256, sha256File, treeHashDir } from "./util.mjs";

export const FIELD_NOTES_SCHEMA = "field-notes-fixture.v1";

/** Detail page every cleanup live-checks, and the article the calibration native-reload cell edits. */
export const FN_CLEANUP_DETAIL = "article-00001";
export const FN_CALIBRATION_ARTICLE = "article-00002";

// writeMethod per op; the first entry is the default.
export const FN_OP_WRITE_METHODS = {
  "edit-body": ["in-place", "atomic-rename"],
  "edit-shared-component": ["in-place", "atomic-rename"],
  "edit-managed-css": ["in-place", "atomic-rename"],
  "edit-public-css": ["in-place", "atomic-rename"],
  "edit-title": ["in-place", "atomic-rename"],
  "edit-category": ["in-place", "atomic-rename"],
  "add-article": ["create"],
  "delete-article": ["unlink"],
  "rename-slug-change": ["rename"],
  "rename-filename-only": ["rename"],
  "edit-invalid-frontmatter": ["in-place", "atomic-rename"],
  "hidden-edit": ["in-place", "atomic-rename"],
  burst: ["in-place", "atomic-rename"],
  "rapid-sequence": ["in-place", "atomic-rename", "create"],
  "edit-prop": ["in-place", "atomic-rename"],
  "rename-case-only": ["rename"],
  // Recovery ops: the method applies to every in-place step (the discovery op's new file is always `create`).
  "css-in-rejected-tick": ["in-place", "atomic-rename"],
  "css-in-rejected-discovery-tick": ["in-place", "atomic-rename"],
  "edit-during-rejected": ["in-place", "atomic-rename"],
};

/** Rejected-generation recovery ops: only in scenarios with set "recovery". */
export const FN_RECOVERY_OPS = new Set([
  "css-in-rejected-tick",
  "css-in-rejected-discovery-tick",
  "edit-during-rejected",
]);
export const FN_RECOVERY_NEEDS_MANAGED_CSS = new Set([
  "css-in-rejected-tick",
  "css-in-rejected-discovery-tick",
]);

/**
 * Visible frontmatter props `edit-prop` may edit, declared before any run:
 * the field, and where its value renders (detail selector, card selector).
 */
export const FN_EDITABLE_PROPS = {
  description: {
    detail: ".article-header .lede",
    card: "the post card's description paragraph (.card-content > p:not(.card-meta))",
    renderedOn: [
      "detail page .lede (timed when targetPath is the detail page)",
      "home card (when the article is among the first homeSize)",
      "its archive page card",
      "its category page card (when among the first categorySize)",
      "the related cards on every referrer's detail page",
    ],
  },
};

export const FN_EXPECTED_NEGATIVE_OPS = new Set([
  "rename-filename-only",
  "edit-invalid-frontmatter",
]);
export const FN_NEEDS_ARTICLE = new Set([
  "edit-body",
  "edit-title",
  "edit-category",
  "delete-article",
  "rename-slug-change",
  "rename-filename-only",
  "edit-invalid-frontmatter",
  "hidden-edit",
  "burst",
  "edit-prop",
  "rename-case-only",
  ...FN_RECOVERY_OPS,
]);

export function loadFieldNotesFixture(fixtureDir) {
  const manifest = readJson(join(fixtureDir, "manifest.json"));
  if (manifest.schema !== FIELD_NOTES_SCHEMA)
    throw new Error(`fixture manifest schema ${manifest.schema} is not ${FIELD_NOTES_SCHEMA}`);
  return manifest;
}

export function fnAdapterPath(fixtureDir, manifest, adapterId) {
  const v = manifest.adapters.variants[adapterId];
  if (!v)
    throw new Error(
      `unknown adapter ${adapterId}; known: ${Object.keys(manifest.adapters.variants).join(", ")}`,
    );
  return join(fixtureDir, v.file);
}

export const FN_SCALES_SCHEMA = "field-notes-scales.v1";
export const FN_SCALE_COUNTS = [100, 1000, 5000];

/** The versioned scale declarations (scales.json) the fixture manifest names. */
export function loadFieldNotesScales(fixtureDir, manifest = loadFieldNotesFixture(fixtureDir)) {
  if (!manifest.scales) throw new Error("field-notes fixture manifest declares no scales");
  const scales = readJson(join(fixtureDir, manifest.scales));
  if (scales.schema !== FN_SCALES_SCHEMA)
    throw new Error(`scales schema ${scales.schema} is not ${FN_SCALES_SCHEMA}`);
  return scales;
}

/** One scale's declaration (default: the declared default, 100), with the generator it was produced by. */
export function fnScale(fixtureDir, manifest, scale = null, scales = null) {
  const all = scales ?? loadFieldNotesScales(fixtureDir, manifest);
  const n = scale ?? all.default;
  const decl = all.byCount[String(n)];
  if (!decl)
    throw new Error(
      `unknown field-notes scale ${n}; declared: ${Object.keys(all.byCount).join(", ")}`,
    );
  return { scale: n, ...decl, generator: all.generator, sourceCopy: all.sourceCopy };
}

/** Pins a profile's fixture.source must carry for a scale (dir excluded). */
export const FN_SCALE_PIN_KEYS = [
  "appsZfbTreeSha256",
  "contentTreeSha256",
  "sharedTreeSha256",
  "datasetSha256",
];

/**
 * Produce a scale's source dir from the original archive: copy apps/, shared/ and scripts/ into a new
 * outDir, verify the generator's sha256, run `node scripts/generate.mjs <count>` there, then compare
 * the resulting tree hashes with the scale declaration. The archive itself is only read.
 */
export async function makeFieldNotesSource({
  fixtureDir,
  archiveDir,
  scale,
  outDir,
  scales = null,
  nodePath = process.execPath,
}) {
  const manifest = loadFieldNotesFixture(fixtureDir);
  const decl = fnScale(fixtureDir, manifest, scale, scales);
  if (existsSync(outDir)) throw new Error(`source out dir already exists: ${outDir}`);
  const generatorAt = join(archiveDir, decl.generator.path);
  const generatorSha = existsSync(generatorAt) ? sha256File(generatorAt) : null;
  if (generatorSha !== decl.generator.sha256)
    throw new Error(
      `archive generator ${generatorAt} sha256 ${generatorSha} is not the declared ${decl.generator.sha256}; nothing copied`,
    );
  mkdirSync(outDir, { recursive: true });
  for (const rel of decl.sourceCopy)
    cpSync(join(archiveDir, rel), join(outDir, rel), {
      recursive: true,
      filter: (p) => !isBuildOutputPath(relative(join(archiveDir, rel), p)),
    });
  const copiedSha = sha256File(join(outDir, decl.generator.path));
  const gen = await new Promise((resolveGen) => {
    execFile(
      nodePath,
      [decl.generator.path, decl.generatorArgs],
      {
        cwd: outDir,
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
        env: { PATH: process.env.PATH ?? "" },
      },
      (error, stdout, stderr) =>
        resolveGen({
          status: error ? (typeof error.code === "number" ? error.code : null) : 0,
          signal: error?.signal ?? null,
          stdout,
          stderr,
        }),
    );
  });
  const generator = {
    path: decl.generator.path,
    sha256: copiedSha,
    args: [decl.generatorArgs],
    node: nodePath,
    status: gen.status,
    signal: gen.signal,
    stdout: gen.stdout,
    stderr: gen.stderr,
  };
  if (gen.status !== 0) return { ok: false, scale: decl.scale, outDir, generator, checks: [] };
  const actual = {
    ...sourceTreeHashes(outDir, manifest),
    contentManifestSha256: sha256File(join(outDir, manifest.source.content, "manifest.json")),
  };
  const checks = [...FN_SCALE_PIN_KEYS, "contentManifestSha256"].map((k) => ({
    name: `scale-${decl.scale}:${k}`,
    ok: actual[k] === decl[k],
    expected: decl[k],
    actual: actual[k],
  }));
  return {
    ok: checks.every((c) => c.ok),
    scale: decl.scale,
    scaleId: decl.id,
    outDir,
    archiveDir,
    generator,
    checks,
    source: Object.fromEntries([["dir", outDir], ...FN_SCALE_PIN_KEYS.map((k) => [k, actual[k]])]),
  };
}

const skipBuildDirs = (_rel, kind, name) =>
  !(
    kind === "dir" &&
    (name === "node_modules" || name === "dist" || name === ".zfb" || name === ".zfb-build")
  );

/** Tree hashes of the parent-provided source dir S (apps/zfb without lib/content.ts, content/, shared/) and the dataset hash. */
export function sourceTreeHashes(sourceDir, manifest) {
  const appRel = manifest.source.app;
  const helperRel = manifest.source.helper;
  const app = treeHashDir(join(sourceDir, appRel), {
    filter: (rel, kind, name) => skipBuildDirs(rel, kind, name) && rel !== helperRel,
  });
  return {
    appsZfbTreeSha256: app.sha256,
    contentTreeSha256: treeHashDir(join(sourceDir, manifest.source.content)).sha256,
    sharedTreeSha256: treeHashDir(join(sourceDir, manifest.source.shared)).sha256,
    datasetSha256: datasetSha256(join(sourceDir, manifest.source.content)),
  };
}

/** scripts/generate.mjs datasetHash: sha256 over slug + "\0" + file bytes, in manifest order. */
export function datasetSha256(contentDir) {
  return datasetSha256Of(readJson(join(contentDir, "manifest.json")), join(contentDir, "articles"));
}

/** The same dataset hash over an articles dir that may live elsewhere (internal placement). */
export function datasetSha256Of(contentManifest, articlesDir) {
  const h = createHash("sha256");
  for (const a of contentManifest.articles)
    h.update(a.slug)
      .update("\0")
      .update(readFileSync(join(articlesDir, `${a.slug}.md`)));
  return h.digest("hex");
}

export const FN_PLACEMENTS = ["external", "internal"];

/** The manifest's declaration for a content placement (external = the source layout, unchanged). */
export function fnPlacement(manifest, placement = "external") {
  const p = manifest.placements?.[placement];
  if (!p) throw new Error(`unknown field-notes placement ${placement}`);
  return p;
}

/** Overlay provenance checks for a placement: the base config at the source and the overlay bytes. */
export function overlayChecks(fixtureDir, sourceDir, manifest, placement) {
  const p = fnPlacement(manifest, placement);
  if (!p.config) return [];
  const base = join(sourceDir, manifest.source.app, p.config.target);
  const baseActual = existsSync(base) ? sha256File(base) : null;
  const overlayActual = sha256File(join(fixtureDir, p.config.overlay));
  return [
    {
      name: `placement-overlay:${placement}:base`,
      ok: baseActual === p.config.baseSha256,
      expected: p.config.baseSha256,
      actual: baseActual,
      provenance: p.provenance,
    },
    {
      name: `placement-overlay:${placement}:overlay`,
      ok: overlayActual === p.config.overlaySha256,
      expected: p.config.overlaySha256,
      actual: overlayActual,
      provenance: p.provenance,
    },
  ];
}

/**
 * Copy S (apps/zfb, content, shared) into the work dir, install the adapter and the optional managed CSS.
 * Placement "internal" copies the articles into apps/zfb/content/articles instead and writes the
 * versioned derived config overlay; both the base config and the overlay are hash-checked first.
 */
export function materializeFieldNotes({
  fixtureDir,
  sourceDir,
  workDir,
  adapter,
  managedCss = false,
  placement = "external",
  scale = null,
  expectedDatasetSha256 = null,
}) {
  const manifest = loadFieldNotesFixture(fixtureDir);
  const scaleDecl = fnScale(fixtureDir, manifest, scale);
  // The pinned dataset is the scale's unless a caller (a unit test's fake corpus) names its own.
  const wantDataset = expectedDatasetSha256 ?? scaleDecl.datasetSha256;
  const place = fnPlacement(manifest, placement);
  if (existsSync(workDir)) throw new Error(`work dir already exists: ${workDir}`);
  const src = manifest.source;
  const failed = overlayChecks(fixtureDir, sourceDir, manifest, placement).filter((c) => !c.ok);
  if (failed.length)
    throw new Error(`placement ${placement} overlay identity mismatch: ${JSON.stringify(failed)}`);
  const sourceDataset = datasetSha256(join(sourceDir, src.content));
  if (sourceDataset !== wantDataset)
    throw new Error(
      `source dataset ${sourceDataset} is not the pinned dataset ${wantDataset}; nothing materialized`,
    );
  mkdirSync(workDir, { recursive: true });
  const roots =
    placement === "internal" ? [src.app, src.shared] : [src.app, src.content, src.shared];
  for (const rel of roots) {
    mkdirSync(dirname(join(workDir, rel)), { recursive: true });
    cpSync(join(sourceDir, rel), join(workDir, rel), {
      recursive: true,
      // Judge only the part below the copied root, so a source dir that itself sits under e.g. /dist/ still copies.
      filter: (p) => !isBuildOutputPath(relative(join(sourceDir, rel), p)),
    });
  }
  const appDir = join(workDir, src.app);
  const helperPath = join(appDir, src.helper);
  writeFileSync(helperPath, readFileSync(fnAdapterPath(fixtureDir, manifest, adapter)));
  let managedCssPath = null;
  if (managedCss) {
    managedCssPath = join(appDir, manifest.managedCss.target);
    mkdirSync(dirname(managedCssPath), { recursive: true });
    writeFileSync(managedCssPath, readFileSync(join(fixtureDir, manifest.managedCss.file)));
  }
  let contentDir;
  let contentManifest;
  const configPath = join(appDir, "zfb.config.ts");
  if (placement === "internal") {
    contentDir = join(workDir, place.contentDest);
    if (existsSync(contentDir))
      throw new Error(`internal content dir already exists: ${contentDir}`);
    mkdirSync(dirname(contentDir), { recursive: true });
    cpSync(join(sourceDir, src.content, "articles"), contentDir, { recursive: true });
    contentManifest = readJson(join(sourceDir, src.content, "manifest.json"));
    writeFileSync(configPath, readFileSync(join(fixtureDir, place.config.overlay)));
  } else {
    contentDir = join(workDir, src.content, "articles");
    contentManifest = readJson(join(workDir, src.content, "manifest.json"));
  }
  const model = createModel(contentManifest, readJson(join(appDir, "lib", "spec.json")));
  const configSha256 = sha256File(configPath);
  const materializedDatasetSha256 = datasetSha256Of(contentManifest, contentDir);
  if (materializedDatasetSha256 !== wantDataset)
    throw new Error(
      `materialized dataset ${materializedDatasetSha256} (${placement}) is not the pinned dataset ${wantDataset}`,
    );
  return {
    kind: "field-notes",
    manifest,
    placement,
    workDir,
    appDir,
    contentDir,
    layout: {
      placement,
      app: relative(workDir, appDir),
      contentDir: relative(workDir, contentDir),
      collectionPath: place.collectionPath,
      allowOutsideRoot: place.allowOutsideRoot === true,
      externalContentCopied: placement !== "internal",
    },
    materializedDatasetSha256,
    scale: scaleDecl.scale,
    scaleId: scaleDecl.id,
    layoutPath: join(appDir, manifest.paths.layout),
    publicCssPath: join(appDir, manifest.paths.publicCss),
    managedCssPath,
    helperPath,
    adapterSha256: sha256File(helperPath),
    trailingSlash: true,
    model,
    route: (key, slug) => fnRoute(key, slug),
    configSha256,
    configFile: "zfb.config.ts",
    configProvenance: place.config ? "derived-overlay" : "source-dir",
    configBaseSha256: place.config?.baseSha256 ?? configSha256,
    configOverlay: place.config
      ? {
          file: place.config.overlay,
          provenance: place.provenance,
          baseSha256: place.config.baseSha256,
          overlaySha256: place.config.overlaySha256,
          diff: place.config.diff,
        }
      : null,
  };
}

export function isBuildOutputPath(rel) {
  return rel
    .split(sep)
    .some(
      (part) =>
        part === "node_modules" || part === "dist" || part === ".zfb" || part === ".zfb-build",
    );
}

/** Next generated article number: max existing article-NNNNN number + 1 (gaps from deletes are never reused). */
export function nextArticleNumber(model) {
  let max = 0;
  for (const a of model.articles) {
    const m = /^article-(\d+)$/.exec(a.slug);
    if (m) max = Math.max(max, Number(m[1]));
  }
  return max + 1;
}

export function fnRoute(key, slug) {
  if (key === "home") return "/";
  if (key === "article") return href(slug);
  if (key === "independent") return "/archive/1/";
  if (key === "archive") return `/archive/${slug}/`;
  if (key === "category") return `/category/${slug}/`;
  throw new Error(`unknown route ${key}`);
}

// ---- selectors (from the original SPEC, verified against apps/zfb/components and pages) ----
export const FN_SEL = {
  body: (slug) => `[data-article-body="${slug}"]`,
  title: ".article-header h1",
  eyebrow: ".article-header .eyebrow",
  relatedLinks: "section.related .post-card h3 a",
  relatedLinkTo: (slug) => `section.related .post-card h3 a[href="${href(slug)}"]`,
  cardLink: (slug) => `.card-grid .post-card h3 a[href="${href(slug)}"]`,
  cardLinks: ".card-grid .post-card h3 a",
  cardMeta: (slug) => `.post-card:has(h3 a[href="${href(slug)}"]) .card-meta a`,
  lede: ".article-header .lede",
  cardDescription: (slug) =>
    `.card-grid .post-card:has(h3 a[href="${href(slug)}"]) .card-content > p:not(.card-meta)`,
  relatedDescription: (slug) =>
    `section.related .post-card:has(h3 a[href="${href(slug)}"]) .card-content > p:not(.card-meta)`,
  heroCount: ".hero-stat strong",
  footerNote: ".site-footer > p",
};

const text = (selector, op, value) => ({ kind: "text", selector, [op]: value });

// The home page with an empty collection (content snapshot failed): hero count "0" and no cards.
export const EMPTY_HOME_PROBE = {
  kind: "all",
  of: [
    { kind: "text", selector: FN_SEL.heroCount, equals: "0" },
    { kind: "count", selector: FN_SEL.cardLinks, equals: 0 },
  ],
};
const count = (selector, equals) => ({ kind: "count", selector, equals });
const hrefList = (selector, list) => ({
  kind: "attribute-list",
  selector,
  name: "href",
  equals: list,
});

function articlePath(m, slug) {
  return join(m.contentDir, `${slug}.md`);
}

function readArticle(m, slug) {
  return readFileSync(articlePath(m, slug), "utf8");
}

/** Where a slug's card lives: [path, predicate] for its listing on the archive page. */
function archiveCard(model, slug, predicate) {
  const p = placement(model, slug);
  return {
    path: fnRoute("archive", String(p.archivePage)),
    predicate: predicate(FN_SEL.cardLink(slug)),
  };
}

/** Supplemental: related cards of every referrer equal the recomputed list. */
function relatedChecks(model, slugs) {
  return [...new Set(slugs)]
    .filter((s) => computeIndex(model).bySlug.has(s))
    .map((s) => ({
      path: href(s),
      predicate: hrefList(FN_SEL.relatedLinks, relatedHrefs(model, s)),
    }));
}

function titleTargetPredicate(model, slug, targetPath, token) {
  if (targetPath === "/") {
    if (!placement(model, slug).onHome)
      throw new Error(
        `edit-title targetPath "/" needs ${slug} among the first ${model.spec.homeSize}`,
      );
    return text(FN_SEL.cardLink(slug), "includes", token);
  }
  if (targetPath === href(slug)) return text(FN_SEL.title, "includes", token);
  return text(FN_SEL.cardLink(slug), "includes", token);
}

function nextCategory(model, current) {
  const cats = model.spec.categories;
  return cats[(cats.findIndex((c) => c.slug === current) + 1) % cats.length].slug;
}

function categoryMembershipChecks(model, slug, oldCategory) {
  const checks = [];
  const p = placement(model, slug);
  if (oldCategory && oldCategory !== p.category)
    checks.push({
      path: fnRoute("category", oldCategory),
      predicate: count(FN_SEL.cardLink(slug), 0),
    });
  checks.push({
    path: fnRoute("category", p.category),
    predicate: count(FN_SEL.cardLink(slug), p.onCategoryPage ? 1 : 0),
  });
  return checks;
}

function heroText(n) {
  return n.toLocaleString("en-US");
}

/**
 * Plan one op against the current model. Returns the attempt (writes, timed
 * predicate on targetPath, supplemental checks after the endpoint) and the
 * model the files will describe once the writes complete (`nextModel`).
 */
export function planFieldNotesOp(m, model, op, { token = newToken(), tokens = null } = {}) {
  const methods = FN_OP_WRITE_METHODS[op.op];
  if (!methods) throw new Error(`unknown field-notes op ${op.op}`);
  const method = op.writeMethod ?? methods[0];
  if (!methods.includes(method))
    throw new Error(`op ${op.op} does not allow writeMethod ${method}`);
  const slug = op.article;
  const idx = computeIndex(model);
  if (FN_NEEDS_ARTICLE.has(op.op) && !idx.bySlug.has(slug))
    throw new Error(`op ${op.op} needs an existing article (got ${slug})`);
  const base = { token, mode: "standard", supplemental: [], touched: slug ? [slug] : [] };
  const inPlace = (path, bytes) => ({ method, path, bytes: Buffer.from(bytes) });

  switch (op.op) {
    case "edit-body": {
      const target = op.targetPath ?? href(slug);
      return {
        ...base,
        targetPath: target,
        predicate: text(FN_SEL.body(slug), "includes", token),
        writes: [inPlace(articlePath(m, slug), tokenInFirstParagraph(readArticle(m, slug), token))],
        nextModel: model,
      };
    }
    case "edit-shared-component": {
      const src = readFileSync(m.layoutPath, "utf8");
      const marker = m.manifest.layoutFooter;
      if (!src.includes(marker)) throw new Error("Layout.tsx footer text not found");
      return {
        ...base,
        targetPath: op.targetPath ?? "/",
        predicate: text(FN_SEL.footerNote, "includes", token),
        supplemental: [
          { path: "/archive/1/", predicate: text(FN_SEL.footerNote, "includes", token) },
        ],
        writes: [inPlace(m.layoutPath, src.replace(marker, `${marker} ${token}`))],
        nextModel: model,
      };
    }
    case "edit-managed-css": {
      if (!m.managedCssPath)
        throw new Error("edit-managed-css needs fixture.managedCss (or importedCss): true");
      if (m.managedCssEdit?.mode === "insert-root") {
        // The archive's imported-CSS variants are exact shared CSS with no marker: add or change one
        // custom property in the first :root{} block.
        const prop = m.managedCssEdit.customProperty;
        const src = readFileSync(m.managedCssPath, "utf8");
        const re = new RegExp(`${prop}:[^;}]*;`);
        const next = re.test(src)
          ? src.replace(re, `${prop}:${token};`)
          : src.replace(":root{", `:root{${prop}:${token};`);
        if (next === src) throw new Error("imported css :root block not found");
        return {
          ...base,
          targetPath: op.targetPath ?? "/",
          predicate: { kind: "computed-style", selector: ":root", property: prop, includes: token },
          writes: [inPlace(m.managedCssPath, next)],
          nextModel: model,
        };
      }
      const prop = m.manifest.managedCss.customProperty;
      const src = readFileSync(m.managedCssPath, "utf8");
      const re = new RegExp(`${prop}: [^;]*;`);
      if (!re.test(src)) throw new Error("managed css marker not found");
      return {
        ...base,
        targetPath: op.targetPath ?? "/",
        predicate: { kind: "computed-style", selector: ":root", property: prop, includes: token },
        writes: [inPlace(m.managedCssPath, src.replace(re, `${prop}: ${token};`))],
        nextModel: model,
      };
    }
    case "edit-public-css": {
      const prop = m.manifest.publicCss.customProperty;
      const src = readFileSync(m.publicCssPath, "utf8");
      const re = new RegExp(`${prop}:[^;}]*;`);
      const next = re.test(src)
        ? src.replace(re, `${prop}:${token};`)
        : src.replace(":root{", `:root{${prop}:${token};`);
      if (next === src) throw new Error("public css :root block not found");
      return {
        ...base,
        targetPath: op.targetPath ?? "/",
        predicate: { kind: "computed-style", selector: ":root", property: prop, includes: token },
        writes: [inPlace(m.publicCssPath, next)],
        nextModel: model,
      };
    }
    case "edit-title": {
      const old = idx.bySlug.get(slug);
      const title = `${old.title} ${token}`;
      const target = op.targetPath ?? href(slug);
      const next = setTitle(model, slug, title);
      const p = placement(next, slug);
      const sup = [];
      if (target !== href(slug))
        sup.push({ path: href(slug), predicate: text(FN_SEL.title, "includes", token) });
      if (p.onHome && target !== "/")
        sup.push({ path: "/", predicate: text(FN_SEL.cardLink(slug), "includes", token) });
      if (p.onCategoryPage)
        sup.push({
          path: fnRoute("category", p.category),
          predicate: text(FN_SEL.cardLink(slug), "includes", token),
        });
      sup.push(archiveCard(next, slug, (s) => text(s, "includes", token)));
      for (const r of referrersOf(next, slug))
        sup.push({ path: href(r), predicate: text(FN_SEL.relatedLinkTo(slug), "includes", token) });
      return {
        ...base,
        targetPath: target,
        predicate: titleTargetPredicate(next, slug, target, token),
        supplemental: sup,
        writes: [
          inPlace(articlePath(m, slug), setFrontmatter(readArticle(m, slug), "title", title)),
        ],
        nextModel: next,
      };
    }
    case "edit-category": {
      const old = idx.bySlug.get(slug);
      const to = op.toCategory ?? nextCategory(model, old.category);
      const label = labelOf(model, to);
      const next = setCategory(model, slug, to);
      const p = placement(next, slug);
      const textNow = setFrontmatter(
        setFrontmatter(readArticle(m, slug), "category", to),
        "categoryLabel",
        label,
      );
      const sup = [...categoryMembershipChecks(next, slug, old.category)];
      if (p.onCategoryPage)
        sup.push({
          path: fnRoute("category", to),
          predicate: text(FN_SEL.cardMeta(slug), "equals", label),
        });
      if (p.onHome)
        sup.push({ path: "/", predicate: text(FN_SEL.cardMeta(slug), "equals", label) });
      sup.push(
        ...relatedChecks(next, [slug, ...referrersOf(model, slug), ...referrersOf(next, slug)]),
      );
      return {
        ...base,
        targetPath: op.targetPath ?? href(slug),
        predicate: text(FN_SEL.eyebrow, "equals", label),
        supplemental: sup,
        writes: [inPlace(articlePath(m, slug), textNow)],
        nextModel: next,
        declared: { from: old.category, to, label },
      };
    }
    case "add-article": {
      const n = nextArticleNumber(model);
      const meta = {
        ...generatedMetadata(model, n),
        ...(op.category
          ? { category: op.category, categoryLabel: labelOf(model, op.category) }
          : {}),
      };
      const added = {
        ...meta,
        title: `A new field note ${token}`,
        description: `Field note ${n}: added during dev-browser-observe.`,
      };
      if (idx.bySlug.has(added.slug)) throw new Error(`${added.slug} already exists`);
      const next = addArticle(model, added);
      const p = placement(next, added.slug);
      const sup = [
        { path: href(added.slug), predicate: text(FN_SEL.body(added.slug), "includes", token) },
        archiveCard(next, added.slug, (s) => count(s, 1)),
        {
          path: fnRoute("category", p.category),
          predicate: count(FN_SEL.cardLink(added.slug), p.onCategoryPage ? 1 : 0),
        },
      ];
      return {
        ...base,
        touched: [added.slug],
        targetPath: op.targetPath ?? "/",
        predicate: text(FN_SEL.heroCount, "equals", heroText(next.articles.length)),
        supplemental: sup,
        writes: [
          {
            method: "create",
            path: articlePath(m, added.slug),
            bytes: Buffer.from(newArticleText(added, token)),
          },
        ],
        nextModel: next,
        declared: { slug: added.slug, count: next.articles.length },
      };
    }
    case "delete-article": {
      const old = idx.bySlug.get(slug);
      const before = placement(model, slug);
      const next = removeArticle(model, slug);
      const sup = [
        { kind: "gone", path: href(slug), absentText: old.title },
        {
          path: fnRoute("archive", String(before.archivePage)),
          predicate: count(FN_SEL.cardLink(slug), 0),
        },
        { path: fnRoute("category", before.category), predicate: count(FN_SEL.cardLink(slug), 0) },
        ...relatedChecks(next, referrersOf(model, slug)),
      ];
      return {
        ...base,
        targetPath: op.targetPath ?? "/",
        predicate: text(FN_SEL.heroCount, "equals", heroText(next.articles.length)),
        supplemental: sup,
        writes: [{ method: "unlink", path: articlePath(m, slug) }],
        nextModel: next,
      };
    }
    case "rename-slug-change": {
      const to = op.to ?? `${slug}-renamed`;
      const old = idx.bySlug.get(slug);
      const next = renameSlug(model, slug, to);
      const p = placement(next, to);
      const target = op.targetPath ?? (p.onHome ? "/" : fnRoute("archive", String(p.archivePage)));
      const textNow = setFrontmatter(readArticle(m, slug), "slug", to);
      const sup = [
        { kind: "gone", path: href(slug), absentText: old.title },
        { path: href(to), predicate: count(FN_SEL.body(to), 1) },
        ...categoryMembershipChecks(next, to, null),
        { path: fnRoute("category", p.category), predicate: count(FN_SEL.cardLink(slug), 0) },
      ];
      return {
        ...base,
        touched: [slug, to],
        targetPath: target,
        predicate: count(FN_SEL.cardLink(to), 1),
        supplemental: sup,
        // Rename first, then make the frontmatter slug agree; the record keeps both step times.
        writes: [
          { method: "rename", path: articlePath(m, slug), to: articlePath(m, to) },
          { method: "in-place", path: articlePath(m, to), bytes: Buffer.from(textNow) },
        ],
        nextModel: next,
      };
    }
    case "rename-filename-only": {
      const to = op.to ?? `${slug}-renamed`;
      const original = computeIndex(model);
      return {
        ...base,
        mode: "expected-negative",
        touched: [slug, to],
        targetPath: "/",
        writes: [{ method: "rename", path: articlePath(m, slug), to: articlePath(m, to) }],
        expectState: op.expectState ?? ["error", "last-good"],
        observe: [
          {
            path: "/",
            lastGood: hrefList(
              FN_SEL.cardLinks,
              original.articles.slice(0, model.spec.homeSize).map((a) => href(a.slug)),
            ),
            empty: EMPTY_HOME_PROBE,
          },
          {
            path: href(slug),
            lastGood: text(FN_SEL.title, "equals", original.bySlug.get(slug).title),
          },
        ],
        errorMarkers: ["Article slug mismatch"],
        recovery: [{ method: "rename", path: articlePath(m, to), to: articlePath(m, slug) }],
        recoveryChecks: liveRestoreChecks(model, [slug]),
        nextModel: model,
      };
    }
    case "edit-invalid-frontmatter": {
      const original = readArticle(m, slug);
      return {
        ...base,
        mode: "expected-negative",
        targetPath: href(slug),
        writes: [inPlace(articlePath(m, slug), breakFrontmatter(original))],
        expectState: op.expectState ?? ["error", "last-good"],
        observe: [
          { path: href(slug), lastGood: text(FN_SEL.title, "equals", idx.bySlug.get(slug).title) },
          {
            path: "/",
            lastGood: text(FN_SEL.heroCount, "equals", heroText(idx.articles.length)),
            empty: EMPTY_HOME_PROBE,
          },
        ],
        errorMarkers: [],
        recovery: [
          { method: "in-place", path: articlePath(m, slug), bytes: Buffer.from(original) },
        ],
        recoveryChecks: liveRestoreChecks(model, [slug]),
        nextModel: model,
      };
    }
    case "hidden-edit": {
      const active = op.targetPath ?? "/";
      if (active === href(slug))
        throw new Error("hidden-edit needs an active page other than the edited article");
      return {
        ...base,
        mode: "hidden",
        targetPath: active,
        hiddenPath: href(slug),
        hiddenPredicate: text(FN_SEL.body(slug), "includes", token),
        observeMs: op.observeMs ?? 3000,
        accessDelayMs: op.accessDelayMs ?? 1000,
        writes: [inPlace(articlePath(m, slug), tokenInFirstParagraph(readArticle(m, slug), token))],
        nextModel: model,
      };
    }
    case "burst": {
      const n = op.count ?? 5;
      const spacing = op.spacingMs ?? 50;
      const list = tokens ?? Array.from({ length: n }, () => newToken());
      const old = idx.bySlug.get(slug);
      const original = readArticle(m, slug);
      const target = op.targetPath ?? href(slug);
      const finalToken = list[list.length - 1];
      const next = setTitle(model, slug, `${old.title} ${finalToken}`);
      const sel = target === href(slug) ? FN_SEL.title : FN_SEL.cardLink(slug);
      return {
        ...base,
        token: finalToken,
        tokens: list,
        targetPath: target,
        predicate: titleTargetPredicate(next, slug, target, finalToken),
        watch: [{ selector: sel, tokens: list.slice(0, -1) }],
        writes: list.map((t, i) => ({
          ...inPlace(articlePath(m, slug), setFrontmatter(original, "title", `${old.title} ${t}`)),
          afterMs: i === 0 ? 0 : spacing,
        })),
        nextModel: next,
      };
    }
    case "rapid-sequence": {
      const variant = op.variant ?? "title-then-category";
      const spacing = op.spacingMs ?? 100;
      if (variant === "title-then-category") {
        if (!idx.bySlug.has(slug))
          throw new Error("rapid-sequence title-then-category needs an existing article");
        const old = idx.bySlug.get(slug);
        const to = op.toCategory ?? nextCategory(model, old.category);
        const label = labelOf(model, to);
        const title = `${old.title} ${token}`;
        const step1 = setFrontmatter(readArticle(m, slug), "title", title);
        const step2 = setFrontmatter(setFrontmatter(step1, "category", to), "categoryLabel", label);
        const next = setCategory(setTitle(model, slug, title), slug, to);
        const im = method === "create" ? "in-place" : method;
        return {
          ...base,
          targetPath: op.targetPath ?? href(slug),
          predicate: {
            kind: "all",
            of: [text(FN_SEL.title, "includes", token), text(FN_SEL.eyebrow, "equals", label)],
          },
          supplemental: [
            ...categoryMembershipChecks(next, slug, old.category),
            ...relatedChecks(next, [slug, ...referrersOf(model, slug), ...referrersOf(next, slug)]),
          ],
          writes: [
            { method: im, path: articlePath(m, slug), bytes: Buffer.from(step1) },
            { method: im, path: articlePath(m, slug), bytes: Buffer.from(step2), afterMs: spacing },
          ],
          nextModel: next,
          declared: { finalTitle: title, finalCategory: to, finalLabel: label },
        };
      }
      if (variant === "add-then-delete") {
        const n = nextArticleNumber(model);
        const meta = {
          ...generatedMetadata(model, n),
          title: `A transient field note ${token}`,
          description: "Created then deleted by dev-browser-observe.",
        };
        const p = articlePath(m, meta.slug);
        const pagesNow = Math.ceil(idx.articles.length / model.spec.pageSize);
        return {
          ...base,
          mode: "final-state",
          touched: [meta.slug],
          targetPath: op.targetPath ?? "/",
          writes: [
            { method: "create", path: p, bytes: Buffer.from(newArticleText(meta, token)) },
            { method: "unlink", path: p, afterMs: spacing },
          ],
          finalChecks: [
            {
              path: "/",
              predicate: text(FN_SEL.heroCount, "equals", heroText(idx.articles.length)),
            },
            { kind: "gone", path: href(meta.slug), absentText: token },
            { kind: "gone", path: fnRoute("archive", String(pagesNow + 1)), absentText: token },
          ],
          nextModel: model,
          declared: {
            finalState: "unchanged",
            articleCount: idx.articles.length,
            transientSlug: meta.slug,
          },
        };
      }
      throw new Error(`unknown rapid-sequence variant ${variant}`);
    }
    case "edit-prop": {
      const field = op.prop ?? "description";
      if (!FN_EDITABLE_PROPS[field]) throw new Error(`edit-prop cannot edit ${field}`);
      const old = idx.bySlug.get(slug);
      const value = `${old[field]} ${token}`;
      const next = setField(model, slug, field, value);
      const p = placement(next, slug);
      const target = op.targetPath ?? href(slug);
      const cardText = (s) => text(s, "includes", token);
      let predicate;
      if (target === href(slug)) predicate = text(FN_SEL.lede, "includes", token);
      else if (target === "/") {
        if (!p.onHome)
          throw new Error(
            `edit-prop targetPath "/" needs ${slug} among the first ${model.spec.homeSize}`,
          );
        predicate = cardText(FN_SEL.cardDescription(slug));
      } else predicate = cardText(FN_SEL.cardDescription(slug));
      const sup = [];
      if (target !== href(slug))
        sup.push({ path: href(slug), predicate: text(FN_SEL.lede, "includes", token) });
      if (p.onHome && target !== "/")
        sup.push({ path: "/", predicate: cardText(FN_SEL.cardDescription(slug)) });
      const archivePath = fnRoute("archive", String(p.archivePage));
      if (target !== archivePath)
        sup.push({ path: archivePath, predicate: cardText(FN_SEL.cardDescription(slug)) });
      const categoryPath = fnRoute("category", p.category);
      if (p.onCategoryPage && target !== categoryPath)
        sup.push({ path: categoryPath, predicate: cardText(FN_SEL.cardDescription(slug)) });
      for (const r of referrersOf(next, slug))
        sup.push({ path: href(r), predicate: cardText(FN_SEL.relatedDescription(slug)) });
      return {
        ...base,
        targetPath: target,
        predicate,
        supplemental: sup,
        writes: [inPlace(articlePath(m, slug), setFrontmatter(readArticle(m, slug), field, value))],
        nextModel: next,
        declared: {
          field,
          from: old[field],
          to: value,
          timed: { path: target, predicate },
          expectedRendered: [{ path: target, predicate }, ...sup],
        },
      };
    }
    case "rename-case-only":
      return planCaseRename(m, model, op, { base, slug });
    case "css-in-rejected-tick":
    case "css-in-rejected-discovery-tick":
    case "edit-during-rejected":
      return planRecovery(m, model, op, { base, slug, method, token });
    default:
      throw new Error(`unknown field-notes op ${op.op}`);
  }
}

/** A case variant of a file name: the first letter flipped to upper case (e.g. Article-00008.md). */
export function caseVariantName(name) {
  const i = name.search(/[a-z]/i);
  if (i < 0) throw new Error(`no letter to change case in ${name}`);
  const c = name[i];
  const flipped = c === c.toLowerCase() ? c.toUpperCase() : c.toLowerCase();
  return name.slice(0, i) + flipped + name.slice(i + 1);
}

/** The write steps of a case-only rename: one direct rename, or two through a temp sibling. */
export function caseRenameWrites(dir, fromName, toName, steps = "direct") {
  const verifyEntries = { dir, present: [toName], absent: [fromName] };
  const from = join(dir, fromName);
  const to = join(dir, toName);
  if (steps === "direct")
    return [{ method: "rename", path: from, to, caseOnly: true, verifyEntries }];
  if (steps !== "via-temp") throw new Error(`unknown caseRenameSteps ${steps}`);
  const tmp = join(dir, `.${fromName}.dbo-case-${randomHex(4)}.tmp`);
  return [
    { method: "rename", path: from, to: tmp },
    { method: "rename", path: tmp, to, verifyEntries },
  ];
}

// The entry slug zfb-content derives from a file: the path relative to the collection root with the
// .md/.mdx/.tsx extension stripped, case preserved (crates/zfb-content/src/collection.rs parse_entry).
export const fnEntrySlugOf = (fileName) => fileName.replace(/\.(md|mdx|tsx)$/, "");

function planCaseRename(m, model, op, { base, slug }) {
  const fromName = `${slug}.md`;
  const toName = op.toName ?? caseVariantName(fromName);
  if (toName === fromName || toName.toLowerCase() !== fromName.toLowerCase())
    throw new Error(`rename-case-only toName ${toName} is not a case variant of ${fromName}`);
  const steps = op.caseRenameSteps ?? "direct";
  // Untimed, before the attempt: the work dir's real case behaviour (never assumed from the platform).
  const caseProbe = probeCaseSensitivity(m.workDir);
  const declared = {
    fromName,
    toName,
    steps,
    caseProbe,
    contentDir: m.contentDir,
    entrySlugAfter: fnEntrySlugOf(toName),
    frontmatterSlug: slug,
    expectation: op.expectState
      ? { set: "expected-negative", expectState: op.expectState }
      : { set: "workload", targetPath: op.targetPath, predicate: op.predicate },
  };
  const writes = caseRenameWrites(m.contentDir, fromName, toName, steps);
  const recovery = caseRenameWrites(m.contentDir, toName, fromName, steps);
  if (op.expectState) {
    const original = computeIndex(model);
    return {
      ...base,
      mode: "expected-negative",
      touched: [slug],
      targetPath: "/",
      writes,
      expectState: op.expectState,
      observe: [
        {
          path: "/",
          lastGood: hrefList(
            FN_SEL.cardLinks,
            original.articles.slice(0, model.spec.homeSize).map((a) => href(a.slug)),
          ),
          empty: EMPTY_HOME_PROBE,
        },
        {
          path: href(slug),
          lastGood: text(FN_SEL.title, "equals", original.bySlug.get(slug).title),
        },
      ],
      errorMarkers: ["Article slug mismatch"],
      recovery,
      recoveryChecks: liveRestoreChecks(model, [slug]),
      nextModel: model,
      declared,
    };
  }
  if (!op.predicate || !op.targetPath)
    throw new Error(
      "rename-case-only outside expected-negative needs a declared targetPath and predicate",
    );
  return {
    ...base,
    touched: [slug],
    targetPath: op.targetPath,
    predicate: op.predicate,
    supplemental: op.supplemental ?? [],
    writes,
    // The rendered model after a case rename is what the profile declared; later ops must not rely on it.
    nextModel: model,
    declared,
  };
}

/** Last two path components, the part of a path zfb's `intake paths:` line ends with on any prefix. */
export const pathTail = (p) => p.split(/[\\/]/).slice(-2).join("/");

function planRecovery(m, model, op, { base, slug, method, token }) {
  const idx = computeIndex(model);
  const old = idx.bySlug.get(slug);
  const inPlace = (path, bytes) => ({ method, path, bytes: Buffer.from(bytes) });
  const common = {
    ...base,
    mode: "recovery",
    variant: op.op,
    expectState: op.expectState,
    beforeRepair: op.beforeRepair ?? "must-be-absent",
    requireSameTick: op.requireSameTick === true,
    errorMarkers: op.errorMarkers ?? ["Article slug mismatch"],
  };
  if (op.op === "edit-during-rejected") {
    const other = op.other;
    if (!other || other === slug || !idx.bySlug.has(other))
      throw new Error(
        "edit-during-rejected needs `other`, an existing article other than `article`",
      );
    const original = readArticle(m, slug);
    const y = idx.bySlug.get(other);
    const yTitle = `${y.title} ${token}`;
    const next = setTitle(model, other, yTitle);
    const p = placement(next, other);
    const yCard = p.onHome
      ? { path: "/", predicate: text(FN_SEL.cardLink(other), "includes", token) }
      : archiveCard(next, other, (s) => text(s, "includes", token));
    return {
      ...common,
      touched: [slug, other],
      targetPath: href(other),
      rejectWrites: [inPlace(articlePath(m, slug), breakFrontmatter(original))],
      rejectedObserve: {
        kind: "paths",
        observe: [
          { path: href(slug), lastGood: text(FN_SEL.title, "equals", old.title) },
          {
            path: "/",
            lastGood: text(FN_SEL.heroCount, "equals", heroText(idx.articles.length)),
            empty: EMPTY_HOME_PROBE,
          },
        ],
      },
      duringWrites: [
        inPlace(articlePath(m, other), setFrontmatter(readArticle(m, other), "title", yTitle)),
      ],
      repairWrites: [inPlace(articlePath(m, slug), original)],
      predicate: text(FN_SEL.title, "includes", token),
      recoveryChecks: [yCard, ...liveRestoreChecks(next, [slug])],
      writes: [inPlace(articlePath(m, slug), breakFrontmatter(original))],
      nextModel: next,
      declared: {
        broken: slug,
        editedWhileRejected: other,
        title: yTitle,
        yHome: p.onHome,
        timed: "other article's title on its open detail page after the repair",
      },
    };
  }
  if (!m.managedCssPath) throw new Error(`${op.op} needs fixture.managedCss: true`);
  const prop = m.manifest.managedCss.customProperty;
  const cssSrc = readFileSync(m.managedCssPath, "utf8");
  const re = new RegExp(`${prop}: [^;]*;`);
  if (!re.test(cssSrc)) throw new Error("managed css marker not found");
  const cssWrite = inPlace(m.managedCssPath, cssSrc.replace(re, `${prop}: ${token};`));
  const predicate = { kind: "computed-style", selector: ":root", property: prop, includes: token };
  const rejectedObserve = {
    kind: "open",
    lastGood: text(FN_SEL.title, "equals", old.title),
  };
  if (op.op === "css-in-rejected-tick") {
    const original = readArticle(m, slug);
    const contentWrite = inPlace(articlePath(m, slug), breakFrontmatter(original));
    return {
      ...common,
      targetPath: href(slug),
      // CSS first, then content, as the Rust e2e does (cold_lazy_css_edit_in_rejected_tick_is_published_after_recovery).
      rejectWrites: [cssWrite, contentWrite],
      rejectedObserve,
      sameTick: { tails: [pathTail(m.managedCssPath), pathTail(articlePath(m, slug))] },
      repairWrites: [inPlace(articlePath(m, slug), original)],
      predicate,
      recoveryChecks: liveRestoreChecks(model, [slug]),
      writes: [cssWrite, contentWrite],
      nextModel: model,
      declared: { open: href(slug), broken: slug, cssToken: token, order: ["css", "content"] },
    };
  }
  // css-in-rejected-discovery-tick: a new article whose bytes are invalid, repaired in place.
  const n = nextArticleNumber(model);
  const meta = {
    ...generatedMetadata(model, n),
    title: `A recovery field note ${token}`,
    description: `Field note ${n}: created invalid, then repaired, by dev-browser-observe.`,
  };
  if (idx.bySlug.has(meta.slug)) throw new Error(`${meta.slug} already exists`);
  const invalid = op.invalid ?? "frontmatter";
  const good = newArticleText(meta, token);
  const bad =
    invalid === "slug-mismatch"
      ? setFrontmatter(good, "slug", `${meta.slug}-mismatch`)
      : invalid === "frontmatter"
        ? breakFrontmatter(good)
        : null;
  if (bad === null) throw new Error(`unknown invalid kind ${invalid}`);
  const newPath = articlePath(m, meta.slug);
  const createWrite = { method: "create", path: newPath, bytes: Buffer.from(bad) };
  const next = addArticle(model, meta);
  return {
    ...common,
    touched: [slug, meta.slug],
    targetPath: href(slug),
    rejectWrites: [cssWrite, createWrite],
    rejectedObserve,
    sameTick: { tails: [pathTail(m.managedCssPath), pathTail(newPath)] },
    repairWrites: [inPlace(newPath, good)],
    predicate,
    recoveryChecks: [
      { path: href(meta.slug), predicate: text(FN_SEL.body(meta.slug), "includes", token) },
      ...liveRestoreChecks(next, [slug]),
    ],
    writes: [cssWrite, createWrite],
    nextModel: next,
    declared: {
      open: href(slug),
      created: meta.slug,
      invalid,
      cssToken: token,
      order: ["css", "content"],
      cleanup: "scenario restoration unlinks the created article",
    },
  };
}

/** Live checks that the original state is back: home, detail pages, every archive page, one category page, counts. */
export function liveRestoreChecks(model, touched = []) {
  const idx = computeIndex(model);
  const checks = [
    {
      path: "/",
      predicate: {
        kind: "all",
        of: [
          text(FN_SEL.heroCount, "equals", heroText(idx.articles.length)),
          hrefList(
            FN_SEL.cardLinks,
            idx.articles.slice(0, model.spec.homeSize).map((a) => href(a.slug)),
          ),
        ],
      },
    },
  ];
  const details = new Set([
    idx.bySlug.has(FN_CLEANUP_DETAIL) ? FN_CLEANUP_DETAIL : idx.articles[0].slug,
    ...touched.filter((s) => idx.bySlug.has(s)),
  ]);
  for (const s of details)
    checks.push({
      path: href(s),
      predicate: {
        kind: "all",
        of: [
          text(FN_SEL.title, "equals", idx.bySlug.get(s).title),
          text(FN_SEL.eyebrow, "equals", idx.bySlug.get(s).categoryLabel),
        ],
      },
    });
  const pages = Math.ceil(idx.articles.length / model.spec.pageSize);
  for (let p = 1; p <= pages; p++)
    checks.push({
      path: fnRoute("archive", String(p)),
      predicate: hrefList(
        FN_SEL.cardLinks,
        idx.articles
          .slice((p - 1) * model.spec.pageSize, p * model.spec.pageSize)
          .map((a) => href(a.slug)),
      ),
    });
  const firstCat = model.spec.categories[0].slug;
  checks.push({
    path: fnRoute("category", firstCat),
    predicate: hrefList(
      FN_SEL.cardLinks,
      idx.categories
        .get(firstCat)
        .slice(0, model.spec.categorySize)
        .map((a) => href(a.slug)),
    ),
  });
  for (const s of touched.filter((x) => !idx.bySlug.has(x)))
    checks.push({ kind: "gone", path: href(s), absentText: s });
  return checks;
}

export function fnWarmPaths(scenario) {
  const detail = href(scenario.warmDetail ?? "article-00001");
  if (scenario.warmOrder === "index-first") return ["/", detail];
  if (scenario.warmOrder === "detail-first") return [detail, "/"];
  return [];
}

/**
 * fixtureOps for the state machine. The planner tracks the model the files
 * describe; restoration resets it to the materialized original.
 */
export function fieldNotesOps(m, { restorationWriteMethod = "in-place" } = {}) {
  const original = m.model;
  const state = { model: original, touched: new Set() };
  const snapshotSpec = {
    dirs: [m.contentDir],
    files: [m.layoutPath, m.publicCssPath, ...(m.managedCssPath ? [m.managedCssPath] : [])],
  };
  return {
    state,
    prepareAttempt: (op) => {
      const a = planFieldNotesOp(m, state.model, op);
      for (const s of a.touched ?? []) state.touched.add(s);
      a.applied = () => {
        state.model = a.nextModel;
      };
      a.write = a.writes[0];
      return a;
    },
    performWrites: (writes, now) => performWrites(writes, now),
    snapshot: () => {
      state.model = original;
      return snapshotPaths(snapshotSpec);
    },
    restore: (s) => {
      const r = restoreSnapshot(s, { writeMethod: restorationWriteMethod, renameRevert: true });
      state.model = original;
      return r;
    },
    liveChecks: () => {
      const checks = liveRestoreChecks(original, [...state.touched]);
      state.touched.clear();
      return checks;
    },
    warmPaths: fnWarmPaths,
    route: m.route,
    calibrationArticle: FN_CALIBRATION_ARTICLE,
    // zfb excludes the default public/ dir from dev watch roots (served from disk, #1165): no auto-update contract.
    noAutoUpdatePaths: [dirname(m.publicCssPath)],
  };
}

export function originalFrontmatter(m, slug) {
  const t = readArticle(m, slug);
  return {
    slug: getFrontmatter(t, "slug"),
    title: getFrontmatter(t, "title"),
    category: getFrontmatter(t, "category"),
  };
}

export { sha256 };
