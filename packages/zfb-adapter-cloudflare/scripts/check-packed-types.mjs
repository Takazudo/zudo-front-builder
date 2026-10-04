import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { resolvePackageTsc } from "../../../scripts/package-tsc.mjs";

const packageDir = dirname(dirname(fileURLToPath(import.meta.url)));
const packageJson = JSON.parse(await readFile(join(packageDir, "package.json"), "utf8"));
// The package's own compiler, plus the TS 5.9 consumer floor the published declarations keep
// supporting while contributor builds move to newer compilers (#3544).
const compilers = [resolvePackageTsc(packageDir), resolvePackageTsc(packageDir, "typescript-5.9")];
const tempDir = await mkdtemp(join(tmpdir(), "zfb-adapter-cloudflare-packed-types-"));
const tarballDir = join(tempDir, "tarballs");
const installDir = join(tempDir, "node_modules", "@takazudo", "zfb-adapter-cloudflare");

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: packageDir,
    encoding: "utf8",
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed with ${result.status === null ? `signal ${result.signal}` : `exit ${result.status}`}:\n${result.stdout ?? ""}${result.stderr ?? ""}`,
    );
  }
  return result.stdout;
}

try {
  await mkdir(tarballDir, { recursive: true });
  await mkdir(installDir, { recursive: true });

  // pnpm pack applies publishConfig's dist entry points to package.json, so
  // TypeScript resolves the same declarations consumers get after publish.
  run("pnpm", ["pack", "--pack-destination", tarballDir]);
  const tarballName = `${packageJson.name.replace(/^@/, "").replace("/", "-")}-${packageJson.version}.tgz`;
  const tarball = join(tarballDir, tarballName);
  run("tar", ["-xzf", tarball, "-C", installDir, "--strip-components=1"]);

  const consumer = await readFile(join(packageDir, "src/__tests__/types/consumer.ts"), "utf8");
  const consumerPath = join(tempDir, "consumer.ts");
  const configPath = join(tempDir, "tsconfig.json");
  await writeFile(consumerPath, consumer);
  await writeFile(
    configPath,
    JSON.stringify(
      {
        compilerOptions: {
          strict: true,
          noEmit: true,
          target: "ES2022",
          module: "ESNext",
          moduleResolution: "Bundler",
          lib: ["ES2022", "DOM"],
          types: [],
          skipLibCheck: true,
        },
        files: ["./consumer.ts"],
      },
      null,
      2,
    ),
  );

  for (const tsc of compilers) {
    run(tsc.command, [...tsc.args, "--project", configPath], { cwd: tempDir });
    process.stdout.write(
      `Packed adapter consumer declarations passed strict tsc ${tsc.version}.\n`,
    );
  }
} finally {
  await rm(tempDir, { recursive: true, force: true });
}
