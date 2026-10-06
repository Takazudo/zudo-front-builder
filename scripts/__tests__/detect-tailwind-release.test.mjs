import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import {
  detectTailwindRelease,
  renderReport,
  requestJson,
  runCli,
} from "../detect-tailwind-release.mjs";

const fixtures = new URL("./fixtures/tailwind-release/", import.meta.url);
const captured = JSON.parse(await readFile(new URL("packument.json", fixtures), "utf8"));
const tag = JSON.parse(await readFile(new URL("tag-ref.json", fixtures), "utf8"));
const accepted = captured.versions["4.3.2"];
const reviewed = captured.versions["4.3.3"];
const records = {
  profile: { schemaVersion: 1, profileId: "wind-preset-free" },
  acceptedFile: {
    schemaVersion: 1,
    acceptedReference: {
      package: "tailwindcss",
      profileId: "wind-preset-free",
      version: "4.3.2",
      channel: "stable",
      integrity: accepted.dist.integrity,
    },
  },
  reviewedFile: {
    schemaVersion: 1,
    reviewedThrough: {
      package: "tailwindcss",
      version: "4.3.3",
      channel: "stable",
      integrity: reviewed.dist.integrity,
      disposition: "review-only",
    },
  },
};
const metadata = (version) => ({
  name: "tailwindcss",
  version,
  dist: {
    integrity: reviewed.dist.integrity,
    tarball: `https://registry.npmjs.org/tailwindcss/-/tailwindcss-${version}.tgz`,
    shasum: reviewed.dist.shasum,
  },
});
const response = (body, status = 200) => ({
  ok: status >= 200 && status < 300,
  status,
  text: async () => JSON.stringify(body),
});
function packument(versions, latest = "4.3.3") {
  return {
    name: "tailwindcss",
    "dist-tags": { latest },
    versions: Object.fromEntries(versions.map((v) => [v, captured.versions[v] ?? metadata(v)])),
  };
}
function client(pack, tagResponse = response({ ...tag, ref: "refs/tags/v4.4.0" })) {
  const calls = [];
  return {
    calls,
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      return url.includes("api.github.com") ? tagResponse : response(pack);
    },
  };
}
const detect = (pack, options = {}) => {
  const c = client(pack, options.tagResponse);
  return detectTailwindRelease({ records, fetchImpl: c.fetchImpl, retries: 0, ...options });
};

function contract(report) {
  expect(Object.keys(report)).toEqual([
    "schemaVersion",
    "kind",
    "status",
    "exitCode",
    "package",
    "profileId",
    "track",
    "accepted",
    "reviewedThrough",
    "latestDistTag",
    "candidate",
    "offTrackNewest",
    "interveningVersions",
    "interveningTruncated",
    "notes",
    "failure",
    "identity",
  ]);
  expect(report.exitCode).toBe(
    {
      "no-change": 0,
      "already-reviewed": 0,
      "prerelease-only": 0,
      REVIEW_NEEDED: 10,
      "operational-failure": 1,
    }[report.status],
  );
  expect(report.identity).toMatch(/^sha256:[0-9a-f]{64}$/);
}

describe("Tailwind release detection", () => {
  it("returns no-change when newest equals accepted", async () => {
    const report = await detect(packument(["4.3.2"], "4.3.2"));
    contract(report);
    expect(report.status).toBe("no-change");
    expect(report.candidate).toBeNull();
  });

  it("reports a new stable release with transition, interval, source, and deterministic identity", async () => {
    const pack = packument(["4.3.2", "4.3.3", "4.3.4", "4.4.0"]);
    const first = await detect(pack);
    const second = await detect(pack);
    contract(first);
    expect(JSON.stringify(first)).toBe(JSON.stringify(second));
    expect(first.status).toBe("REVIEW_NEEDED");
    expect(first.candidate.version).toBe("4.4.0");
    expect(first.candidate.transition).toEqual({ major: false, minor: true, patch: false });
    expect(first.interveningVersions).toEqual(["4.3.3", "4.3.4", "4.4.0"]);
    expect(first.candidate.source).toEqual({
      tag: "v4.4.0",
      observedTagObject: { type: "commit", sha: tag.object.sha },
      status: "tag-observed-artifact-link-unverified",
    });
    const expectedSummary =
      "Tailwind release detection: REVIEW_NEEDED\n" +
      "Track: tailwindcss / wind-preset-free / stable\n" +
      "Accepted: 4.3.2 → Candidate: 4.4.0\n" +
      "Intervening versions: 3\n" +
      "Notes:\n- Latest dist-tag 4.3.3 is below newest on-track 4.4.0.\n";
    expect(renderReport(first)).toBe(expectedSummary);
    const directory = await mkdtemp(join(tmpdir(), "zfb-tailwind-render-"));
    try {
      const path = join(directory, "report.json");
      await writeFile(path, JSON.stringify(first));
      let output = "";
      const exitCode = await runCli(["--render", path], {
        write: (chunk) => {
          output += chunk;
        },
      });
      expect(exitCode).toBe(0);
      expect(output).toBe(expectedSummary);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  it("keeps the real baseline already reviewed despite newer off-track prerelease", async () => {
    const c = client(packument(["4.3.2", "4.3.3", "4.4.0-alpha.1"]));
    const report = await detectTailwindRelease({ records, fetchImpl: c.fetchImpl });
    contract(report);
    expect(report.status).toBe("already-reviewed");
    expect(report.candidate.source).toBeNull();
    expect(report.offTrackNewest).toEqual({ version: "4.4.0-alpha.1", channel: "prerelease" });
    expect(c.calls).toHaveLength(1);
  });

  it("classifies prerelease-only and adds notes for prerelease latest and skipped versions", async () => {
    const pack = packument(["4.3.2", "4.4.0-alpha.1"], "4.4.0-alpha.1");
    pack.versions.bogus = {};
    const report = await detect(pack);
    contract(report);
    expect(report.status).toBe("prerelease-only");
    expect(report.notes.join(" ")).toContain("Skipped 1 unparseable");
    expect(report.notes.join(" ")).toContain("points at a prerelease");
  });

  it("marks major transitions and caps the intervening list", async () => {
    const versions = ["4.3.2", ...Array.from({ length: 31 }, (_, i) => `4.3.${i + 3}`), "5.0.0"];
    const report = await detect(packument(versions), {
      tagResponse: response({ ...tag, ref: "refs/tags/v5.0.0" }),
    });
    contract(report);
    expect(report.candidate.transition.major).toBe(true);
    expect(report.interveningVersions).toHaveLength(30);
    expect(report.interveningVersions[0]).toBe("4.3.3");
    expect(report.interveningTruncated).toBe(true);
  });

  it("classifies tag 404, annotated tags, authentication errors, and malformed refs", async () => {
    const pack = packument(["4.3.2", "4.4.0"]);
    const missing = await detect(pack, { tagResponse: response({}, 404) });
    contract(missing);
    expect(missing.status).toBe("REVIEW_NEEDED");
    expect(missing.candidate.source.status).toBe("tag-missing");
    const annotated = await detect(pack, {
      tagResponse: response({
        ...tag,
        ref: "refs/tags/v4.4.0",
        object: { ...tag.object, type: "tag" },
      }),
    });
    expect(annotated.candidate.source.status).toBe("tag-object-unresolved");
    expect(annotated.candidate.source.observedTagObject.type).toBe("tag");
    for (const status of [401, 403]) {
      const denied = await detect(pack, { tagResponse: response({}, status) });
      contract(denied);
      expect(denied.failure).toMatchObject({ stage: "tag", httpStatus: status });
      expect(denied.candidate).toBeNull();
    }
    const malformed = await detect(pack, {
      tagResponse: response({ ...tag, ref: "refs/tags/wrong" }),
    });
    expect(malformed.failure.stage).toBe("tag");
  });

  it("reports malformed registry data as operational failure", async () => {
    const cases = [
      {},
      { versions: { bogus: {} } },
      { versions: { "4.3.2": { ...accepted, dist: { tarball: accepted.dist.tarball } } } },
    ];
    for (const pack of cases) {
      const report = await detect(pack);
      contract(report);
      expect(report.failure.stage).toBe("registry");
      expect(report.candidate).toBeNull();
    }
  });

  it("preserves 5xx status after bounded retries, and classifies network and timeout errors", async () => {
    let calls = 0;
    const server = await detectTailwindRelease({
      records,
      retries: 2,
      sleepImpl: async () => {},
      fetchImpl: async () => {
        calls++;
        return response({}, 503);
      },
    });
    contract(server);
    expect(calls).toBe(3);
    expect(server.failure).toMatchObject({ stage: "registry", httpStatus: 503 });
    const network = await detectTailwindRelease({
      records,
      retries: 0,
      fetchImpl: async () => {
        throw Error("offline");
      },
    });
    expect(network.failure).toMatchObject({ stage: "registry", httpStatus: null });
    const timeout = await detectTailwindRelease({
      records,
      retries: 0,
      timeoutMs: 1,
      fetchImpl: async () => new Promise(() => {}),
    });
    expect(timeout.failure).toMatchObject({ stage: "registry", httpStatus: null });
  });

  it("handles invalid records without a registry request", async () => {
    const report = await detectTailwindRelease({
      records: { ...records, acceptedFile: {} },
      fetchImpl: async () => {
        throw Error("unexpected fetch");
      },
    });
    contract(report);
    expect(report.failure.stage).toBe("records");
    expect(report.profileId).toBeNull();
  });

  it("requestJson retries 429 but not other 4xx", async () => {
    let calls = 0;
    const body = await requestJson("https://example.test", {
      retries: 1,
      sleepImpl: async () => {},
      fetchImpl: async () => {
        calls++;
        return response({}, calls === 1 ? 429 : 200);
      },
    });
    expect(body).toEqual({});
    expect(calls).toBe(2);
    calls = 0;
    await expect(
      requestJson("https://example.test", {
        retries: 2,
        sleepImpl: async () => {},
        fetchImpl: async () => {
          calls++;
          return response({}, 403);
        },
      }),
    ).rejects.toMatchObject({ httpStatus: 403 });
    expect(calls).toBe(1);
  });
});
