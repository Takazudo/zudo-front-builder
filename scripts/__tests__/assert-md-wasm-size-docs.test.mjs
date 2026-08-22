import { describe, expect, it } from "vitest";

import {
  DOC_FILES,
  validateAllowanceScopes,
  validateCeilingTable,
  validateClosure,
  validateEntryTables,
  validateHighlightRootPair,
  fixMdWasmSizeDocs,
  fixTableProse,
  validateMdWasmSizeDocs,
  validateShippedTables,
  validateTableProse,
} from "../assert-md-wasm-size-docs.mjs";

const manifest = {
  measuredOnVersion: "2.8.0",
  measured: {
    root: { finalWasm: 3_274_064, gzip9: 1_458_444, glue: 14_998, glueGzip9: 4_199 },
    highlight: { finalWasm: 1_476_740, gzip9: 758_244, glue: 8_758, glueGzip9: 2_637 },
    render: { finalWasm: 2_083_465, gzip9: 1_011_160, glue: 8_772, glueGzip9: 2_661 },
    parse: { finalWasm: 624_976, gzip9: 276_437, glue: 11_159, glueGzip9: 3_797 },
  },
  ceilings: {
    root: 1_600_000,
    highlight: 820_000,
    render: 1_100_000,
    parse: 325_000,
    tarball: 3_900_000,
  },
};
const ceilings = { ...manifest.ceilings };
const artifacts = ["root", "highlight", "render", "parse"];
const columns = ["finalWasm", "gzip9", "glue", "glueGzip9"];

const bytes = (value) => `${value.toLocaleString("en-US")} B`;

function shippedTable({ ja = false } = {}) {
  return `| Entry/graph | final wasm | gzip-9 | glue | glue gzip-9 |
| --- | ---: | ---: | ---: | ---: |
${artifacts
  .map((artifact) => {
    const label = artifact === "root" ? (ja ? "root（full）" : "root (full)") : artifact;
    return `| ${label} | ${columns.map((column) => bytes(manifest.measured[artifact][column])).join(" | ")} |`;
  })
  .join("\n")}`;
}

function entryTable({ ja = false, version = manifest.measuredOnVersion } = {}) {
  const open = ja ? "（" : "(";
  const close = ja ? "）" : ")";
  return `| Entry | gzip-9 wasm ${open}${version}${close} | API |
| --- | ---: | --- |
${artifacts.map((artifact) => `| \`${artifact === "root" ? "." : `./${artifact}`}\` | ${bytes(manifest.measured[artifact].gzip9)} | x |`).join("\n")}`;
}

const enProse = `Locked gzip-9 ceilings are root 1,600,000 B, highlight 820,000 B, render
1,100,000 B, and parse 325,000 B; the complete packed tarball ceiling is
3,900,000 B. All four ship inside their ceilings, with 141,556 B (root),
61,756 B (highlight), 88,840 B (render), and 48,563 B (parse) of headroom.
The highlight artifact is 1,797,324 B smaller raw and 700,200 B smaller gzip-9,
landing at about 45% of root's raw bytes and 52% of its gzipped bytes.`;

const jaProse = `固定された gzip-9 の上限は root 1,600,000 B、highlight 820,000 B、
render 1,100,000 B、parse 325,000 B、完全な packed tarball は 3,900,000 B です。
上限までの余裕は root 141,556 B、highlight 61,756 B、render 88,840 B、parse 48,563 B です。
raw で 1,797,324 B、gzip-9 で 700,200 B 小さく、highlight は root の
raw バイトの約 45%、gzip 後のバイトの約 52% に収まります。`;

function correctFiles() {
  return Object.fromEntries(
    DOC_FILES.map((file, index) => {
      const ja = file.includes("docs-ja/");
      if (index >= 6) {
        const pair = ja
          ? `${manifest.measuredOnVersion} では gzip-9 で 758,244 B、ルートは 1,458,444 B です。`
          : "the download of the root entry: 758,244 B gzip-9 versus 1,458,444 B in 2.8.0.";
        return [file, pair];
      }
      const count = index >= 4 ? 2 : 1;
      const versionProse = ja
        ? `出荷された **${manifest.measuredOnVersion}** のアーティファクトの行です。\nこれらは永続的な保証ではなく ${manifest.measuredOnVersion} の測定値です。`
        : `These are the shipped **${manifest.measuredOnVersion}** artifact rows.\nThese are ${manifest.measuredOnVersion} measurements, not permanent promises.`;
      const sections = Array.from(
        { length: count },
        () => `${versionProse}\n${shippedTable({ ja })}\n${ja ? jaProse : enProse}`,
      ).join("\n\n");
      const pair =
        index === 1 ? "payload shows it: 758,244 B gzip-9 versus 1,458,444 B for root." : "";
      return [file, `${entryTable({ ja })}\n\n${sections}\n${pair}`];
    }),
  );
}

function staleTableFiles() {
  let staleValue = 9_000_001;
  return Object.fromEntries(
    Object.entries(correctFiles()).map(([file, content]) => {
      let stale = content.replace(
        /^\| (?:root(?: \(full\)|（full）)|highlight|render|parse) \|.*\|$/gm,
        (line) => {
          const values = line.slice(1, -1).split("|");
          for (let index = 1; index <= 4; index += 1) {
            values[index] = ` ${bytes(staleValue)}`;
            staleValue += 1;
          }
          return `|${values.join("|")}|`;
        },
      );
      stale = stale.replaceAll(manifest.measuredOnVersion, "2.7.9");
      return [file, stale];
    }),
  );
}

describe("md-wasm documentation validator", () => {
  it("accepts a correct injected fixture set", () => {
    expect(validateMdWasmSizeDocs({ files: correctFiles(), manifest, ceilings })).toEqual([]);
  });

  it.each(artifacts.flatMap((artifact) => columns.map((column) => [artifact, column])))(
    "reports a stale shipped-table %s %s cell",
    (artifact, column) => {
      const original = bytes(manifest.measured[artifact][column]);
      const content = shippedTable().replace(original, "9,999,999 B");
      const findings = validateShippedTables(DOC_FILES[0], content, manifest, 1);
      expect(findings.map(({ message }) => message).join("\n")).toContain(
        `table 1, ${artifact} ${column}`,
      );
    },
  );

  it.each(artifacts)("reports a stale entry-table %s cell", (artifact) => {
    const content = entryTable().replace(bytes(manifest.measured[artifact].gzip9), "9,999,999 B");
    const findings = validateEntryTables(DOC_FILES[0], content, manifest, 1);
    expect(findings.map(({ message }) => message).join("\n")).toContain(
      `table 1, ${artifact} gzip9`,
    );
  });

  it.each([
    ["shipped", shippedTable(), validateShippedTables, "shipped-artifact-row-count"],
    ["entry", entryTable(), validateEntryTables, "entry-artifact-row-count"],
  ])("rejects a duplicate artifact row in a %s table", (_label, content, validate, code) => {
    const rootRow = content.split("\n")[2];
    expect(validate(DOC_FILES[0], `${content}\n${rootRow}`, manifest, 1)).toContainEqual(
      expect.objectContaining({ code, artifact: "root", found: 2 }),
    );
  });

  it.each([false, true])("reports a wrong entry version with fullwidth=%s", (ja) => {
    const findings = validateEntryTables(
      DOC_FILES[ja ? 3 : 0],
      entryTable({ ja, version: "2.7.9" }),
      manifest,
      1,
    );
    expect(findings).toEqual([
      expect.objectContaining({ code: "entry-table-version", version: "2.7.9" }),
    ]);
  });

  it("rejects a semantically wrong but numerically valid highlight/root swap", () => {
    const content = "the download of the root entry: 758,244 B gzip-9 versus 758,244 B in 2.8.0.";
    expect(validateClosure(DOC_FILES[6], content, manifest, ceilings)).toEqual([]);
    expect(validateHighlightRootPair(DOC_FILES[6], content, manifest, 1)).toEqual([
      expect.objectContaining({ code: "stale-highlight-root-pair" }),
    ]);
  });

  it("fails when a prose anchor disappears", () => {
    const content = enProse.replace("of headroom", "remaining");
    expect(validateTableProse(DOC_FILES[0], content, manifest, ceilings, 1)).toContainEqual(
      expect.objectContaining({
        code: "prose-anchor-count",
        field: "headroom per artifact",
        found: 0,
      }),
    );
  });

  it("rejects an unregistered byte literal", () => {
    expect(validateClosure(DOC_FILES[0], "unexpected 9,999,999 B", manifest, ceilings)).toEqual([
      expect.objectContaining({
        code: "unregistered-byte-literal",
        line: 1,
        literal: "9,999,999 B",
      }),
    ]);
  });

  it("accepts an allowance only in its registered files", () => {
    expect(validateClosure(DOC_FILES[0], "snapshot 3,638,607 B", manifest, ceilings)).toEqual([]);
    expect(validateClosure(DOC_FILES[6], "snapshot 3,638,607 B", manifest, ceilings)).toEqual([
      expect.objectContaining({ code: "unregistered-byte-literal" }),
    ]);
  });

  it("retains the shipped 2.8.0 highlight result as a historical allowance", () => {
    const nextManifest = structuredClone(manifest);
    nextManifest.measuredOnVersion = "2.9.0";
    nextManifest.measured.highlight.gzip9 += 2;
    expect(
      validateClosure(
        DOC_FILES[0],
        "Historical optimization: 767,009 B → 758,244 B, −8,765 B.",
        nextManifest,
        ceilings,
      ),
    ).toEqual([]);
  });

  it.each(["root +117 B", "a 210-second reference", "median 155.015 s"])(
    "rejects a scoped non-closure allowance in the wrong file: %s",
    (content) => {
      expect(validateAllowanceScopes(DOC_FILES[6], content)).toEqual([
        expect.objectContaining({ code: "allowance-file-scope" }),
      ]);
    },
  );

  it("does not treat bare benchmark document sizes as artifact bytes", () => {
    expect(
      validateClosure(
        DOC_FILES[1],
        "| bytes |\n| ---: |\n| 1,361 |\n| 21,846 |",
        manifest,
        ceilings,
      ),
    ).toEqual([]);
  });

  it.each([
    ["removed", "", 0],
    ["duplicated", `${shippedTable()}\n\n${shippedTable()}`, 2],
  ])("reports a %s shipped table", (_label, content, found) => {
    expect(validateShippedTables(DOC_FILES[0], content, manifest, 1)).toContainEqual(
      expect.objectContaining({ code: "shipped-table-count", expected: 1, found }),
    );
  });

  it("fails when an injected ceiling table disagrees with the manifest", () => {
    expect(validateCeilingTable(manifest, { ...ceilings, render: ceilings.render + 1 })).toEqual([
      expect.objectContaining({ code: "ceiling-table-drift", key: "render" }),
    ]);
  });

  it("--fix repairs every shipped-table column and entry-table cell", () => {
    const stale = staleTableFiles();
    const fixed = fixMdWasmSizeDocs({ files: stale, manifest, ceilings });
    expect(fixed).toEqual(correctFiles());
  });

  it("--fix is idempotent", () => {
    const stale = staleTableFiles();
    const once = fixMdWasmSizeDocs({ files: stale, manifest, ceilings });
    expect(fixMdWasmSizeDocs({ files: once, manifest, ceilings })).toEqual(once);
  });

  it("--fix repairs modeled EN prose without changing its surrounding text", () => {
    const stale = `Locked gzip-9 ceilings are root 9,000,001 B, highlight 9,000,002 B, render
9,000,003 B, and parse 9,000,004 B; the complete packed tarball ceiling is
9,000,005 B. All four ship inside their ceilings, with 9,000,006 B (root),
9,000,007 B (highlight), 9,000,008 B (render), and 9,000,009 B (parse) of headroom.
The highlight artifact is 9,000,010 B smaller raw and 9,000,011 B smaller gzip-9,
landing at about 9% of root's raw bytes and 8% of its gzipped bytes.`;
    expect(fixTableProse(DOC_FILES[0], stale, manifest, ceilings, 1)).toBe(enProse);
  });

  it("--fix repairs modeled JA prose while preserving fullwidth punctuation", () => {
    const stale = `固定された gzip-9 の上限は root 9,000,001 B、highlight 9,000,002 B、
render 9,000,003 B、parse 9,000,004 B、完全な packed tarball は 9,000,005 B です。
上限までの余裕は root 9,000,006 B、highlight 9,000,007 B、render 9,000,008 B、parse 9,000,009 B です。
raw で 9,000,010 B、gzip-9 で 9,000,011 B 小さく、highlight は root の
raw バイトの約 9%、gzip 後のバイトの約 8% に収まります。`;
    expect(fixTableProse(DOC_FILES[3], stale, manifest, ceilings, 1)).toBe(jaProse);
  });

  it("--fix leaves an unmodeled closure literal for a human", () => {
    const files = correctFiles();
    files[DOC_FILES[0]] += "\nHuman note: 9,999,999 B.";
    const fixed = fixMdWasmSizeDocs({ files, manifest, ceilings });
    expect(fixed[DOC_FILES[0]]).toBe(files[DOC_FILES[0]]);
    expect(validateMdWasmSizeDocs({ files: fixed, manifest, ceilings })).toContainEqual(
      expect.objectContaining({
        code: "unregistered-byte-literal",
        literal: "9,999,999 B",
      }),
    );
  });

  it("--fix never rewrites a registered documentation allowance", () => {
    const files = correctFiles();
    files[DOC_FILES[0]] += "\nHistorical snapshot: 3,638,607 B.";
    const fixed = fixMdWasmSizeDocs({ files, manifest, ceilings });
    expect(fixed[DOC_FILES[0]]).toBe(files[DOC_FILES[0]]);
    expect(validateMdWasmSizeDocs({ files: fixed, manifest, ceilings })).toEqual([]);
  });
});
