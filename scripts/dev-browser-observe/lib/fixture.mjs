import {
  chmodSync,
  closeSync,
  constants,
  cpSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  statSync,
  unlinkSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { basename, dirname, join } from "node:path";
import {
  hrtimeNs,
  noWriteStepsError,
  randomHex,
  readJson,
  sha256,
  treeHashDir,
  walkFiles,
} from "./util.mjs";

export const FIXTURE_SCHEMA = "tiny-fixture.v1";

export function loadFixture(fixtureDir) {
  const manifest = readJson(join(fixtureDir, "manifest.json"));
  if (manifest.schema !== FIXTURE_SCHEMA)
    throw new Error(`fixture manifest schema ${manifest.schema} is not ${FIXTURE_SCHEMA}`);
  return manifest;
}

export function adapterPath(fixtureDir, manifest, adapterId) {
  const v = manifest.adapters.variants[adapterId];
  if (!v)
    throw new Error(
      `unknown adapter ${adapterId}; known: ${Object.keys(manifest.adapters.variants).join(", ")}`,
    );
  return join(fixtureDir, v.file);
}

export function zfbConfigFor(manifest, placement, { trailingSlash = false } = {}) {
  const p = manifest.content.placements[placement];
  if (!p) throw new Error(`unknown placement ${placement}`);
  const collection = { name: manifest.content.collection, path: p.configPath };
  if (p.allowOutsideRoot) collection.allowOutsideRoot = true;
  const config = { ...manifest.config };
  if (trailingSlash) config.trailingSlash = true;
  return { ...config, collections: [collection] };
}

/** Route path for `key` ("home" | "article" | "independent"), following the trailingSlash option. */
export function routePath(manifest, key, { slug = "", trailingSlash = false } = {}) {
  const table = trailingSlash ? manifest.routesTrailingSlash : manifest.routes;
  return table[key].replace("{slug}", slug);
}

/** True when the profile shape is exactly the one the original config file encodes. */
export function originalConfigApplies(manifest, { configFormat, placement, trailingSlash }) {
  const w = manifest.configOriginal?.appliesWhen;
  if (!w) return false;
  return (
    configFormat === w.configFormat &&
    placement === w.placement &&
    trailingSlash === w.trailingSlash &&
    manifest.config?.wind === w.wind
  );
}

/**
 * The config file to write: zfb.config.json, the byte-exact original
 * zfb.config.ts when the shape matches, or a generated (reconstructed)
 * defineConfig file.
 */
export function configFileFor(
  fixtureDir,
  manifest,
  { configFormat = "json", placement, trailingSlash = false },
) {
  const config = zfbConfigFor(manifest, placement, { trailingSlash });
  if (configFormat === "json") {
    return {
      name: "zfb.config.json",
      bytes: Buffer.from(`${JSON.stringify(config, null, 2)}\n`),
      provenance: "generated-json",
    };
  }
  if (configFormat !== "ts") throw new Error(`unknown configFormat ${configFormat}`);
  if (originalConfigApplies(manifest, { configFormat, placement, trailingSlash })) {
    const bytes = readFileSync(join(fixtureDir, manifest.configOriginal.file));
    return {
      name: "zfb.config.ts",
      bytes,
      provenance: "original-input",
      originalSha256: manifest.configOriginal.sha256,
    };
  }
  const text = [
    "// RECONSTRUCTED by dev-browser-observe: generated for a profile shape the original zfb.config.ts does not cover.",
    'import { defineConfig } from "@takazudo/zfb/config";',
    "",
    `export default defineConfig(${JSON.stringify(config, null, 2)});`,
    "",
  ].join("\n");
  return { name: "zfb.config.ts", bytes: Buffer.from(text), provenance: "reconstructed" };
}

/** Copy the fixture into a fresh work dir for one run; the source fixture is never edited. */
export function materialize({
  fixtureDir,
  workDir,
  placement,
  adapter,
  trailingSlash = false,
  configFormat = "json",
}) {
  const manifest = loadFixture(fixtureDir);
  if (existsSync(workDir)) throw new Error(`work dir already exists: ${workDir}`);
  mkdirSync(workDir, { recursive: true });
  const appDir = join(workDir, manifest.app);
  cpSync(join(fixtureDir, manifest.app), appDir, { recursive: true });
  const contentDir = join(workDir, manifest.content.placements[placement].dest);
  mkdirSync(dirname(contentDir), { recursive: true });
  cpSync(join(fixtureDir, manifest.content.source), contentDir, { recursive: true });
  const adapterTarget = join(workDir, manifest.adapters.target);
  mkdirSync(dirname(adapterTarget), { recursive: true });
  cpSync(adapterPath(fixtureDir, manifest, adapter), adapterTarget);
  const configFile = configFileFor(fixtureDir, manifest, {
    configFormat,
    placement,
    trailingSlash,
  });
  writeFileSync(join(appDir, configFile.name), configFile.bytes);
  const articles = Object.fromEntries(
    manifest.articles.map((slug) => [slug, join(contentDir, `${slug}.md`)]),
  );
  return {
    manifest,
    trailingSlash,
    route: (key, slug) => routePath(manifest, key, { slug, trailingSlash }),
    workDir,
    appDir,
    contentDir,
    cssPath: join(workDir, manifest.css.managed),
    articles,
    configFormat,
    configFile: configFile.name,
    configProvenance: configFile.provenance,
    configOriginalSha256: configFile.originalSha256 ?? null,
    configSha256: sha256(configFile.bytes),
    adapterSha256: sha256(readFileSync(adapterTarget)),
    materializedTreeSha256: treeHashDir(workDir, {
      filter: (_r, _k, name) => name !== ".zfb" && name !== "node_modules",
    }).sha256,
  };
}

export function newToken() {
  return `tok-${randomHex(6)}`;
}

function replaceLine(text, re, line, what) {
  if (!re.test(text)) throw new Error(`fixture file has no ${what} line`);
  return text.replace(re, line);
}

/** Pure content transforms; each returns the new file bytes for the op. */
export const TRANSFORMS = {
  "edit-title": (text, token, slug) =>
    replaceLine(text, /^title: .*$/m, `title: Article ${slug.toUpperCase()} ${token}`, "title"),
  "edit-category": (text, token) =>
    replaceLine(text, /^category: .*$/m, `category: ${token}`, "category"),
  "edit-body": (text, token, slug) =>
    replaceLine(
      text,
      /^Body marker .*$/m,
      `Body marker ${token} for article ${slug}.`,
      "body marker",
    ),
  "stale-predicate": (text, token, slug) =>
    replaceLine(
      text,
      /^Body marker .*$/m,
      `Body marker ${token} for article ${slug}.`,
      "body marker",
    ),
  "edit-css": (text, token) =>
    replaceLine(
      text,
      /--observe-css-marker: [^;]*;/,
      `--observe-css-marker: ${token};`,
      "css marker",
    ),
};

// Which write methods each op may use; the first is the default when the profile omits one.
export const OP_WRITE_METHODS = {
  "edit-title": ["in-place", "atomic-rename"],
  "edit-body": ["in-place", "atomic-rename"],
  "edit-category": ["in-place", "atomic-rename"],
  "edit-css": ["in-place", "atomic-rename"],
  "stale-predicate": ["in-place", "atomic-rename"],
  "delete-article": ["unlink"],
  "create-article": ["create"],
  "rename-article": ["rename"],
};

export const RESTORATION_WRITE_METHODS = ["in-place", "atomic-rename"];

export function resolveWriteMethod(op) {
  const allowed = OP_WRITE_METHODS[op.op];
  if (!allowed) throw new Error(`unknown op ${op.op}`);
  const method = op.writeMethod ?? allowed[0];
  if (!allowed.includes(method))
    throw new Error(
      `op ${op.op} does not allow writeMethod ${method}; allowed: ${allowed.join(", ")}`,
    );
  return method;
}

export function renamedSlug(op) {
  return op.to ?? `${op.article}-renamed`;
}

function sel(manifest, key, slug) {
  return manifest.selectors[key].replaceAll("{slug}", slug);
}

/**
 * Default timed predicate and supplemental checks for an op, before token
 * substitution. Title/category edits follow the op's target page: on the
 * detail page the home listing is the supplemental check, and on the home
 * page (e.g. targetPath "/") the detail page is.
 */
export function defaultsFor(manifest, op, { trailingSlash = false } = {}) {
  const slug = op.article;
  const route = (key, s = slug) => routePath(manifest, key, { slug: s, trailingSlash });
  const text = (key, s = slug, value = "{token}") => ({
    kind: "text",
    selector: sel(manifest, key, s),
    includes: value,
  });
  const homeOrDetail = (detailKey, homeKey) => {
    const targetPath = op.targetPath ?? route("article");
    const onHome = targetPath === route("home");
    return {
      targetPath,
      predicate: onHome ? text(homeKey) : text(detailKey),
      supplemental: [
        onHome
          ? { path: route("article"), predicate: text(detailKey) }
          : { path: route("home"), predicate: text(homeKey) },
      ],
    };
  };
  switch (op.op) {
    case "edit-title":
      return homeOrDetail("articleTitle", "homeTitle");
    case "edit-category":
      return homeOrDetail("articleCategory", "homeCategory");
    case "edit-body":
      return { targetPath: route("article"), predicate: text("articleBody"), supplemental: [] };
    case "stale-predicate":
      return {
        targetPath: route("article"),
        predicate: text("articleBody", slug, "{unwrittenToken}"),
        supplemental: [],
      };
    case "edit-css":
      return {
        targetPath: route("independent"),
        predicate: {
          kind: "computed-style",
          selector: ":root",
          property: manifest.css.customProperty,
          includes: "{token}",
        },
        supplemental: [],
      };
    case "delete-article":
      return {
        targetPath: route("home"),
        predicate: { kind: "absent", selector: sel(manifest, "homeItem", slug) },
        supplemental: [],
      };
    case "create-article":
      return { targetPath: route("home"), predicate: text("homeTitle"), supplemental: [] };
    case "rename-article":
      return {
        targetPath: route("home"),
        predicate: {
          kind: "count",
          selector: sel(manifest, "homeItem", renamedSlug(op)),
          equals: 1,
        },
        supplemental: [],
      };
    default:
      throw new Error(`unknown op ${op.op}`);
  }
}

export function substitute(value, vars) {
  if (typeof value === "string")
    return value.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? vars[k] : m));
  if (Array.isArray(value)) return value.map((v) => substitute(v, vars));
  if (value && typeof value === "object")
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, substitute(v, vars)]));
  return value;
}

/** Concrete attempt for one op: file target, new bytes, predicate and supplemental checks with tokens filled in. */
export function prepareAttempt(
  materialized,
  op,
  { token = newToken(), unwrittenToken = newToken() } = {},
) {
  const { manifest } = materialized;
  const d = defaultsFor(manifest, op, { trailingSlash: materialized.trailingSlash });
  const vars = { token, unwrittenToken, slug: op.article ?? "" };
  const predicate = substitute(op.predicate ?? d.predicate, vars);
  const supplemental = substitute(op.supplemental ?? d.supplemental, vars);
  const targetPath = op.targetPath ?? d.targetPath;
  const method = resolveWriteMethod(op);
  const base = { token, unwrittenToken, targetPath, predicate, supplemental };
  const articlePath = (slug) => join(materialized.contentDir, `${slug}.md`);
  if (method === "unlink") return { ...base, write: { method, path: articlePath(op.article) } };
  if (method === "rename")
    return {
      ...base,
      write: { method, path: articlePath(op.article), to: articlePath(renamedSlug(op)) },
    };
  if (method === "create") {
    const slug = op.article;
    const bytes = `---\ntitle: Article ${slug.toUpperCase()} ${token}\ncategory: created\n---\n\nBody marker ${token} for article ${slug}.\n`;
    return { ...base, write: { method, path: articlePath(slug), bytes: Buffer.from(bytes) } };
  }
  const path = op.op === "edit-css" ? materialized.cssPath : materialized.articles[op.article];
  if (!path) throw new Error(`op ${op.op} needs a known article`);
  const next = TRANSFORMS[op.op](readFileSync(path, "utf8"), token, op.article ?? "");
  return { ...base, write: { method, path, bytes: Buffer.from(next) } };
}

function inode(path) {
  try {
    return statSync(path).ino;
  } catch {
    return null;
  }
}

/** A true in-place truncate-write: O_WRONLY|O_TRUNC on the existing inode, never O_CREAT. */
function writeInPlace(path, bytes, onT0, fsImpl) {
  const t0 = onT0();
  const fd = fsImpl.openSync(path, constants.O_WRONLY | constants.O_TRUNC);
  try {
    fsImpl.writeSync(fd, bytes);
  } finally {
    fsImpl.closeSync(fd);
  }
  return t0;
}

export const REAL_FS = { openSync, writeSync, closeSync, renameSync, unlinkSync, writeFileSync };

/**
 * Perform the real filesystem operation with the declared method. `onT0` is
 * invoked immediately before the timed syscall. For atomic-rename the temp
 * sibling is written first (untimed) in the target's own directory, exactly
 * like an editor's atomic save. Evidence (inodes, temp path) is attached to
 * `write.evidence`.
 */
export function performWrite(write, onT0, fsImpl = REAL_FS) {
  const ev = { method: write.method, inodeBefore: inode(write.path) };
  let t0;
  switch (write.method) {
    case "in-place":
      if (ev.inodeBefore === null) throw new Error(`in-place target missing: ${write.path}`);
      t0 = writeInPlace(write.path, write.bytes, onT0, fsImpl);
      break;
    case "atomic-rename": {
      if (ev.inodeBefore === null) throw new Error(`atomic-rename target missing: ${write.path}`);
      const tmp = join(dirname(write.path), `.${basename(write.path)}.dbo-${randomHex(4)}.tmp`);
      ev.tempSibling = tmp;
      fsImpl.writeFileSync(tmp, write.bytes, { flag: "wx" });
      t0 = onT0();
      fsImpl.renameSync(tmp, write.path);
      break;
    }
    case "create": {
      if (ev.inodeBefore !== null) throw new Error(`create target exists: ${write.path}`);
      t0 = onT0();
      const fd = fsImpl.openSync(
        write.path,
        constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
      );
      try {
        fsImpl.writeSync(fd, write.bytes);
      } finally {
        fsImpl.closeSync(fd);
      }
      break;
    }
    case "unlink":
      if (ev.inodeBefore === null) throw new Error(`unlink target missing: ${write.path}`);
      t0 = onT0();
      fsImpl.unlinkSync(write.path);
      break;
    case "rename":
      if (ev.inodeBefore === null) throw new Error(`rename source missing: ${write.path}`);
      if (write.caseOnly) {
        if (write.path === write.to || write.path.toLowerCase() !== write.to.toLowerCase())
          throw new Error(
            `case-only rename needs a case variant of ${write.path} (got ${write.to})`,
          );
        // On a case-insensitive volume the destination name resolves to the source itself.
        const destIno = inode(write.to);
        if (destIno !== null && destIno !== ev.inodeBefore)
          throw new Error(`rename destination exists as a different file: ${write.to}`);
        ev.caseOnly = true;
        ev.destinationResolvedToSource = destIno !== null;
      } else if (existsSync(write.to)) throw new Error(`rename destination exists: ${write.to}`);
      if (write.verifyEntries) ev.readdirBefore = dirEntries(write.verifyEntries.dir);
      t0 = onT0();
      fsImpl.renameSync(write.path, write.to);
      ev.to = write.to;
      ev.inodeAfterTo = inode(write.to);
      if (write.verifyEntries) {
        const v = write.verifyEntries;
        ev.readdirAfter = dirEntries(v.dir);
        const missing = (v.present ?? []).filter((n) => !ev.readdirAfter.includes(n));
        const stillThere = (v.absent ?? []).filter((n) => ev.readdirAfter.includes(n));
        ev.entryCheck = { present: v.present ?? [], absent: v.absent ?? [], missing, stillThere };
        if (missing.length || stillThere.length) {
          ev.inodeAfter = inode(write.path);
          write.evidence = ev;
          const e = new Error(
            `rename-noop: directory entries after the rename do not show it (missing ${JSON.stringify(missing)}, still present ${JSON.stringify(stillThere)})`,
          );
          e.code = RENAME_NOOP;
          e.evidence = ev;
          throw e;
        }
      }
      break;
    default:
      throw new Error(`unknown write method ${write.method}`);
  }
  ev.inodeAfter = inode(write.path);
  write.evidence = ev;
  return t0;
}

export const RENAME_NOOP = "rename-noop";

/** Exact directory entry names (sorted), as readdir returns them. */
export function dirEntries(dir) {
  return readdirSync(dir).sort();
}

/**
 * Probe the volume's case sensitivity on `dir` at run time: create a
 * lower-case probe file, stat its upper-case variant, compare inodes, then
 * remove the probe. Nothing is assumed from the platform.
 */
export function probeCaseSensitivity(dir) {
  const name = `.dbo-case-probe-${randomHex(4)}`;
  const probe = join(dir, name);
  const variantName = name.toUpperCase();
  writeFileSync(probe, "", { flag: "wx" });
  try {
    const a = statSync(probe);
    let v = null;
    try {
      v = statSync(join(dir, variantName));
    } catch {
      v = null;
    }
    const sameInode = v !== null && v.ino === a.ino && v.dev === a.dev;
    return {
      method: "create lower-case probe; stat upper-case variant; compare dev+inode; unlink probe",
      dir,
      probe: name,
      variant: variantName,
      variantFound: v !== null,
      sameInode,
      caseSensitive: !sameInode,
      dev: a.dev,
    };
  } finally {
    unlinkSync(probe);
  }
}

export function defaultT0() {
  return hrtimeNs();
}

/** Exact bytes + sha256 of every file in `dirs` plus the listed `files`. */
export function snapshotPaths({ dirs = [], files = [] }) {
  const out = {};
  for (const dir of dirs) for (const f of walkFiles(dir)) out[f.abs] = readFileSync(f.abs);
  for (const path of files) out[path] = readFileSync(path);
  return {
    dirs: [...dirs],
    files: Object.fromEntries(
      Object.entries(out).map(([p, b]) => [p, { bytes: b, sha256: sha256(b) }]),
    ),
  };
}

export function snapshotSources(materialized) {
  const files = {};
  for (const f of walkFiles(materialized.contentDir)) files[f.abs] = readFileSync(f.abs);
  files[materialized.cssPath] = readFileSync(materialized.cssPath);
  return {
    dirs: [materialized.contentDir],
    files: Object.fromEntries(
      Object.entries(files).map(([p, b]) => [p, { bytes: b, sha256: sha256(b) }]),
    ),
  };
}

export function verifySnapshot(snapshot) {
  const mismatches = [];
  const entries = new Map();
  const namesIn = (dir) => {
    if (!entries.has(dir)) entries.set(dir, existsSync(dir) ? readdirSync(dir) : []);
    return entries.get(dir);
  };
  for (const [path, { sha256: want }] of Object.entries(snapshot.files)) {
    const got = existsSync(path) ? sha256(readFileSync(path)) : null;
    if (got !== want) mismatches.push({ path, want, got });
    // A case-insensitive volume resolves a case variant to the same file: compare the exact entry name too.
    else if (got !== null && !namesIn(dirname(path)).includes(basename(path)))
      mismatches.push({
        path,
        want: basename(path),
        got: "name-case-mismatch",
        entries: namesIn(dirname(path)).filter(
          (n) => n.toLowerCase() === basename(path).toLowerCase(),
        ),
      });
  }
  for (const dir of snapshot.dirs) {
    for (const f of walkFiles(dir))
      if (!(f.abs in snapshot.files))
        mismatches.push({ path: f.abs, want: null, got: "unexpected-file" });
  }
  return mismatches;
}

/**
 * Revert case-only renames before anything else: an entry whose name differs
 * from a snapshot file's name only by case (and whose exact name is gone) is
 * renamed back. Without this a case-insensitive volume would report the
 * snapshot file as present and the variant as an extra file, and the unlink
 * pass would delete the article itself. A direct rename that leaves the entry
 * name unchanged is retried through a temp sibling; both steps are recorded.
 */
export function revertCaseRenames(snapshot, fsImpl = REAL_FS) {
  const actions = [];
  const byDir = new Map();
  for (const p of Object.keys(snapshot.files)) {
    const d = dirname(p);
    if (!byDir.has(d)) byDir.set(d, []);
    byDir.get(d).push(basename(p));
  }
  for (const [dir, wanted] of byDir) {
    if (!existsSync(dir)) continue;
    const names = readdirSync(dir);
    for (const want of wanted) {
      if (names.includes(want)) continue;
      const variant = names.find(
        (n) => n !== want && !wanted.includes(n) && n.toLowerCase() === want.toLowerCase(),
      );
      if (!variant) continue;
      const verifyEntries = { dir, present: [want], absent: [variant] };
      const direct = {
        method: "rename",
        path: join(dir, variant),
        to: join(dir, want),
        caseOnly: true,
        verifyEntries,
      };
      try {
        performWrite(direct, () => 0n, fsImpl);
        actions.push({
          path: direct.to,
          from: direct.path,
          method: "rename",
          caseOnly: true,
          evidence: direct.evidence,
        });
        continue;
      } catch (error) {
        actions.push({
          path: direct.to,
          from: direct.path,
          method: "rename",
          caseOnly: true,
          error: String(error?.message ?? error),
          evidence: error?.evidence ?? direct.evidence ?? null,
        });
        if (error?.code !== RENAME_NOOP) continue;
      }
      const tmp = join(dir, `.${want}.dbo-case-${randomHex(4)}.tmp`);
      const steps = [
        { method: "rename", path: join(dir, variant), to: tmp },
        { method: "rename", path: tmp, to: join(dir, want), verifyEntries },
      ];
      for (const step of steps) {
        try {
          performWrite(step, () => 0n, fsImpl);
          actions.push({
            path: step.to,
            from: step.path,
            method: "rename",
            caseOnly: true,
            viaTemp: true,
            evidence: step.evidence,
          });
        } catch (error) {
          actions.push({
            path: step.to,
            from: step.path,
            method: "rename",
            caseOnly: true,
            viaTemp: true,
            error: String(error?.message ?? error),
          });
          break;
        }
      }
    }
  }
  return actions;
}

/**
 * Restore the snapshot with an explicit method: changed files use
 * `writeMethod` (in-place or atomic-rename), missing files are recreated with
 * `create`, unexpected files in snapshot dirs are removed with `unlink`.
 * Every action is returned with its method, then hashes are re-verified.
 */
export function restoreSnapshot(
  snapshot,
  { writeMethod = "in-place", fsImpl = REAL_FS, renameRevert = false } = {},
) {
  if (!RESTORATION_WRITE_METHODS.includes(writeMethod))
    throw new Error(`restoration writeMethod ${writeMethod} not allowed`);
  const actions = [];
  const untimed = () => 0n;
  if (renameRevert) actions.push(...revertCaseRenames(snapshot, fsImpl));
  if (renameRevert) {
    // A renamed file is reverted with a rename (then fixed in place if its bytes differ):
    // pair each missing snapshot file with an extra file in the same dir, by equal
    // content first, otherwise when exactly one of each is left in that dir.
    const missing = Object.keys(snapshot.files).filter((p) => !existsSync(p));
    const extras = snapshot.dirs
      .flatMap((d) => walkFiles(d).map((f) => f.abs))
      .filter((p) => !(p in snapshot.files));
    const pairs = [];
    for (const m of missing) {
      const same = extras.find(
        (e) =>
          dirname(e) === dirname(m) &&
          !pairs.some((x) => x.from === e) &&
          sha256(readFileSync(e)) === snapshot.files[m].sha256,
      );
      if (same) pairs.push({ from: same, to: m });
    }
    for (const dir of new Set(missing.map((m) => dirname(m)))) {
      const leftM = missing.filter((m) => dirname(m) === dir && !pairs.some((x) => x.to === m));
      const leftE = extras.filter((e) => dirname(e) === dir && !pairs.some((x) => x.from === e));
      if (leftM.length === 1 && leftE.length === 1) pairs.push({ from: leftE[0], to: leftM[0] });
    }
    for (const pair of pairs) {
      const write = { method: "rename", path: pair.from, to: pair.to };
      try {
        performWrite(write, untimed, fsImpl);
        actions.push({
          path: pair.to,
          from: pair.from,
          method: "rename",
          evidence: write.evidence,
        });
      } catch (error) {
        actions.push({
          path: pair.to,
          from: pair.from,
          method: "rename",
          error: String(error?.message ?? error),
        });
      }
    }
  }
  for (const [path, { bytes, sha256: want }] of Object.entries(snapshot.files)) {
    const exists = existsSync(path);
    const got = exists ? sha256(readFileSync(path)) : null;
    if (got === want) continue;
    const write = { method: exists ? writeMethod : "create", path, bytes };
    try {
      performWrite(write, untimed, fsImpl);
      actions.push({ path, method: write.method, evidence: write.evidence });
    } catch (error) {
      actions.push({ path, method: write.method, error: String(error?.message ?? error) });
    }
  }
  for (const dir of snapshot.dirs) {
    for (const f of walkFiles(dir)) {
      if (f.abs in snapshot.files) continue;
      const write = { method: "unlink", path: f.abs };
      try {
        performWrite(write, untimed, fsImpl);
        actions.push({ path: f.abs, method: "unlink", evidence: write.evidence });
      } catch (error) {
        actions.push({ path: f.abs, method: "unlink", error: String(error?.message ?? error) });
      }
    }
  }
  return {
    writeMethod,
    restored: actions.map((a) => a.path),
    actions,
    mismatches: verifySnapshot(snapshot),
  };
}

/** Live predicates that hold for the original snapshot (the article titles + css marker). */
export function originalLiveChecks(materialized, snapshot) {
  const { manifest } = materialized;
  const checks = [];
  for (const slug of manifest.articles) {
    const p = materialized.articles[slug];
    const text = snapshot.files[p]?.bytes.toString("utf8") ?? "";
    const title = text.match(/^title: (.*)$/m)?.[1];
    if (title) {
      checks.push({
        path: routePath(manifest, "article", { slug, trailingSlash: materialized.trailingSlash }),
        predicate: { kind: "text", selector: sel(manifest, "articleTitle", slug), equals: title },
      });
    }
  }
  const articleCount = Object.keys(snapshot.files).filter(
    (p) => p.startsWith(`${materialized.contentDir}/`) && p.endsWith(".md"),
  ).length;
  checks.push({
    path: routePath(manifest, "home", { trailingSlash: materialized.trailingSlash }),
    predicate: { kind: "count", selector: "[data-home-item]", equals: articleCount },
  });
  const css = snapshot.files[materialized.cssPath]?.bytes.toString("utf8") ?? "";
  const marker = css.match(/--observe-css-marker: ([^;]*);/)?.[1];
  if (marker) {
    checks.push({
      path: routePath(manifest, "independent", { trailingSlash: materialized.trailingSlash }),
      predicate: {
        kind: "computed-style",
        selector: ":root",
        property: manifest.css.customProperty,
        equals: marker,
      },
    });
  }
  return checks;
}

/** Controller-driven, unmeasured warm-up navigations for a scenario's warmOrder. */
export function warmPaths(materialized, scenario) {
  const home = routePath(materialized.manifest, "home", {
    trailingSlash: materialized.trailingSlash,
  });
  const detail = routePath(materialized.manifest, "article", {
    slug: scenario.warmDetail ?? "a",
    trailingSlash: materialized.trailingSlash,
  });
  if (scenario.warmOrder === "index-first") return [home, detail];
  if (scenario.warmOrder === "detail-first") return [detail, home];
  return [];
}

/** sha256 of a file, or null when it does not exist. */
export function hashOf(path) {
  return existsSync(path) ? sha256(readFileSync(path)) : null;
}

/** A sibling path that does not exist (the write-error injection target). */
export function missingSibling(path) {
  for (;;) {
    const p = join(dirname(path), `.${basename(path)}.dbo-missing-${randomHex(4)}`);
    if (!existsSync(p)) return p;
  }
}

/** restore-blocked injection: make the file read-only so the restoration write fails. */
export function blockWrite(path) {
  const modeBefore = statSync(path).mode & 0o7777;
  chmodSync(path, 0o444);
  return { path, modeBefore, modeApplied: 0o444 };
}

export function unblockWrite(block) {
  chmodSync(block.path, block.modeBefore);
  return { path: block.path, modeRestored: statSync(block.path).mode & 0o7777 };
}

/**
 * Multi-step workload write. Each step uses its declared method; step i waits
 * its requested `afterMs` (spacing) before running. Returns the first step's
 * t0, the last step's start, and every step's actual timestamp. A failing
 * step throws with `.steps` (the completed ones) attached.
 */
export async function performWrites(
  steps,
  now,
  fsImpl = REAL_FS,
  sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
) {
  if (!Array.isArray(steps) || steps.length === 0) throw noWriteStepsError();
  const done = [];
  let t0 = null;
  for (const [i, step] of steps.entries()) {
    if (i > 0 && step.afterMs) await sleep(step.afterMs);
    try {
      const t = performWrite(step, now, fsImpl);
      if (t0 === null) t0 = t;
      done.push({
        i,
        method: step.method,
        path: step.path,
        to: step.to ?? null,
        requestedAfterMs: step.afterMs ?? 0,
        tNs: String(t),
        evidence: step.evidence ?? null,
      });
    } catch (error) {
      error.steps = done;
      error.failedStep = i;
      error.failedStepEvidence = step.evidence ?? null;
      throw error;
    }
  }
  const lastNs = BigInt(done[done.length - 1].tNs);
  return { t0, tLast: lastNs, steps: done };
}
