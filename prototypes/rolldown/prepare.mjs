import { cpSync, mkdirSync, existsSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
const repo = resolve(import.meta.dirname, "../..");
const root = resolve(process.argv[2] ?? join(repo, "target/rolldown-prototype/project"));
const tools = resolve(process.env.ZFB_PROTOTYPE_TOOLS ?? join(repo, "target/rolldown-tools"));
mkdirSync(root, { recursive: true });
cpSync(join(import.meta.dirname, "fixture"), root, { recursive: true });
for (const dir of ["content", "layouts", "node_modules/@takazudo"])
  mkdirSync(join(root, dir), { recursive: true });
for (const [name, target] of [
  ["@takazudo/zfb", join(repo, "packages/zfb")],
  ["@takazudo/zfb-runtime", join(repo, "packages/zfb-runtime")],
  ["@takazudo/zfb-slugify", join(repo, "packages/zfb-slugify")],
  ["hono", join(tools, "node_modules/hono")],
]) {
  if (!existsSync(join(root, "node_modules", name)))
    cpSync(target, join(root, "node_modules", name), { recursive: true });
}
writeFileSync(
  join(root, "tsconfig.json"),
  JSON.stringify({
    compilerOptions: {
      jsx: "react-jsx",
      jsxImportSource: "@takazudo/zfb/zudo-react",
      baseUrl: ".",
      paths: { "@fixture/message": ["components/message.ts"] },
    },
  }),
);
console.log(root);
