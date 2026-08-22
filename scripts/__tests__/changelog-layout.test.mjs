import { lstatSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const DOCS_ROOT = join(REPO_ROOT, "docs");
const CHANGELOG_ROOT = join(DOCS_ROOT, "src", "content", "docs", "changelog");
const REDIRECTS_PATH = join(DOCS_ROOT, "public", "_redirects");
const RELEASE_SKILL_RELATIVE_PATH = ".claude/skills/l-make-release/SKILL.md";

const LANES = ["zfb", "zfb-runtime", "zfb-adapter-cloudflare", "create-zfb", "zfb-md-wasm"];
const HISTORICAL_CUTOFF = "v2.10.0";
const MIGRATED_ZFB_PAGE_COUNT = 114;
const LANE_INDEX_POSITIONS = new Map(LANES.map((lane, index) => [lane, index + 1]));
const RELEASE_ENTRIES = new Map();

function read(relativePath) {
  return readFileSync(join(REPO_ROOT, relativePath), "utf8");
}

function readFrontmatter(text, source) {
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/);
  expect(match, `${source} must start with YAML frontmatter`).not.toBeNull();

  const fields = new Map();
  for (const line of match[1].split(/\r?\n/)) {
    const field = line.match(/^([A-Za-z_][A-Za-z0-9_]*):\s*(.*?)\s*$/);
    if (!field) continue;
    const [, key, rawValue] = field;
    fields.set(key, rawValue.replace(/^(['"])(.*)\1$/, "$2"));
  }
  return fields;
}

function releaseEntries(lane) {
  if (RELEASE_ENTRIES.has(lane)) return RELEASE_ENTRIES.get(lane);
  const directory = join(CHANGELOG_ROOT, lane);
  const entries = readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.name.startsWith("v") && entry.name.endsWith(".mdx"))
    .map((entry) => {
      const version = entry.name.slice(0, -4);
      const path = join(directory, entry.name);
      const text = readFileSync(path, "utf8");
      return {
        lane,
        name: entry.name,
        version,
        text,
        frontmatter: readFrontmatter(text, `changelog/${lane}/${entry.name}`),
      };
    });
  RELEASE_ENTRIES.set(lane, entries);
  return entries;
}

function parseVersion(version) {
  const match = version.match(/^v(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/);
  if (!match) throw new Error(`invalid changelog version: ${version}`);
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ? match[4].split(".") : [],
  };
}

function compareVersions(left, right) {
  const a = parseVersion(left);
  const b = parseVersion(right);
  for (const key of ["major", "minor", "patch"]) {
    if (a[key] !== b[key]) return a[key] - b[key];
  }
  if (a.prerelease.length === 0 || b.prerelease.length === 0) {
    return a.prerelease.length === b.prerelease.length ? 0 : a.prerelease.length === 0 ? 1 : -1;
  }
  for (let index = 0; index < Math.min(a.prerelease.length, b.prerelease.length); index += 1) {
    const leftIdentifier = a.prerelease[index];
    const rightIdentifier = b.prerelease[index];
    if (leftIdentifier === rightIdentifier) continue;
    const leftNumeric = /^\d+$/.test(leftIdentifier);
    const rightNumeric = /^\d+$/.test(rightIdentifier);
    if (leftNumeric && rightNumeric) return Number(leftIdentifier) - Number(rightIdentifier);
    if (leftNumeric !== rightNumeric) return leftNumeric ? -1 : 1;
    return leftIdentifier < rightIdentifier ? -1 : 1;
  }
  return a.prerelease.length - b.prerelease.length;
}

function sortedByVersion(entries) {
  return [...entries].sort((left, right) => compareVersions(left.version, right.version));
}

function sortedByPosition(entries) {
  return [...entries].sort((left, right) => {
    const leftPosition = Number(left.frontmatter.get("sidebar_position"));
    const rightPosition = Number(right.frontmatter.get("sidebar_position"));
    return leftPosition - rightPosition;
  });
}

function isHistorical(version) {
  return compareVersions(version, HISTORICAL_CUTOFF) <= 0;
}

function countOccurrences(text, needle) {
  return text.split(needle).length - 1;
}

function sectionBetween(text, start, end) {
  const startIndex = text.indexOf(start);
  expect(startIndex, `missing section marker: ${start}`).toBeGreaterThanOrEqual(0);
  const bodyStart = startIndex + start.length;
  const endIndex = end ? text.indexOf(end, bodyStart) : text.length;
  expect(endIndex, `missing section end marker: ${end}`).toBeGreaterThanOrEqual(bodyStart);
  return text.slice(bodyStart, endIndex);
}

function parseRedirects(text) {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "")
    .map((line) => {
      const parts = line.trim().split(/\s+/);
      expect(parts, `redirect must have source, target, and status: ${line}`).toHaveLength(3);
      return { source: parts[0], target: parts[1], status: parts[2], line };
    });
}

describe("integrated changelog contract", () => {
  it("keeps the five lanes and their order across landing, header, root contract, and release skill", () => {
    const landing = read("docs/src/content/docs/changelog/index.mdx");
    const docsContract = read("docs/CLAUDE.md");
    const rootContract = read("CLAUDE.md");
    const releaseSkill = read(RELEASE_SKILL_RELATIVE_PATH);

    expect(landing).toContain("The changelog is split into five package lanes");
    expect(landing).toContain("The other four lanes are reserved for package-specific notes");
    expect(landing).toContain('<CategoryNav category="changelog" />');

    const expectedContractSentence =
      "The changelog has five package lanes in this stable order: `zfb`, `zfb-runtime`, `zfb-adapter-cloudflare`, `create-zfb`, and `zfb-md-wasm`.";
    expect(docsContract.replace(/\s+/g, " ")).toContain(expectedContractSentence);
    expect(rootContract.replace(/\s+/g, " ")).toContain(
      "Each future release nevertheless authors exactly five default-locale-only English MDX notes:",
    );

    const laneClassification = sectionBetween(
      releaseSkill,
      "Then classify every user-facing commit and diff into package lanes by ownership:",
      "A change that affects multiple packages",
    );
    expect([...laneClassification.matchAll(/\*\*([^*]+)\*\*:/g)].map((match) => match[1])).toEqual(
      LANES,
    );
  });

  it("keeps the Changelog header parent scoped and its children single-segment", () => {
    const config = read("docs/zfb.config.ts");
    const changelogNav = config.match(
      /label: "Changelog",[\s\S]*?children:\s*\[([\s\S]*?)\n\s*\],\n\s*\},/,
    );
    expect(changelogNav, "Changelog header item must have a children block").not.toBeNull();

    const parent = changelogNav[0];
    const children = changelogNav[1];
    expect(parent).toContain('categoryMatch: "changelog"');
    expect(children).not.toMatch(/categoryMatch\s*:/);

    const entries = [...children.matchAll(/label:\s*"([^"]+)"[\s\S]*?path:\s*"([^"]+)"/g)].map(
      ([, label, path]) => ({ label, path }),
    );
    expect(entries).toEqual(
      LANES.map((lane) => ({ label: lane, path: `/docs/changelog/${lane}` })),
    );
  });

  it("keeps landing and package index metadata, CategoryNav scopes, and lane boundaries aligned", () => {
    const landingPath = "docs/src/content/docs/changelog/index.mdx";
    const landing = read(landingPath);
    const landingFrontmatter = readFrontmatter(landing, landingPath);
    expect(landingFrontmatter.get("pagination_prev")).toBe("null");
    expect(landingFrontmatter.get("pagination_next")).toBe("null");
    expect(countOccurrences(landing, '<CategoryNav category="changelog" />')).toBe(1);

    for (const lane of LANES) {
      const relativePath = `docs/src/content/docs/changelog/${lane}/index.mdx`;
      const index = read(relativePath);
      const frontmatter = readFrontmatter(index, relativePath);
      expect(frontmatter.get("title")).toBe(lane);
      expect(frontmatter.get("sidebar_position")).toBe(String(LANE_INDEX_POSITIONS.get(lane)));
      expect(frontmatter.get("category_sort_order")).toBe("desc");
      expect(frontmatter.get("pagination_prev")).toBe("null");
      expect(frontmatter.get("pagination_next")).toBe("null");
      expect(countOccurrences(index, `<CategoryNav category="changelog/${lane}" />`)).toBe(1);
    }
  });

  it("keeps the migrated corpus nested once, with no changelog category symlinks", () => {
    const rootEntries = readdirSync(CHANGELOG_ROOT, { withFileTypes: true });
    expect(
      rootEntries.filter((entry) => entry.name.startsWith("v") && entry.name.endsWith(".mdx")),
    ).toEqual([]);
    expect(
      rootEntries
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
        .sort(),
    ).toEqual([...LANES].sort());

    for (const directory of [CHANGELOG_ROOT, ...LANES.map((lane) => join(CHANGELOG_ROOT, lane))]) {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        expect(
          lstatSync(join(directory, entry.name)).isSymbolicLink(),
          `${directory}/${entry.name}`,
        ).toBe(false);
      }
    }

    const allEntries = LANES.flatMap((lane) => releaseEntries(lane));
    const historical = allEntries.filter((entry) => isHistorical(entry.version));
    const historicalNames = historical.map((entry) => entry.version);
    expect(historical.filter((entry) => entry.lane === "zfb")).toHaveLength(
      MIGRATED_ZFB_PAGE_COUNT,
    );
    expect(new Set(historicalNames).size).toBe(historicalNames.length);
    expect(historical.every((entry) => entry.lane === "zfb")).toBe(true);

    const zfbHistorical = releaseEntries("zfb").filter((entry) => isHistorical(entry.version));
    expect(zfbHistorical.map((entry) => entry.version)).toEqual(historicalNames);
  });

  it("keeps every lane's sidebar positions unique, consecutive, and semver ordered", () => {
    for (const lane of LANES) {
      const entries = releaseEntries(lane);
      const byPosition = sortedByPosition(entries);
      const positions = byPosition.map((entry) =>
        Number(entry.frontmatter.get("sidebar_position")),
      );
      expect(positions, `${lane} sidebar positions`).toEqual(
        Array.from({ length: entries.length }, (_, index) => index + 1),
      );
      expect(byPosition.map((entry) => entry.frontmatter.get("title"))).toEqual(
        byPosition.map((entry) => entry.version),
      );

      for (let index = 1; index < byPosition.length; index += 1) {
        expect(
          compareVersions(byPosition[index - 1].version, byPosition[index].version),
          `${lane} positions ${positions[index - 1]} and ${positions[index]}`,
        ).toBeLessThan(0);
      }

      expect(byPosition.map((entry) => entry.version)).toEqual(
        sortedByVersion(entries).map((entry) => entry.version),
      );
    }

    const zfb = sortedByPosition(releaseEntries("zfb"));
    const prereleaseIndex = zfb.findIndex((entry) => entry.version === "v1.1.0-next.1");
    const stableIndex = zfb.findIndex((entry) => entry.version === "v1.1.0");
    expect(prereleaseIndex).toBeGreaterThanOrEqual(0);
    expect(stableIndex).toBe(prereleaseIndex + 1);
  });

  it("keeps the oldest release at a lane-local pager boundary", () => {
    for (const lane of LANES) {
      const entries = releaseEntries(lane);
      if (entries.length === 0) continue;
      const oldest = sortedByVersion(entries)[0];
      expect(oldest.frontmatter.get("pagination_next"), `${lane}/${oldest.name}`).toBe("null");
      for (const otherLane of LANES.filter((candidate) => candidate !== lane)) {
        expect(oldest.text).not.toContain(`/docs/changelog/${otherLane}`);
      }
    }
  });

  it("retains the approved v0.1.0-next.12 relative-link repair set", () => {
    const page = releaseEntries("zfb").find((entry) => entry.version === "v0.1.0-next.12");
    expect(page).toBeDefined();
    const relativeLinks = [...page.text.matchAll(/\]\((\.\.\/[^)]+)\)/g)].map(
      ([, target]) => target,
    );
    expect(relativeLinks).toHaveLength(19);
    expect(relativeLinks.every((target) => target.startsWith("../../"))).toBe(true);
    expect((page.text.match(/\.\.\/\.\.\//g) ?? []).length).toBe(19);
  });

  it("keeps exactly two 301 compatibility redirects per migrated release", () => {
    const redirects = parseRedirects(readFileSync(REDIRECTS_PATH, "utf8"));
    const migrated = releaseEntries("zfb").filter((entry) => isHistorical(entry.version));
    const migratedVersions = new Set(migrated.map((entry) => entry.version));
    expect(redirects).toHaveLength(migrated.length * 2);

    const sources = new Set();
    const redirectsByVersion = new Map();
    for (const redirect of redirects) {
      expect(redirect.status, redirect.line).toBe("301");
      expect(redirect.source).not.toMatch(/^\/docs\/changelog\/?$/);
      expect(redirect.source).not.toMatch(new RegExp(`^/docs/changelog/(?:${LANES.join("|")})/?$`));
      expect(redirect.source).toMatch(/^\/docs\/changelog\/v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\/?$/);
      expect(redirect.target).toMatch(
        /^\/docs\/changelog\/zfb\/v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\/$/,
      );

      const sourceVersion = redirect.source.match(/^\/docs\/changelog\/(v[^/]+)\/?$/)[1];
      const targetVersion = redirect.target.match(/^\/docs\/changelog\/zfb\/(v[^/]+)\/$/)[1];
      expect(targetVersion).toBe(sourceVersion);
      expect(migratedVersions.has(sourceVersion), redirect.line).toBe(true);
      expect(releaseEntries("zfb").some((entry) => entry.version === targetVersion)).toBe(true);
      expect(sources.has(redirect.source), `duplicate redirect source: ${redirect.source}`).toBe(
        false,
      );
      sources.add(redirect.source);
      const versions = redirectsByVersion.get(sourceVersion) ?? [];
      versions.push(redirect.source);
      redirectsByVersion.set(sourceVersion, versions);
    }

    for (const version of migratedVersions) {
      expect(redirectsByVersion.get(version)?.sort()).toEqual([
        `/docs/changelog/${version}`,
        `/docs/changelog/${version}/`,
      ]);
    }
  });

  it("keeps the release skill authoring and staging five independent lane pages", () => {
    const skill = read(RELEASE_SKILL_RELATIVE_PATH);
    const expectedPaths = LANES.map(
      (lane) => `docs/src/content/docs/changelog/${lane}/v<version>.mdx`,
    );
    const createSection = sectionBetween(
      skill,
      "Create exactly these five English pages",
      "Use this shape for each page:",
    );
    const stageSection = sectionBetween(skill, "git add packages/", "git commit -m");
    expect(
      [
        ...createSection.matchAll(/docs\/src\/content\/docs\/changelog\/[^\s`]+\/v<version>\.mdx/g),
      ].map(([match]) => match),
    ).toEqual(expectedPaths);
    expect(
      [
        ...stageSection.matchAll(/docs\/src\/content\/docs\/changelog\/[^\s`]+\/v<version>\.mdx/g),
      ].map(([match]) => match),
    ).toEqual(expectedPaths);
    expect(stageSection).not.toContain("docs/src/content/docs/changelog/v<version>.mdx");
  });

  it("keeps release position scans lane-local and documents the first-page boundary", () => {
    const skill = read(RELEASE_SKILL_RELATIVE_PATH);
    const positionSection = sectionBetween(
      skill,
      "Compute `sidebar_position` independently in each package directory.",
      "The migrated `zfb` lane continues after its historical maximum.",
    );
    for (const lane of LANES) {
      expect(positionSection).toContain(
        `find docs/src/content/docs/changelog/${lane} -maxdepth 1 -type f -name 'v*.mdx'`,
      );
    }
    expect(positionSection).not.toMatch(
      /find docs\/src\/content\/docs\/changelog -maxdepth 1 -type f -name 'v\*\.mdx'/,
    );
    expect(skill).toContain("include `pagination_next: null` in that first page's frontmatter");
    expect(skill).toContain("previous/next traversal cannot cross into another package lane");
    expect(skill).toContain("use exactly `- No package-specific changes.`");
  });

  it("keeps five independent package headings and notes sources in release assembly", () => {
    const skill = read(RELEASE_SKILL_RELATIVE_PATH);
    const notesStart = skill.indexOf("ZFB_NOTES=");
    expect(notesStart).toBeGreaterThanOrEqual(0);
    const notesEnd = skill.indexOf("Keep these five extractions independent", notesStart);
    expect(notesEnd).toBeGreaterThan(notesStart);
    const notesSection = skill.slice(notesStart, notesEnd);
    const expectedNotes = LANES.map((lane) => ({
      variable: `${lane === "zfb" ? "ZFB" : lane.replaceAll("-", "_").toUpperCase()}_NOTES`,
      path: `docs/src/content/docs/changelog/${lane}/v<version>.mdx`,
      heading:
        lane === "zfb"
          ? "## @takazudo/zfb"
          : lane === "zfb-runtime"
            ? "## @takazudo/zfb-runtime"
            : lane === "zfb-adapter-cloudflare"
              ? "## @takazudo/zfb-adapter-cloudflare"
              : lane === "create-zfb"
                ? "## create-zfb"
                : "## @takazudo/zfb-md-wasm",
    }));

    for (const { variable, path } of expectedNotes) {
      expect(notesSection).toContain(`${variable}=$(sed`);
      expect(notesSection).toContain(path);
    }
    expect(
      [...notesSection.matchAll(/'## [^']+'/g)].map(([heading]) => heading.slice(1, -1)),
    ).toEqual(expectedNotes.map(({ heading }) => heading));
    expect(
      [...notesSection.matchAll(/'(## [^']+)' \"\$([A-Z0-9_]+_NOTES)\"/g)].map(
        ([, heading, variable]) => ({ heading, variable }),
      ),
    ).toEqual(expectedNotes.map(({ heading, variable }) => ({ heading, variable })));
    expect(skill).toContain("Keep these five extractions independent");
  });

  it("keeps initially empty package lanes honest about shared history", () => {
    const landing = read("docs/src/content/docs/changelog/index.mdx");
    expect(landing.replace(/\s+/g, " ")).toContain("their release pages live in the `zfb` lane");
    expect(landing.replace(/\s+/g, " ")).toContain("historical pages are not duplicated into them");

    for (const lane of LANES.slice(1)) {
      const relativePath = `docs/src/content/docs/changelog/${lane}/index.mdx`;
      const index = read(relativePath);
      expect(index).toMatch(
        /shared\s+lockstep\s+history[\s\S]*through \*\*v2\.10\.0\*\* remains in the `zfb` lane/,
      );
      expect(index.replace(/\s+/g, " ")).toContain("intentionally not duplicated here");
      expect(releaseEntries(lane).filter((entry) => isHistorical(entry.version))).toEqual([]);
    }
  });
});
