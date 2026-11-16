import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, it } from "vitest";
import { gitFixtureAsync } from "./helpers/git-fixture.mjs";

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

it.each(["commit signing", "tag signing", "inherited hooks"])(
  "isolates real Git operations from %s",
  (hostile) => {
    const root = mkdtempSync(join(tmpdir(), "zfb-git-fixture-"));
    roots.push(root);
    const repo = join(root, "repo");
    mkdirSync(repo);
    const hooks = join(root, "hooks");
    mkdirSync(hooks);
    const hook = join(hooks, "pre-commit");
    writeFileSync(hook, "#!/bin/sh\necho hostile-hook >&2\nexit 27\n");
    chmodSync(hook, 0o755);
    const config = join(root, "global.gitconfig");
    writeFileSync(
      config,
      hostile === "inherited hooks"
        ? `[core]\n  hooksPath = ${hooks}\n`
        : `[${hostile === "tag signing" ? "tag" : "commit"}]\n  gpgsign = true\n[gpg]\n  format = ssh\n[gpg "ssh"]\n  program = /nonexistent/zfb-test-signer\n[user]\n  signingkey = bogus\n`,
    );
    const script = `
      import { spawnSync } from "node:child_process";
      import { writeFileSync } from "node:fs";
      import { gitFixtureSync } from ${JSON.stringify(new URL("./helpers/git-fixture.mjs", import.meta.url).href)};
      const repo = process.argv[1];
      gitFixtureSync(repo, "init", "-q");
      writeFileSync(repo + "/file", "fixture");
      gitFixtureSync(repo, "add", ".");
      if (${JSON.stringify(hostile)} === "tag signing") gitFixtureSync(repo, "commit", "-q", "-m", "base");
      const operation = ${JSON.stringify(hostile)} === "tag signing"
        ? ["tag", "-a", "fixture", "-m", "fixture"]
        : ["commit", "-m", "fixture"];
      const raw = spawnSync("git", ["-C", repo, "-c", "user.email=t@example.invalid", "-c", "user.name=t", ...operation], { encoding: "utf8" });
      gitFixtureSync(repo, ...operation);
      console.log(JSON.stringify({ rawStatus: raw.status, rawStderr: raw.stderr, sha: gitFixtureSync(repo, "rev-parse", ${JSON.stringify(hostile)} === "tag signing" ? "fixture^{commit}" : "HEAD") }));
    `;
    const output = execFileSync(process.execPath, ["--input-type=module", "-e", script, repo], {
      encoding: "utf8",
      env: { ...process.env, GIT_CONFIG_GLOBAL: config, GIT_CONFIG_NOSYSTEM: "1" },
    });
    const result = JSON.parse(output);
    expect(result.rawStatus).not.toBe(0);
    expect(result.rawStderr).toMatch(
      hostile === "inherited hooks" ? /hostile-hook/ : /sign|gpg|nonexistent/i,
    );
    expect(result.sha).toMatch(/^[a-f0-9]{40}$/);
  },
);

it("rejects a genuinely failing asynchronous Git command with diagnostic context", async () => {
  const repo = mkdtempSync(join(tmpdir(), "zfb-git-async-failure-"));
  roots.push(repo);
  await gitFixtureAsync(repo, "init", "-q");
  await expect(gitFixtureAsync(repo, "rev-parse", "refs/heads/missing")).rejects.toThrow(
    /git .*rev-parse refs\/heads\/missing failed \(status 128\):.*fatal:/s,
  );
});
