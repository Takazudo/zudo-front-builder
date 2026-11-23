import {
  chmodSync,
  cpSync,
  lstatSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { materializeFieldNotes, sourceTreeHashes } from "../lib/fieldnotes.mjs";
import { blockWrite, materialize, performWrite, unblockWrite } from "../lib/fixture.mjs";
import { fixtureTreeHash } from "../lib/identity.mjs";
import { HARNESS_DIR, ensureOwnerWritable } from "../lib/util.mjs";
import { makeFakeSource } from "./fieldnotes-fake.mjs";

const TINY = join(HARNESS_DIR, "fixtures", "tiny-two-article");
const FIELD_NOTES = join(HARNESS_DIR, "fixtures", "field-notes");
const tmps = [];
const tmp = () => {
  const d = mkdtempSync(join(tmpdir(), "dbo-ro-"));
  tmps.push(d);
  return d;
};
afterEach(() => {
  while (tmps.length) {
    const d = tmps.pop();
    ensureOwnerWritable(d);
    rmSync(d, { recursive: true, force: true });
  }
});

/** Make a tree read-only the way `freeze` does: files a-w, dirs r-x only. */
function makeReadOnly(dir) {
  const st = lstatSync(dir);
  if (st.isSymbolicLink()) return;
  if (st.isDirectory()) {
    for (const name of readdirSync(dir)) makeReadOnly(join(dir, name));
    chmodSync(dir, 0o555);
  } else chmodSync(dir, st.mode & 0o555);
}

/** Every file and dir under `dir` with its mode, for before/after comparisons. */
function modes(dir) {
  const out = {};
  const visit = (p) => {
    const st = lstatSync(p);
    out[p] = st.mode & 0o7777;
    if (st.isDirectory()) for (const name of readdirSync(p)) visit(join(p, name));
  };
  visit(dir);
  return out;
}

function unwritable(dir) {
  return Object.entries(modes(dir))
    .filter(
      ([p, m]) =>
        (m & (statSync(p).isDirectory() ? 0o700 : 0o600)) !==
        (statSync(p).isDirectory() ? 0o700 : 0o600),
    )
    .map(([p]) => p);
}

describe("materializing from a read-only (frozen) source gives a writable work copy", () => {
  it("tiny: the copy is owner-writable, the source keeps its modes and hash, and edits work", () => {
    const root = tmp();
    const fixture = join(root, "fixture");
    cpSync(TINY, fixture, { recursive: true });
    const hashBefore = fixtureTreeHash(fixture);
    makeReadOnly(fixture);
    expect(fixtureTreeHash(fixture)).toBe(hashBefore);
    const sourceModes = modes(fixture);
    for (const placement of ["internal", "external"]) {
      const m = materialize({
        fixtureDir: fixture,
        workDir: join(root, `work-${placement}`),
        placement,
        adapter: "A0",
      });
      expect(unwritable(m.workDir), placement).toEqual([]);
      const target = join(m.contentDir, "a.md");
      performWrite({ method: "in-place", path: target, bytes: Buffer.from("edited\n") }, () => 1n);
      expect(readFileSync(target, "utf8")).toBe("edited\n");
    }
    expect(modes(fixture)).toEqual(sourceModes);
  });

  it("the restore-blocked control still applies 0444 and reverts to the work file's mode", () => {
    const root = tmp();
    const fixture = join(root, "fixture");
    cpSync(TINY, fixture, { recursive: true });
    makeReadOnly(fixture);
    const m = materialize({
      fixtureDir: fixture,
      workDir: join(root, "w"),
      placement: "internal",
      adapter: "A0",
    });
    const target = join(m.contentDir, "a.md");
    const before = statSync(target).mode & 0o7777;
    const block = blockWrite(target);
    expect(block).toMatchObject({ modeBefore: before, modeApplied: 0o444 });
    expect(statSync(target).mode & 0o7777).toBe(0o444);
    expect(() =>
      performWrite({ method: "in-place", path: target, bytes: Buffer.from("x") }, () => 1n),
    ).toThrow(/EACCES/);
    expect(unblockWrite(block).modeRestored).toBe(before);
  });

  it("field-notes: read-only fixture and source, external and internal placement, managed CSS", () => {
    const root = tmp();
    const fixture = join(root, "fixture");
    cpSync(FIELD_NOTES, fixture, { recursive: true });
    const src = join(root, "src");
    const fake = makeFakeSource(src, 20);
    // The internal placement checks the real base config; it is the overlay with the original
    // external collection path.
    const overlay = readFileSync(
      join(FIELD_NOTES, "overlays", "internal-placement", "zfb.config.ts.overlay"),
      "utf8",
    );
    writeFileSync(
      join(src, "apps", "zfb", "zfb.config.ts"),
      overlay.replace(
        '      path: "content/articles",\n',
        '      path: "../../content/articles",\n      allowOutsideRoot: true,\n',
      ),
    );
    const pinsBefore = sourceTreeHashes(
      src,
      JSON.parse(readFileSync(join(fixture, "manifest.json"), "utf8")),
    );
    makeReadOnly(fixture);
    makeReadOnly(src);
    const sourceModes = { ...modes(fixture), ...modes(src) };
    for (const placement of ["external", "internal"]) {
      const m = materializeFieldNotes({
        fixtureDir: fixture,
        sourceDir: src,
        workDir: join(root, `work-${placement}`),
        adapter: "original-memoized",
        managedCss: true,
        placement,
        expectedDatasetSha256: fake.datasetSha256,
      });
      expect(unwritable(m.workDir), placement).toEqual([]);
      const article = join(m.contentDir, "article-00005.md");
      performWrite({ method: "in-place", path: article, bytes: Buffer.from("x\n") }, () => 1n);
      performWrite({ method: "in-place", path: m.layoutPath, bytes: Buffer.from("x\n") }, () => 1n);
    }
    expect({ ...modes(fixture), ...modes(src) }).toEqual(sourceModes);
    expect(
      sourceTreeHashes(src, JSON.parse(readFileSync(join(fixture, "manifest.json"), "utf8"))),
    ).toEqual(pinsBefore);
  });
});
