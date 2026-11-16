import { execFile, execFileSync } from "node:child_process";

export function gitFixtureArgs(repo, ...args) {
  return [
    "-C",
    repo,
    "-c",
    "user.email=t@example.invalid",
    "-c",
    "user.name=t",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "tag.gpgsign=false",
    "-c",
    "core.hooksPath=/dev/null",
    ...args,
  ];
}

export function gitFixtureSync(repo, ...args) {
  return execFileSync("git", gitFixtureArgs(repo, ...args), { encoding: "utf8" }).trim();
}

export function gitFixtureAsync(repo, ...args) {
  const command = gitFixtureArgs(repo, ...args);
  return new Promise((resolve, reject) => {
    execFile("git", command, { encoding: "utf8" }, (error, stdout, stderr) => {
      if (error) {
        reject(
          new Error(
            `git ${command.join(" ")} failed (status ${error.code ?? "unknown"}): ${stderr.trim() || error.message}`,
            { cause: error },
          ),
        );
      } else {
        resolve(stdout.trim());
      }
    });
  });
}
