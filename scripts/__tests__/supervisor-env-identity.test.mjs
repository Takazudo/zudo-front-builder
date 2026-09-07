import { describe, expect, it } from "vitest";
import {
  ENV_IDENTITY_VERSION,
  REPORTED_ENV_KEYS,
  STEERING_ENV_KEYS,
  VOLATILE_ENV_KEYS,
  digestOf,
  envIdentityToken,
  parseEnvIdentity,
  steeringEnvDigest,
  steeringEnvIdentity,
} from "../supervisor-env-identity.mjs";

const BASE_ENV = {
  npm_execpath: "/usr/local/bin/pnpm",
  npm_config_user_agent: "pnpm/9.0.0 npm/? node/v20.11.0 darwin arm64",
  NODE_OPTIONS: "",
  NODE_ENV: "test",
  CI: "",
  PNPM_HOME: "/Users/example/.local/share/pnpm",
  TMPDIR: "/tmp",
};

describe("supervisor-env-identity", () => {
  it("leaves the digest identical when a non-steering key changes", () => {
    const before = steeringEnvDigest(BASE_ENV);
    for (const key of ["GITHUB_RUN_ID", "GITHUB_SHA", "_", "SHLVL", "TMUX_PANE", "OLDPWD"]) {
      const after = steeringEnvDigest({ ...BASE_ENV, [key]: "changed-value" });
      expect(after).toBe(before);
    }
  });

  it("changes the digest when any steering key changes", () => {
    const before = steeringEnvDigest(BASE_ENV);
    for (const key of ["NODE_OPTIONS", "CI", "PNPM_HOME", "NODE_ENV"]) {
      const after = steeringEnvDigest({ ...BASE_ENV, [key]: "changed-value" });
      expect(after).not.toBe(before);
    }
  });

  it("excludes volatile keys from STEERING_ENV_KEYS and from the digest", () => {
    for (const key of VOLATILE_ENV_KEYS) {
      expect(STEERING_ENV_KEYS).not.toContain(key);
    }
    const before = steeringEnvDigest(BASE_ENV);
    const after = steeringEnvDigest({
      ...BASE_ENV,
      VITEST_WORKER_ID: "1",
      VITEST_POOL_ID: "2",
      TINYPOOL_WORKER_ID: "3",
    });
    expect(after).toBe(before);
  });

  it("treats <unset> as different from the empty string", () => {
    const unset = steeringEnvDigest({});
    const empty = steeringEnvDigest({ NODE_ENV: "" });
    expect(unset).not.toBe(empty);
  });

  it("reports every steering and volatile key, and nothing else", () => {
    for (const key of VOLATILE_ENV_KEYS) {
      expect(REPORTED_ENV_KEYS).toContain(key);
    }
    expect(STEERING_ENV_KEYS).toEqual(
      REPORTED_ENV_KEYS.filter((key) => !VOLATILE_ENV_KEYS.has(key)),
    );
    expect(REPORTED_ENV_KEYS).toHaveLength(STEERING_ENV_KEYS.length + VOLATILE_ENV_KEYS.size);
  });

  it("serializes the steering keys in their declared order (the digest's contract)", () => {
    // Pinned so a reorder -- which silently changes every future `env=`
    // token -- cannot pass as a no-op refactor.
    expect(STEERING_ENV_KEYS).toEqual([
      "npm_execpath",
      "npm_config_user_agent",
      "NODE_OPTIONS",
      "NODE_ENV",
      "CI",
      "PNPM_HOME",
      "TMPDIR",
    ]);
    expect(steeringEnvDigest(BASE_ENV)).toBe(
      digestOf(STEERING_ENV_KEYS.map((key) => `${key}=${BASE_ENV[key]}`).join("\n")),
    );
  });

  it("returns a sha256: digest with 16 lowercase hex chars", () => {
    const digest = digestOf("some text");
    expect(digest).toMatch(/^sha256:[0-9a-f]{16}$/);
  });
});

describe("env identity contract version (#2933)", () => {
  it("is v1 today -- the contract the unversioned records were emitted under", () => {
    // Pinned on purpose: bumping it is the deliberate act that splits the
    // population, and the key-order test above pins the other half of what
    // v1 means. Change both together, never one.
    expect(ENV_IDENTITY_VERSION).toBe(1);
  });

  it("stamps the live version onto the live digest as the emitted token", () => {
    const token = steeringEnvIdentity(BASE_ENV);
    expect(token).toMatch(/^v1:sha256:[0-9a-f]{16}$/);
    expect(token).toBe(envIdentityToken(ENV_IDENTITY_VERSION, steeringEnvDigest(BASE_ENV)));
  });

  it("reads an unversioned token as v1 and canonicalises it to the same form as an explicit v1", () => {
    const legacy = parseEnvIdentity("sha256:ed62f5285936a0ca");
    const explicit = parseEnvIdentity("v1:sha256:ed62f5285936a0ca");
    expect(legacy).toEqual({
      version: 1,
      digest: "sha256:ed62f5285936a0ca",
      token: "v1:sha256:ed62f5285936a0ca",
    });
    expect(explicit).toEqual(legacy);
  });

  it("reads a higher version as its own cohort", () => {
    expect(parseEnvIdentity("v2:sha256:ed62f5285936a0ca")).toEqual({
      version: 2,
      digest: "sha256:ed62f5285936a0ca",
      token: "v2:sha256:ed62f5285936a0ca",
    });
    expect(parseEnvIdentity("v12:x").version).toBe(12);
  });

  it("never throws: a token that is not a well-formed v<N>: prefix is an opaque v1 value", () => {
    // `v0:` and a zero-padded `v01:` are not versions this contract ever
    // stamps, and a bare fixture value like `c` is what the summarizer's own
    // tests feed it. All of them must surface as (v1) drift in a report,
    // never as an exit-64 parse failure that discards the samples beside them.
    for (const raw of ["c", "v0:sha256:abc", "v01:sha256:abc", "v:sha256:abc", "v2"]) {
      expect(parseEnvIdentity(raw)).toEqual({ version: 1, digest: raw, token: `v1:${raw}` });
    }
  });

  it("a STEERING_ENV_KEYS change is a version bump, not a date epoch", () => {
    // Stage the next contract beside the current one: one more steering key
    // and the version bumped. The v1 token is byte-identical to before (the
    // old contract is untouched), the v2 token carries its own version, and
    // the two parse into different cohorts -- which is all the summarizer
    // needs to keep them apart, whatever date either was emitted on.
    const nextKeys = [...STEERING_ENV_KEYS, "ZFB_NEXT_STEERING_KEY"];
    const nextVersion = ENV_IDENTITY_VERSION + 1;
    const v1 = steeringEnvIdentity(BASE_ENV);
    const v2 = steeringEnvIdentity(BASE_ENV, { keys: nextKeys, version: nextVersion });

    expect(v1).toBe(envIdentityToken(1, steeringEnvDigest(BASE_ENV)));
    expect(v2).toMatch(/^v2:sha256:[0-9a-f]{16}$/);
    expect(parseEnvIdentity(v1).version).toBe(1);
    expect(parseEnvIdentity(v2).version).toBe(nextVersion);
    // The new key steers the v2 digest and leaves v1's alone.
    const v2Changed = steeringEnvIdentity(
      { ...BASE_ENV, ZFB_NEXT_STEERING_KEY: "on" },
      { keys: nextKeys, version: nextVersion },
    );
    expect(v2Changed).not.toBe(v2);
    expect(steeringEnvIdentity({ ...BASE_ENV, ZFB_NEXT_STEERING_KEY: "on" })).toBe(v1);
  });
});
