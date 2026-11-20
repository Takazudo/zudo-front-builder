import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { hrtimeNs, randomHex, readJson, sha256, treeHashDir, walkFiles } from "./util.mjs";

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

export function zfbConfigFor(manifest, placement) {
  const p = manifest.content.placements[placement];
  if (!p) throw new Error(`unknown placement ${placement}`);
  const collection = { name: manifest.content.collection, path: p.configPath };
  if (p.allowOutsideRoot) collection.allowOutsideRoot = true;
  return { ...manifest.config, collections: [collection] };
}

/** Copy the fixture into a fresh work dir for one run; the source fixture is never edited. */
export function materialize({ fixtureDir, workDir, placement, adapter }) {
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
  const config = `${JSON.stringify(zfbConfigFor(manifest, placement), null, 2)}\n`;
  writeFileSync(join(appDir, "zfb.config.json"), config);
  const scratch = join(workDir, ".dbo-tmp");
  mkdirSync(scratch);
  const articles = Object.fromEntries(
    manifest.articles.map((slug) => [slug, join(contentDir, `${slug}.md`)]),
  );
  return {
    manifest,
    workDir,
    appDir,
    contentDir,
    scratchDir: scratch,
    cssPath: join(workDir, manifest.css.managed),
    articles,
    configSha256: sha256(config),
    adapterSha256: sha256(readFileSync(adapterTarget)),
    materializedTreeSha256: treeHashDir(workDir, {
      filter: (_r, _k, name) => name !== ".dbo-tmp" && name !== ".zfb" && name !== "node_modules",
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

function sel(manifest, key, slug) {
  return manifest.selectors[key].replaceAll("{slug}", slug);
}

/** Default timed predicate and supplemental checks for an op, before token substitution. */
export function defaultsFor(manifest, op) {
  const slug = op.article;
  const article = slug ? manifest.routes.article.replace("{slug}", slug) : null;
  switch (op.op) {
    case "edit-title":
      return {
        targetPath: article,
        predicate: {
          kind: "text",
          selector: sel(manifest, "articleTitle", slug),
          includes: "{token}",
        },
        supplemental: [
          {
            path: "/",
            predicate: {
              kind: "text",
              selector: sel(manifest, "homeTitle", slug),
              includes: "{token}",
            },
          },
        ],
      };
    case "edit-category":
      return {
        targetPath: article,
        predicate: {
          kind: "text",
          selector: sel(manifest, "articleCategory", slug),
          includes: "{token}",
        },
        supplemental: [
          {
            path: "/",
            predicate: {
              kind: "text",
              selector: sel(manifest, "homeCategory", slug),
              includes: "{token}",
            },
          },
        ],
      };
    case "edit-body":
      return {
        targetPath: article,
        predicate: {
          kind: "text",
          selector: sel(manifest, "articleBody", slug),
          includes: "{token}",
        },
        supplemental: [],
      };
    case "stale-predicate":
      return {
        targetPath: article,
        predicate: {
          kind: "text",
          selector: sel(manifest, "articleBody", slug),
          includes: "{unwrittenToken}",
        },
        supplemental: [],
      };
    case "edit-css":
      return {
        targetPath: op.targetPath ?? manifest.routes.independent,
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
        targetPath: "/",
        predicate: { kind: "absent", selector: sel(manifest, "homeItem", slug) },
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
  const d = defaultsFor(manifest, op);
  const vars = { token, unwrittenToken, slug: op.article ?? "" };
  const predicate = substitute(op.predicate ?? d.predicate, vars);
  const supplemental = substitute(op.supplemental ?? d.supplemental, vars);
  const targetPath = op.targetPath ?? d.targetPath;
  if (op.op === "delete-article") {
    return {
      token,
      unwrittenToken,
      targetPath,
      predicate,
      supplemental,
      write: { kind: "unlink", path: materialized.articles[op.article] },
    };
  }
  const path = op.op === "edit-css" ? materialized.cssPath : materialized.articles[op.article];
  if (!path) throw new Error(`op ${op.op} needs a known article`);
  const next = TRANSFORMS[op.op](readFileSync(path, "utf8"), token, op.article ?? "");
  const kind = op.writeMode === "rename" ? "rename" : "writeFile";
  return {
    token,
    unwrittenToken,
    targetPath,
    predicate,
    supplemental,
    write: { kind, path, bytes: Buffer.from(next), tmpDir: materialized.scratchDir },
  };
}

/**
 * Perform the real filesystem operation. `onT0` is invoked immediately before
 * the syscall (for rename, the temp file is written first and is not timed).
 */
export function performWrite(write, onT0, fsImpl = { writeFileSync, renameSync, unlinkSync }) {
  if (write.kind === "writeFile") {
    const t0 = onT0();
    fsImpl.writeFileSync(write.path, write.bytes);
    return t0;
  }
  if (write.kind === "rename") {
    const tmp = join(write.tmpDir, `${randomHex(6)}.tmp`);
    fsImpl.writeFileSync(tmp, write.bytes);
    const t0 = onT0();
    fsImpl.renameSync(tmp, write.path);
    return t0;
  }
  if (write.kind === "unlink") {
    const t0 = onT0();
    fsImpl.unlinkSync(write.path);
    return t0;
  }
  throw new Error(`unknown write kind ${write.kind}`);
}

export function defaultT0() {
  return hrtimeNs();
}

/** Exact bytes + sha256 of every workload-reachable file. */
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
  for (const [path, { sha256: want }] of Object.entries(snapshot.files)) {
    const got = existsSync(path) ? sha256(readFileSync(path)) : null;
    if (got !== want) mismatches.push({ path, want, got });
  }
  for (const dir of snapshot.dirs) {
    for (const f of walkFiles(dir))
      if (!(f.abs in snapshot.files))
        mismatches.push({ path: f.abs, want: null, got: "unexpected-file" });
  }
  return mismatches;
}

/** Restore bytes for changed/missing files, then verify. Never deletes unexpected files. */
export function restoreSnapshot(snapshot, fsImpl = { writeFileSync }) {
  const restored = [];
  for (const [path, { bytes, sha256: want }] of Object.entries(snapshot.files)) {
    const got = existsSync(path) ? sha256(readFileSync(path)) : null;
    if (got !== want) {
      fsImpl.writeFileSync(path, bytes);
      restored.push(path);
    }
  }
  return { restored, mismatches: verifySnapshot(snapshot) };
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
        path: manifest.routes.article.replace("{slug}", slug),
        predicate: { kind: "text", selector: sel(manifest, "articleTitle", slug), equals: title },
      });
    }
  }
  const css = snapshot.files[materialized.cssPath]?.bytes.toString("utf8") ?? "";
  const marker = css.match(/--observe-css-marker: ([^;]*);/)?.[1];
  if (marker) {
    checks.push({
      path: manifest.routes.independent,
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
