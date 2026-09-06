#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const DOC_FILES = [
  "crates/zfb-md-wasm/README.md",
  "crates/zfb-md-wasm/npm/README.md",
  "docs/src/content/docs/api/md-wasm.mdx",
  "docs/src/content/docs-ja/api/md-wasm.mdx",
  "docs/src/content/docs/guides/browser-markdown-preview.mdx",
  "docs/src/content/docs-ja/guides/browser-markdown-preview.mdx",
  "docs/src/content/docs/guides/syntax-highlighting.mdx",
  "docs/src/content/docs-ja/guides/syntax-highlighting.mdx",
];

export const EXPECTED_SHIPPED_TABLES = [1, 1, 1, 1, 2, 2, 0, 0];
export const EXPECTED_ENTRY_TABLES = [1, 1, 1, 1, 1, 1, 0, 0];

const ARTIFACTS = ["root", "highlight", "render", "parse"];
const COLUMNS = ["finalWasm", "gzip9", "glue", "glueGzip9"];
const COLUMN_LABELS = ["final wasm", "gzip-9", "glue", "glue gzip-9"];
const TABLE_FILE_SET = new Set(DOC_FILES.slice(0, 6));
const README_FILES = DOC_FILES.slice(0, 2);

// Documentation-only exceptions are decision-record facts, not current shipped-size truth.
// Their notes deliberately retain the provenance issue that made each number historical.
export const DOCUMENTATION_ALLOWANCES = [
  { value: 3_638_607, note: "#2447 split-package decision snapshot", files: [...TABLE_FILE_SET] },
  {
    value: 2_314_818,
    note: "#2447 root-plus-highlight decision snapshot",
    files: [...TABLE_FILE_SET],
  },
  {
    value: 1_484_705,
    note: "#2447 pre-#2449/#2450 SWC-retaining raw baseline",
    files: [...TABLE_FILE_SET],
  },
  {
    value: 767_009,
    note: "#2447 pre-#2449/#2450 SWC-retaining gzip-9 baseline",
    files: [...TABLE_FILE_SET],
  },
  {
    value: 758_244,
    note: "#2450 shipped 2.8.0 highlight gzip-9 result retained in the historical optimization comparison",
    files: [...TABLE_FILE_SET],
  },
  { value: 7_965, note: "#2447 candidate/highlight baseline delta", files: [...TABLE_FILE_SET] },
  { value: 8_765, note: "#2447 pre-#2449/#2450 gzip-9 delta", files: [...TABLE_FILE_SET] },
  { value: 62_869, note: "#2447 root candidate-vs-shipped delta", files: README_FILES },
  { value: 39_844, note: "#2447 render candidate-vs-shipped delta", files: README_FILES },
  { value: 25_482, note: "#2447 parse candidate-vs-shipped delta", files: README_FILES },
  { value: 117, note: "#2447 root glue delta", files: README_FILES },
  { value: 135, note: "#2447 render glue delta", files: README_FILES },
  { value: 18, note: "#2447 parse glue delta", files: README_FILES },
  {
    value: 210,
    note: "#2447 four-step clean production reference ceiling (seconds)",
    files: [...TABLE_FILE_SET],
  },
  { value: 155.015, note: "#2447 selected median (seconds)", files: [...TABLE_FILE_SET] },
  {
    value: 153.496,
    note: "#2447 selected median lower bound (seconds)",
    files: [...TABLE_FILE_SET],
  },
  {
    value: 165.977,
    note: "#2447 selected median upper bound (seconds)",
    files: [...TABLE_FILE_SET],
  },
];

// #2885: a release note claimed "shipped artifacts and their sizes are unchanged" for a
// documentation-only patch. Sizes were indeed unchanged, but every release stamps its own
// `ZFB_RELEASE_VERSION` string into each `.wasm`, so all four SHA-256 digests moved anyway and a
// digest-pinning consumer nearly skipped a required re-pin. The four files that carry the size
// tables must therefore also carry the digest disclaimer, so "sizes are guarded" is never read
// alone. Matched with all whitespace stripped, so prose reflow by the MDX formatter cannot break it.
export const DIGEST_DISCLAIMER_ANCHORS = {
  en: "**Sizes are guarded; content digests are not.**",
  ja: "**サイズは保証されますが、コンテンツダイジェストは保証されません。**",
};

export const DIGEST_DISCLAIMER_FILES = DOC_FILES.slice(0, 4);

function stripWhitespace(value) {
  return value.replace(/\s+/g, "");
}

export function validateDigestDisclaimer(file, content) {
  if (!DIGEST_DISCLAIMER_FILES.includes(file)) return [];
  const locale = file.includes("docs-ja/") ? "ja" : "en";
  const anchor = DIGEST_DISCLAIMER_ANCHORS[locale];
  if (stripWhitespace(content).includes(stripWhitespace(anchor))) return [];
  return [
    finding(
      "missing-digest-disclaimer",
      file,
      `${file}: shipped-size section is missing the digest disclaimer anchored on ${anchor} — see zfb#2885; add it by hand, --fix cannot write prose`,
      { locale, anchor },
    ),
  ];
}

// `1,361` / `21,846` are benchmark document sizes and intentionally excluded by
// the ` B` suffix scope. Do not widen the closure regex to bare grouped digits.
const BYTE_LITERAL = /\b\d{1,3}(?:,\d{3})+ B\b/g;

function finding(code, file, message, details = {}) {
  return { code, file, message, ...details };
}

function format(value) {
  return value.toLocaleString("en-US");
}

function parseBytes(cell) {
  const match = cell.trim().match(/^(\d{1,3}(?:,\d{3})*|\d+) B$/);
  return match ? Number(match[1].replaceAll(",", "")) : null;
}

function tableCells(line) {
  const trimmed = line.trim();
  if (!trimmed.startsWith("|") || !trimmed.endsWith("|")) return null;
  return trimmed
    .slice(1, -1)
    .split("|")
    .map((cell) => cell.trim());
}

function markdownTables(content) {
  const lines = content.split("\n");
  const tables = [];
  for (let lineIndex = 0; lineIndex < lines.length - 1; lineIndex += 1) {
    const header = tableCells(lines[lineIndex]);
    const separator = tableCells(lines[lineIndex + 1]);
    if (!header || !separator || separator.some((cell) => !/^:?-{3,}:?$/.test(cell))) continue;
    const rows = [];
    const rowLines = [];
    let end = lineIndex + 2;
    while (end < lines.length) {
      const cells = tableCells(lines[end]);
      if (!cells) break;
      rows.push(cells);
      rowLines.push(end);
      end += 1;
    }
    tables.push({ header, rows, line: lineIndex + 1, headerLine: lineIndex, rowLines });
    lineIndex = end - 1;
  }
  return tables;
}

function plainTableLine(cells) {
  return `| ${cells.join(" | ")} |`;
}

function updateTableLine(lines, lineIndex, updates) {
  const cells = tableCells(lines[lineIndex]);
  if (!cells) return;
  let changed = false;
  for (const [cellIndex, value] of updates) {
    if (cells[cellIndex] === undefined || cells[cellIndex] === value) continue;
    cells[cellIndex] = value;
    changed = true;
  }
  if (changed) lines[lineIndex] = plainTableLine(cells);
}

function rowArtifact(cell) {
  const normalized = cell.replaceAll("`", "").trim();
  if (
    /^root(?:\b|（|\()/.test(normalized) ||
    normalized === "." ||
    normalized === "@takazudo/zfb-md-wasm"
  )
    return "root";
  for (const artifact of ARTIFACTS.slice(1)) {
    if (
      normalized === artifact ||
      normalized === `./${artifact}` ||
      normalized === `@takazudo/zfb-md-wasm/${artifact}`
    )
      return artifact;
  }
  return null;
}

function checkTableValue({ findings, file, tableIndex, artifact, column, cell, expected, line }) {
  const actual = parseBytes(cell ?? "");
  if (actual !== expected) {
    findings.push(
      finding(
        "stale-table-cell",
        file,
        `${file}: table ${tableIndex}, ${artifact} ${column} is ${cell ?? "missing"}; expected ${format(expected)} B`,
        { tableIndex, artifact, column, line },
      ),
    );
  }
}

export function validateShippedTables(file, content, manifest, expectedCount) {
  const findings = [];
  const tables = markdownTables(content).filter(({ header }) =>
    COLUMN_LABELS.every(
      (label, index) => header[index + 1]?.trim().toLowerCase().replace(/\s+/g, " ") === label,
    ),
  );
  if (tables.length !== expectedCount) {
    findings.push(
      finding(
        "shipped-table-count",
        file,
        `${file}: expected ${expectedCount} shipped table(s), found ${tables.length}`,
        { expected: expectedCount, found: tables.length },
      ),
    );
  }
  tables.forEach((table, index) => {
    if (table.rows.length !== ARTIFACTS.length)
      findings.push(
        finding(
          "shipped-table-row-count",
          file,
          `${file}: table ${index + 1} expected ${ARTIFACTS.length} data rows, found ${table.rows.length}`,
          { tableIndex: index + 1, expected: ARTIFACTS.length, found: table.rows.length },
        ),
      );
    for (const artifact of ARTIFACTS) {
      const rows = table.rows.filter((row) => rowArtifact(row[0] ?? "") === artifact);
      if (rows.length !== 1) {
        findings.push(
          finding(
            "shipped-artifact-row-count",
            file,
            `${file}: table ${index + 1} expected one ${artifact} row, found ${rows.length}`,
            { tableIndex: index + 1, artifact, expected: 1, found: rows.length, line: table.line },
          ),
        );
      }
      const row = rows[0];
      if (!row) {
        continue;
      }
      COLUMNS.forEach((column, columnIndex) =>
        checkTableValue({
          findings,
          file,
          tableIndex: index + 1,
          artifact,
          column,
          cell: row[columnIndex + 1],
          expected: manifest.measured[artifact][column],
          line: table.line,
        }),
      );
    }
  });
  return findings;
}

export function fixShippedTables(file, content, manifest, expectedCount) {
  const lines = content.split("\n");
  const tables = markdownTables(content).filter(({ header }) =>
    COLUMN_LABELS.every(
      (label, index) => header[index + 1]?.trim().toLowerCase().replace(/\s+/g, " ") === label,
    ),
  );
  if (tables.length !== expectedCount) return content;

  for (const table of tables) {
    for (const artifact of ARTIFACTS) {
      const rowIndexes = table.rows
        .map((row, index) => (rowArtifact(row[0] ?? "") === artifact ? index : -1))
        .filter((index) => index >= 0);
      if (rowIndexes.length !== 1) continue;
      const row = table.rows[rowIndexes[0]];
      if (row.length < COLUMNS.length + 1) continue;
      updateTableLine(
        lines,
        table.rowLines[rowIndexes[0]],
        COLUMNS.map((column, columnIndex) => [
          columnIndex + 1,
          `${format(manifest.measured[artifact][column])} B`,
        ]),
      );
    }
  }
  return lines.join("\n");
}

export function validateEntryTables(file, content, manifest, expectedCount) {
  const findings = [];
  const tables = markdownTables(content).filter(({ header }) =>
    /^gzip-9 wasm\s*[（(].+[）)]$/.test(header[1] ?? ""),
  );
  if (tables.length !== expectedCount)
    findings.push(
      finding(
        "entry-table-count",
        file,
        `${file}: expected ${expectedCount} entry table(s), found ${tables.length}`,
        { expected: expectedCount, found: tables.length },
      ),
    );
  tables.forEach((table, index) => {
    const version = table.header[1].match(/^gzip-9 wasm\s*[（(](.+)[）)]$/)?.[1];
    if (version !== manifest.measuredOnVersion)
      findings.push(
        finding(
          "entry-table-version",
          file,
          `${file}: entry table ${index + 1} labels version ${version ?? "missing"}; expected ${manifest.measuredOnVersion}`,
          { tableIndex: index + 1, version, line: table.line },
        ),
      );
    if (table.rows.length !== ARTIFACTS.length)
      findings.push(
        finding(
          "entry-table-row-count",
          file,
          `${file}: entry table ${index + 1} expected ${ARTIFACTS.length} data rows, found ${table.rows.length}`,
          { tableIndex: index + 1, expected: ARTIFACTS.length, found: table.rows.length },
        ),
      );
    for (const artifact of ARTIFACTS) {
      const rows = table.rows.filter((row) => rowArtifact(row[0] ?? "") === artifact);
      if (rows.length !== 1) {
        findings.push(
          finding(
            "entry-artifact-row-count",
            file,
            `${file}: entry table ${index + 1} expected one ${artifact} row, found ${rows.length}`,
            { tableIndex: index + 1, artifact, expected: 1, found: rows.length, line: table.line },
          ),
        );
      }
      const row = rows[0];
      if (!row) {
        continue;
      }
      checkTableValue({
        findings,
        file,
        tableIndex: index + 1,
        artifact,
        column: "gzip9",
        cell: row[1],
        expected: manifest.measured[artifact].gzip9,
        line: table.line,
      });
    }
  });
  return findings;
}

export function fixEntryTables(file, content, manifest, expectedCount) {
  const lines = content.split("\n");
  const tables = markdownTables(content).filter(({ header }) =>
    /^gzip-9 wasm\s*[（(].+[）)]$/.test(header[1] ?? ""),
  );
  if (tables.length !== expectedCount) return content;

  for (const table of tables) {
    const versionPattern = /^(gzip-9 wasm\s*[（(])(.+)([）)])$/;
    const versionMatch = table.header[1]?.match(versionPattern);
    if (versionMatch) {
      const cells = tableCells(lines[table.headerLine]);
      if (cells && cells[1] !== undefined) {
        const replacement = `${versionMatch[1]}${manifest.measuredOnVersion}${versionMatch[3]}`;
        updateTableLine(lines, table.headerLine, [[1, replacement]]);
      }
    }

    for (const artifact of ARTIFACTS) {
      const rowIndexes = table.rows
        .map((row, index) => (rowArtifact(row[0] ?? "") === artifact ? index : -1))
        .filter((index) => index >= 0);
      if (rowIndexes.length !== 1) continue;
      const row = table.rows[rowIndexes[0]];
      if (row.length < 2) continue;
      updateTableLine(lines, table.rowLines[rowIndexes[0]], [
        [1, `${format(manifest.measured[artifact].gzip9)} B`],
      ]);
    }
  }
  return lines.join("\n");
}

const PROSE_FIELDS = {
  en: [
    {
      name: "headroom per artifact",
      pattern:
        /with\s+([\d,]+) B\s+\(root\),\s+([\d,]+) B\s+\(highlight\),\s+([\d,]+) B\s+\(render\), and ([\d,]+) B\s+\(parse\) of headroom/g,
      expected: ({ headroom }) => ARTIFACTS.map((key) => headroom[key]),
    },
    {
      name: "root-highlight deltas",
      pattern: /highlight artifact is ([\d,]+) B smaller raw and ([\d,]+) B\s+smaller gzip-9/g,
      expected: ({ rawDelta, gzipDelta }) => [rawDelta, gzipDelta],
    },
    {
      name: "root-highlight ratios",
      pattern: /about (\d+)% of root's raw bytes and (\d+)% of its\s+gzipped bytes/g,
      expected: ({ rawRatio, gzipRatio }) => [rawRatio, gzipRatio],
    },
    {
      name: "ceilings",
      pattern:
        /ceilings are\s+root\s+([\d,]+) B, highlight\s+([\d,]+) B, render\s+([\d,]+) B, and parse\s+([\d,]+) B; the complete packed\s+tarball ceiling is\s+([\d,]+) B/g,
      expected: ({ ceilings }) => [...ARTIFACTS.map((key) => ceilings[key]), ceilings.tarball],
    },
  ],
  ja: [
    {
      name: "headroom per artifact",
      pattern:
        /余裕は root ([\d,]+) B、highlight ([\d,]+) B、\s*render ([\d,]+) B、parse ([\d,]+) B です/g,
      expected: ({ headroom }) => ARTIFACTS.map((key) => headroom[key]),
    },
    {
      name: "root-highlight deltas",
      pattern: /raw で ([\d,]+) B、gzip-9 で ([\d,]+) B 小さく/g,
      expected: ({ rawDelta, gzipDelta }) => [rawDelta, gzipDelta],
    },
    {
      name: "root-highlight ratios",
      pattern: /raw バイトの約 (\d+)%、\s*gzip 後のバイトの約 (\d+)%/g,
      expected: ({ rawRatio, gzipRatio }) => [rawRatio, gzipRatio],
    },
    {
      name: "ceilings",
      pattern:
        /上限は root ([\d,]+) B、highlight ([\d,]+) B、\s*render ([\d,]+) B、parse ([\d,]+) B、完全な packed tarball は ([\d,]+) B/g,
      expected: ({ ceilings }) => [...ARTIFACTS.map((key) => ceilings[key]), ceilings.tarball],
    },
  ],
};

const VERSION_FIELDS = {
  en: [
    {
      name: "shipped artifact rows version",
      pattern: /These are the shipped \*\*([^*]+)\*\* artifact rows/g,
      expectedCount: ({ shipped }) => shipped,
    },
    {
      name: "measurement disclaimer version",
      pattern: /These are ([^\s]+) measurements/g,
      expectedCount: ({ shipped }) => shipped,
    },
    {
      name: "highlight-root comparison version",
      pattern:
        /download of the root entry:\s*[\d,]+ B gzip-9 versus\s*[\d,]+ B in ([0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)\./g,
      expectedCount: ({ comparison }) => comparison,
    },
  ],
  ja: [
    {
      name: "shipped artifact rows version",
      pattern: /出荷された \*\*([^*]+)\*\* のアーティファクトの行/g,
      expectedCount: ({ shipped }) => shipped,
    },
    {
      name: "measurement disclaimer version",
      pattern: /永続的な保証ではなく ([^\s]+) の測定値/g,
      expectedCount: ({ shipped }) => shipped,
    },
    {
      name: "highlight-root comparison version",
      pattern: /([^\s]+) では gzip-9 で [\d,]+ B、ルートは [\d,]+ B/g,
      expectedCount: ({ comparison }) => comparison,
    },
  ],
};

function derivedValues(manifest, ceilings) {
  return {
    ceilings,
    headroom: Object.fromEntries(
      ARTIFACTS.map((key) => [key, ceilings[key] - manifest.measured[key].gzip9]),
    ),
    rawDelta: manifest.measured.root.finalWasm - manifest.measured.highlight.finalWasm,
    gzipDelta: manifest.measured.root.gzip9 - manifest.measured.highlight.gzip9,
    rawRatio: Math.round(
      (manifest.measured.highlight.finalWasm / manifest.measured.root.finalWasm) * 100,
    ),
    gzipRatio: Math.round((manifest.measured.highlight.gzip9 / manifest.measured.root.gzip9) * 100),
  };
}

export function validateMeasuredVersionLabels(
  file,
  content,
  manifest,
  shippedCount,
  comparisonCount,
) {
  const findings = [];
  const locale = file.includes("docs-ja/") ? "ja" : "en";
  const counts = { shipped: shippedCount, comparison: comparisonCount };
  for (const field of VERSION_FIELDS[locale]) {
    const matches = [...content.matchAll(field.pattern)];
    const expectedCount = field.expectedCount(counts);
    if (matches.length !== expectedCount) {
      findings.push(
        finding(
          "version-anchor-count",
          file,
          `${file}: ${field.name} anchor expected ${expectedCount} occurrence(s), found ${matches.length}`,
          { field: field.name, expected: expectedCount, found: matches.length },
        ),
      );
    }
    matches.forEach((match, index) => {
      if (match[1] !== manifest.measuredOnVersion) {
        findings.push(
          finding(
            "stale-version-field",
            file,
            `${file}: ${field.name} occurrence ${index + 1} is ${match[1]}; expected ${manifest.measuredOnVersion}`,
            { field: field.name, occurrence: index + 1 },
          ),
        );
      }
    });
  }
  return findings;
}

export function fixMeasuredVersionLabels(file, content, manifest, shippedCount, comparisonCount) {
  const locale = file.includes("docs-ja/") ? "ja" : "en";
  const counts = { shipped: shippedCount, comparison: comparisonCount };
  for (const field of VERSION_FIELDS[locale]) {
    const matches = [...content.matchAll(field.pattern)];
    if (matches.length !== field.expectedCount(counts)) continue;
    for (let index = matches.length - 1; index >= 0; index -= 1) {
      const match = matches[index];
      const capturedOffset = match[0].indexOf(match[1]);
      if (capturedOffset < 0) continue;
      const start = match.index + capturedOffset;
      content =
        content.slice(0, start) +
        manifest.measuredOnVersion +
        content.slice(start + match[1].length);
    }
  }
  return content;
}

export function validateTableProse(file, content, manifest, ceilings, expectedCount) {
  const findings = [];
  const locale = file.includes("docs-ja/") ? "ja" : "en";
  const derived = derivedValues(manifest, ceilings);
  for (const field of PROSE_FIELDS[locale]) {
    const matches = [...content.matchAll(field.pattern)];
    if (matches.length !== expectedCount) {
      findings.push(
        finding(
          "prose-anchor-count",
          file,
          `${file}: ${field.name} anchor expected ${expectedCount} occurrence(s), found ${matches.length}`,
          { field: field.name, expected: expectedCount, found: matches.length },
        ),
      );
    }
    const expected = field.expected(derived);
    matches.forEach((match, index) => {
      const actual = match.slice(1).map((value) => Number(value.replaceAll(",", "")));
      if (actual.some((value, valueIndex) => value !== expected[valueIndex]))
        findings.push(
          finding(
            "stale-prose-field",
            file,
            `${file}: ${field.name} occurrence ${index + 1} has [${actual.join(", ")}]; expected [${expected.join(", ")}]`,
            { field: field.name, occurrence: index + 1 },
          ),
        );
    });
  }
  return findings;
}

function replaceCapturedValues(match, expected) {
  let cursor = 0;
  let replacement = "";
  for (let index = 0; index < expected.length; index += 1) {
    const captured = match[index + 1];
    const capturedIndex = match[0].indexOf(captured, cursor);
    if (capturedIndex < 0) return match[0];
    replacement += match[0].slice(cursor, capturedIndex);
    replacement += format(expected[index]);
    cursor = capturedIndex + captured.length;
  }
  return replacement + match[0].slice(cursor);
}

export function fixTableProse(file, content, manifest, ceilings, expectedCount) {
  const locale = file.includes("docs-ja/") ? "ja" : "en";
  const derived = derivedValues(manifest, ceilings);
  for (const field of PROSE_FIELDS[locale]) {
    const matches = [...content.matchAll(field.pattern)];
    if (matches.length !== expectedCount) continue;
    const expected = field.expected(derived);
    for (let index = matches.length - 1; index >= 0; index -= 1) {
      const match = matches[index];
      const replacement = replaceCapturedValues(match, expected);
      content =
        content.slice(0, match.index) + replacement + content.slice(match.index + match[0].length);
    }
  }
  return content;
}

export function validateHighlightRootPair(file, content, manifest, expectedCount) {
  const pattern = file.includes("docs-ja/")
    ? /gzip-9 で ([\d,]+) B、ルートは ([\d,]+) B/g
    : /(?:payload shows it:|download of the root entry:)\s*([\d,]+) B gzip-9 versus\s*([\d,]+) B (?:for root|in)/g;
  const matches = [...content.matchAll(pattern)];
  const findings = [];
  if (matches.length !== expectedCount)
    findings.push(
      finding(
        "highlight-root-anchor-count",
        file,
        `${file}: highlight-vs-root anchor expected ${expectedCount} occurrence(s), found ${matches.length}`,
        { expected: expectedCount, found: matches.length },
      ),
    );
  matches.forEach((match, index) => {
    const actual = match.slice(1).map((value) => Number(value.replaceAll(",", "")));
    const expected = [manifest.measured.highlight.gzip9, manifest.measured.root.gzip9];
    if (actual[0] !== expected[0] || actual[1] !== expected[1])
      findings.push(
        finding(
          "stale-highlight-root-pair",
          file,
          `${file}: highlight-vs-root occurrence ${index + 1} has [${actual.join(", ")}]; expected [${expected.join(", ")}]`,
          { occurrence: index + 1 },
        ),
      );
  });
  return findings;
}

export function fixHighlightRootPair(file, content, manifest, expectedCount) {
  const pattern = file.includes("docs-ja/")
    ? /gzip-9 で ([\d,]+) B、ルートは ([\d,]+) B/g
    : /(?:payload shows it:|download of the root entry:)\s*([\d,]+) B gzip-9 versus\s*([\d,]+) B (?:for root|in)/g;
  const matches = [...content.matchAll(pattern)];
  if (matches.length !== expectedCount) return content;
  const expected = [manifest.measured.highlight.gzip9, manifest.measured.root.gzip9];
  for (let index = matches.length - 1; index >= 0; index -= 1) {
    const match = matches[index];
    const replacement = replaceCapturedValues(match, expected);
    content =
      content.slice(0, match.index) + replacement + content.slice(match.index + match[0].length);
  }
  return content;
}

export function validateClosure(
  file,
  content,
  manifest,
  ceilings,
  allowances = DOCUMENTATION_ALLOWANCES,
) {
  const live = liveByteValues(manifest, ceilings);
  const findings = [];
  for (const match of content.matchAll(BYTE_LITERAL)) {
    const value = Number(match[0].slice(0, -2).replaceAll(",", ""));
    if (live.has(value)) continue;
    const allowancesForValue = allowances.filter((allowance) => allowance.value === value);
    if (allowancesForValue.some((allowance) => allowance.files.includes(file))) continue;
    const line = content.slice(0, match.index).split("\n").length;
    const scope =
      allowancesForValue.length > 0
        ? "registered for a different file"
        : "not a registered allowance";
    findings.push(
      finding(
        "unregistered-byte-literal",
        file,
        `${file}:${line}: ${match[0]} is neither derivable from the manifest nor a registered allowance for this file (${scope})`,
        { line, literal: match[0], value },
      ),
    );
  }
  return findings;
}

function liveByteValues(manifest, ceilings) {
  const derived = derivedValues(manifest, ceilings);
  return new Set([
    ...ARTIFACTS.flatMap((artifact) =>
      COLUMNS.map((column) => manifest.measured[artifact][column]),
    ),
    ...ARTIFACTS.map((artifact) => derived.headroom[artifact]),
    derived.rawDelta,
    derived.gzipDelta,
    ...Object.values(ceilings),
  ]);
}

export function validateAllowanceScopes(
  file,
  content,
  allowances = DOCUMENTATION_ALLOWANCES,
  live = new Set(),
) {
  const findings = [];
  for (const allowance of allowances) {
    if (live.has(allowance.value)) continue;
    let pattern;
    if (allowance.value === 210) {
      pattern = /\b210(?:-second| s| seconds| 秒)/g;
    } else if (!Number.isInteger(allowance.value)) {
      pattern = new RegExp(
        `(?<![\\d.])${String(allowance.value).replace(".", "\\.")}(?: s| 秒|(?=\\s*[,\\]]))`,
        "g",
      );
    } else {
      pattern = new RegExp(`(?<![\\d,])${format(allowance.value)} B\\b`, "g");
    }
    if (!allowance.files.includes(file) && pattern.test(content)) {
      findings.push(
        finding(
          "allowance-file-scope",
          file,
          `${file}: documentation allowance ${allowance.value} (${allowance.note}) is registered for a different file`,
          { value: allowance.value, note: allowance.note },
        ),
      );
    }
  }
  return findings;
}

export function validateCeilingTable(manifest, ceilings) {
  const findings = [];
  for (const key of [...ARTIFACTS, "tarball"]) {
    if (ceilings[key] !== manifest.ceilings[key])
      findings.push(
        finding(
          "ceiling-table-drift",
          "crates/zfb-md-wasm/shipped-sizes.json",
          `ceiling table ${key}=${ceilings[key] ?? "missing"} disagrees with manifest ${manifest.ceilings[key]}`,
          { key, actual: ceilings[key], expected: manifest.ceilings[key] },
        ),
      );
  }
  return findings;
}

export function validateMdWasmSizeDocs({ files, manifest, ceilings }) {
  const findings = [...validateCeilingTable(manifest, ceilings)];
  DOC_FILES.forEach((file, index) => {
    const content = files[file];
    if (typeof content !== "string") {
      findings.push(
        finding("missing-file", file, `${file}: allowlisted documentation file was not provided`),
      );
      return;
    }
    findings.push(
      ...validateShippedTables(file, content, manifest, EXPECTED_SHIPPED_TABLES[index]),
    );
    findings.push(...validateEntryTables(file, content, manifest, EXPECTED_ENTRY_TABLES[index]));
    if (TABLE_FILE_SET.has(file))
      findings.push(
        ...validateTableProse(file, content, manifest, ceilings, EXPECTED_SHIPPED_TABLES[index]),
      );
    const pairCount = file === DOC_FILES[1] || index >= 6 ? 1 : 0;
    if (pairCount > 0)
      findings.push(...validateHighlightRootPair(file, content, manifest, pairCount));
    findings.push(
      ...validateMeasuredVersionLabels(
        file,
        content,
        manifest,
        EXPECTED_SHIPPED_TABLES[index],
        index >= 6 ? 1 : 0,
      ),
    );
    findings.push(...validateDigestDisclaimer(file, content));
    findings.push(...validateClosure(file, content, manifest, ceilings));
    findings.push(
      ...validateAllowanceScopes(
        file,
        content,
        DOCUMENTATION_ALLOWANCES,
        liveByteValues(manifest, ceilings),
      ),
    );
  });
  return findings;
}

export function fixMdWasmSizeDocs({ files, manifest, ceilings }) {
  const fixedFiles = { ...files };
  DOC_FILES.forEach((file, index) => {
    const content = fixedFiles[file];
    if (typeof content !== "string") return;
    let fixed = fixShippedTables(file, content, manifest, EXPECTED_SHIPPED_TABLES[index]);
    fixed = fixEntryTables(file, fixed, manifest, EXPECTED_ENTRY_TABLES[index]);
    if (TABLE_FILE_SET.has(file)) {
      fixed = fixTableProse(file, fixed, manifest, ceilings, EXPECTED_SHIPPED_TABLES[index]);
    }
    const pairCount = file === DOC_FILES[1] || index >= 6 ? 1 : 0;
    if (pairCount > 0) fixed = fixHighlightRootPair(file, fixed, manifest, pairCount);
    fixed = fixMeasuredVersionLabels(
      file,
      fixed,
      manifest,
      EXPECTED_SHIPPED_TABLES[index],
      index >= 6 ? 1 : 0,
    );
    fixedFiles[file] = fixed;
  });
  return fixedFiles;
}

async function main() {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const [{ SHIPPED_SIZES }, budgets] = await Promise.all([
    import("../crates/zfb-md-wasm/shipped-sizes.mjs"),
    import("./assert-zfb-md-wasm-budgets.mjs"),
  ]);
  const artifactKeys = {
    default: "root",
    "highlight-only": "highlight",
    "render-only": "render",
    "parse-only": "parse",
  };
  const ceilings = Object.fromEntries(
    budgets.ARTIFACTS.map((artifact) => [artifactKeys[artifact.label], artifact.ceiling]),
  );
  ceilings.tarball = budgets.TARBALL_CEILING;
  const files = Object.fromEntries(
    DOC_FILES.map((file) => [file, readFileSync(resolve(root, file), "utf8")]),
  );
  const fix = process.argv.slice(2).includes("--fix");
  const fixedFiles = fix ? fixMdWasmSizeDocs({ files, manifest: SHIPPED_SIZES, ceilings }) : files;
  const findings = validateMdWasmSizeDocs({
    files: fixedFiles,
    manifest: SHIPPED_SIZES,
    ceilings,
  });
  if (findings.length > 0) {
    for (const item of findings) {
      const message =
        fix && item.code === "unregistered-byte-literal"
          ? `${item.message}; no model for this value; edit by hand`
          : item.message;
      console.error(message);
    }
    process.exitCode = 1;
    return;
  }
  if (fix) {
    DOC_FILES.forEach((file) => {
      if (fixedFiles[file] !== files[file]) writeFileSync(resolve(root, file), fixedFiles[file]);
    });
  }
  console.log(`OK: ${DOC_FILES.length} md-wasm documentation files match shipped-sizes.json`);
}

const argument = process.argv[1];
if (argument !== undefined && import.meta.url === pathToFileURL(argument).href) await main();
