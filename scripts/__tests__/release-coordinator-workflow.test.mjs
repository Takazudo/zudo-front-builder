// Structural backstop for the Actions release route (#4701): the wiring that
// coordinate-release.test.mjs cannot see. Least privilege per job, no npm
// credential or id-token anywhere in the coordinator, a main-only dispatch
// that defaults to a dry run, the tag-ref acknowledgement in release.yml, and
// the tag-bound installer-smoke dispatch.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { parse } from "yaml";
import { CANDIDATE_CONTRACTS } from "../release/coordinate-release.mjs";
import { scriptsDir } from "./helpers/darwin-fixture.mjs";

const repoRoot = join(scriptsDir, "..");
const read = (path) => readFileSync(join(repoRoot, path), "utf8");
const coordinatorText = read(".github/workflows/release-coordinator.yml");
const coordinator = parse(coordinatorText);
// The executable part only: the header comment names what the workflow never does.
const coordinatorCode = coordinatorText
  .split("\n")
  .filter((line) => !/^\s*#/.test(line))
  .join("\n");
const releaseText = read(".github/workflows/release.yml");
const release = parse(releaseText);
const smoke = parse(read(".github/workflows/node-free-smoke.yml"));

describe("release-coordinator.yml", () => {
  it("is dispatch-only, defaults to a dry run and requires version + sha", () => {
    expect(Object.keys(coordinator.on)).toEqual(["workflow_dispatch"]);
    const inputs = coordinator.on.workflow_dispatch.inputs;
    expect(Object.keys(inputs).sort()).toEqual(["dry_run", "sha", "version"]);
    expect(inputs.dry_run.default).toBe(true);
    expect(inputs.version.required).toBe(true);
    expect(inputs.sha.required).toBe(true);
    expect(coordinatorCode).not.toMatch(/allow_provenance_downgrade|provenance-policy\.json/);
  });

  it("grants each job only its own permissions and never id-token", () => {
    expect(coordinator.permissions).toEqual({});
    const perms = Object.fromEntries(
      Object.entries(coordinator.jobs).map(([name, job]) => [name, job.permissions]),
    );
    expect(perms).toEqual({
      gate: { contents: "write", actions: "read", checks: "read", statuses: "read" },
      tag: { contents: "write", actions: "read", checks: "read", statuses: "read" },
      publish: { contents: "write", actions: "read", checks: "read", statuses: "read" },
      dispatch: { contents: "write", actions: "write", checks: "read", statuses: "read" },
    });
    expect(coordinatorCode).not.toMatch(/id-token|NPM_TOKEN|NODE_AUTH_TOKEN|npm publish/);
  });

  it("orders gate → tag → publish → dispatch and skips every write on a dry run", () => {
    const { gate, tag, publish, dispatch } = coordinator.jobs;
    expect(gate.needs).toBeUndefined();
    expect(tag.needs).toBe("gate");
    expect(tag.if).toMatch(/inputs\.dry_run != true/);
    expect(publish.needs).toBe("tag");
    expect(dispatch.needs).toBe("publish");
    // A skipped `tag` (dry run) must skip everything after it: no always()/
    // !cancelled() override may let publish or dispatch run on their own.
    for (const job of [publish, dispatch]) {
      expect(job.if).toBeUndefined();
    }
  });

  it("refuses a dispatch from anything but main and never cancels a sequence", () => {
    const guard = coordinator.jobs.gate.steps[0];
    expect(guard.if).toMatch(/github\.ref != 'refs\/heads\/main'/);
    expect(coordinator.concurrency["cancel-in-progress"]).toBe(false);
  });

  it("runs no macOS runner and checks out without persisted credentials", () => {
    for (const job of Object.values(coordinator.jobs)) {
      expect(job["runs-on"]).toBe("ubuntu-latest");
      for (const step of job.steps) {
        if (String(step.uses ?? "").startsWith("actions/checkout@")) {
          expect(step.with["persist-credentials"]).toBe(false);
        }
      }
    }
  });

  it("calls the coordinator script once per job with its own subcommand", () => {
    const sub = (job) =>
      coordinator.jobs[job].steps
        .map((s) => /coordinate-release\.mjs (\w+)/.exec(s.run ?? "")?.[1])
        .filter(Boolean);
    expect(sub("gate")).toEqual(["gate"]);
    expect(sub("tag")).toEqual(["tag"]);
    expect(sub("publish")).toEqual(["publish"]);
    expect(sub("dispatch")).toEqual(["dispatch"]);
  });
});

describe("candidate contract markers (this checkout satisfies its own contract)", () => {
  it.each(CANDIDATE_CONTRACTS)("$path carries $marker", ({ path, marker }) => {
    expect(read(path)).toContain(marker);
  });
});

describe("release.yml: tag-ref dispatch acknowledgement", () => {
  const ack = release.jobs["release-context"].steps.find((s) => s.id === "ack");

  it("reads the owner's marker from the published Release only for a non-recovery tag-ref dispatch", () => {
    expect(ack.run).toMatch(/IS_RECOVERY" != "true"/);
    expect(ack.run).toMatch(/TRIGGER_REF" == "refs\/tags\/\$\{RELEASE_TAG\}"/);
    expect(ack.run).toMatch(/select\(\.isDraft == false\)/);
    expect(ack.run).toMatch(/grep -Fxq "\$MARKER"/);
    expect(ack.run).toMatch(/failing closed/);
  });

  it("keeps the Release body out of the script text (env only) and never sets the input", () => {
    expect(ack.run).not.toMatch(/\$\{\{/);
    expect(releaseText).not.toMatch(/allow_provenance_downgrade:\s*true/);
    expect(release.jobs["release-context"].permissions).toEqual({ contents: "read" });
  });
});

describe("node-free-smoke.yml: tag-bound installer dispatch", () => {
  it("accepts a dispatch only at exactly refs/tags/<release_tag>", () => {
    expect(smoke.on.workflow_dispatch.inputs.release_tag.required).toBe(true);
    const job = smoke.jobs["smoke-released"];
    expect(job.if).toMatch(/github\.ref == format\('refs\/tags\/\{0\}', inputs\.release_tag\)/);
    expect(job.steps[0].with.ref).toMatch(/github\.sha/);
  });

  it("does not let a dispatch start the PR/main jobs or the showcase deploy", () => {
    expect(smoke.jobs["build-binary"].if).toMatch(
      /pull_request' \|\| github\.event_name == 'push'/,
    );
    expect(smoke.jobs["showcase-deploy"].if).toMatch(/github\.event_name == 'push'/);
  });
});
