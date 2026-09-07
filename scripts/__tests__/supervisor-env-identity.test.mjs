import { describe, expect, it } from "vitest";
import {
  REPORTED_ENV_KEYS,
  STEERING_ENV_KEYS,
  VOLATILE_ENV_KEYS,
  digestOf,
  steeringEnvDigest,
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
