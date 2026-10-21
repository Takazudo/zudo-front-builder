#!/usr/bin/env python3
"""#4347 regression: plugin `addAlias` from a node_modules importer, through the actual CLI.

A config plugin registers `addAlias("@case/stub", "./src/stub.ts")`. The SSR page
imports a module resident under a `node_modules` directory, and that module
imports `@case/stub`. esbuild does not apply tsconfig `paths` there, so the
alias only resolves through the `--alias` flag the SSR bundler emits for a
target inside the stage root (#3002; `bundle.exclude` is set to a pattern that
matches nothing, which stages every alias target).

On Windows the SSR shadow used to be a `\\\\?\\` verbatim path, and the build
failed with `Could not resolve "@case/stub"` from the `//?/C:/…` importer
(#4346, run 39465472632). #4360 gave the shadow parent its ordinary drive spelling.

The importer lives in `vendor/node_modules/`, not `<project>/node_modules/`, so
the project has no `node_modules` of its own and the build uses the binary's
embedded @takazudo packages. Any inherited ZFB_ROLLDOWN_PROTOTYPE is removed.
Exit 0 PASS, 1 FAIL; the report and build logs stay under --output.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

MARKER = "CASE_ALIAS_FROM_NODE_MODULES"
FILES = {
    "zfb.config.ts": '''export default {
  outDir: "dist",
  bundle: { exclude: ["never-matches/**"] },
  plugins: [{ name: "./case-plugin.mjs" }],
};
''',
    "case-plugin.mjs": '''export default {
  name: "zfb-cli-case-alias-plugin",
  setup({ addAlias }) {
    addAlias("@case/stub", "./src/stub.ts");
  },
};
''',
    "src/stub.ts": f'export default "{MARKER}";\n',
    "vendor/node_modules/@scope/preset/package.json": '{ "name": "@scope/preset", "version": "0.0.0" }\n',
    "vendor/node_modules/@scope/preset/chrome.ts": '''import marker from "@case/stub";

export function chrome() {
  return "chrome:" + marker;
}
''',
    "pages/index.tsx": '''import { chrome } from "../vendor/node_modules/@scope/preset/chrome.ts";

export default function AliasPage() {
  return (
    <html><body>
      <p data-case="alias">{chrome()}</p>
    </body></html>
  );
}
''',
}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="New directory for the report and logs")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"binary": str(args.binary.resolve()), "passed": False}
    env = {key: value for key, value in os.environ.items() if key != "ZFB_ROLLDOWN_PROTOTYPE"}
    try:
        with tempfile.TemporaryDirectory(prefix="zfb-alias-case-") as directory:
            root = Path(directory)
            for relative, content in FILES.items():
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(content)
            with (output / "build.stdout").open("wb") as out, (output / "build.stderr").open("wb") as err:
                code = subprocess.run([str(args.binary.resolve()), "build"], cwd=root, env=env,
                                      stdout=out, stderr=err, timeout=300, check=False).returncode
            report["exit"] = code
            assert code == 0, f"build exit {code} (see build.stderr)"
            page = (root / "dist/index.html").read_text(errors="replace")
            assert "chrome:" + MARKER in page, "SSR page lacks the aliased value"
        # Only after the site directory is cleaned up, so a cleanup error is never a PASS.
        report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
