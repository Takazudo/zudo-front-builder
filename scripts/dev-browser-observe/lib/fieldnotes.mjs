import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
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
  setFrontmatter,
  setTitle,
  tokenInFirstParagraph,
} from "./fieldnotes-model.mjs";
import { newToken, performWrites, restoreSnapshot, snapshotPaths } from "./fixture.mjs";
import { readJson, sha256, sha256File, treeHashDir } from "./util.mjs";

export const FIELD_NOTES_SCHEMA = "field-notes-fixture.v1";

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
  const m = readJson(join(contentDir, "manifest.json"));
  const h = createHash("sha256");
  for (const a of m.articles)
    h.update(a.slug)
      .update("\0")
      .update(readFileSync(join(contentDir, "articles", `${a.slug}.md`)));
  return h.digest("hex");
}

/** Copy S (apps/zfb, content, shared) into the work dir, install the adapter and the optional managed CSS. */
export function materializeFieldNotes({
  fixtureDir,
  sourceDir,
  workDir,
  adapter,
  managedCss = false,
}) {
  const manifest = loadFieldNotesFixture(fixtureDir);
  if (existsSync(workDir)) throw new Error(`work dir already exists: ${workDir}`);
  mkdirSync(workDir, { recursive: true });
  const src = manifest.source;
  for (const rel of [src.app, src.content, src.shared]) {
    mkdirSync(dirname(join(workDir, rel)), { recursive: true });
    cpSync(join(sourceDir, rel), join(workDir, rel), {
      recursive: true,
      filter: (p) => !/\/(node_modules|dist|\.zfb|\.zfb-build)(\/|$)/.test(p),
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
  const contentRoot = join(workDir, src.content);
  const model = createModel(
    readJson(join(contentRoot, "manifest.json")),
    readJson(join(appDir, "lib", "spec.json")),
  );
  return {
    kind: "field-notes",
    manifest,
    workDir,
    appDir,
    contentDir: join(contentRoot, "articles"),
    layoutPath: join(appDir, manifest.paths.layout),
    publicCssPath: join(appDir, manifest.paths.publicCss),
    managedCssPath,
    helperPath,
    adapterSha256: sha256File(helperPath),
    trailingSlash: true,
    model,
    route: (key, slug) => fnRoute(key, slug),
    configSha256: sha256File(join(appDir, "zfb.config.ts")),
    configFile: "zfb.config.ts",
    configProvenance: "source-dir",
  };
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
      if (!m.managedCssPath) throw new Error("edit-managed-css needs fixture.managedCss: true");
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
      const n = idx.articles.length + 1;
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
        const n = idx.articles.length + 1;
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
    default:
      throw new Error(`unknown field-notes op ${op.op}`);
  }
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
  const details = new Set([idx.articles[0].slug, ...touched.filter((s) => idx.bySlug.has(s))]);
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
    calibrationArticle: "article-00002",
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
