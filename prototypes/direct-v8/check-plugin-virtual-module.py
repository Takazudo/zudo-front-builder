#!/usr/bin/env python3
"""#4343 regression: a config plugin's virtual module through the actual CLI.

Builds a minimal site whose `zfb.config.ts` plugin registers `virtual:case-data`
with `addVirtualModule`; the SSR page and a `*.client.ts` script both import it.
On Windows the SSR bundler's shadow directory is a `\\\\?\\` verbatim path, and the
build used to fail with `no in-memory source for file:///?/C:/…/.zfb-virtual-*.mjs`.
Any inherited ZFB_ROLLDOWN_PROTOTYPE is removed: this is the default bundler.
Exit 0 PASS, 1 FAIL; the report and build logs stay under --output.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

FILES = {
    "zfb.config.ts": 'export default { outDir: "dist", plugins: [{ name: "./case-plugin.mjs" }] };\n',
    "case-plugin.mjs": '''export default {
  name: "zfb-cli-case-plugin",
  setup({ addVirtualModule }) {
    addVirtualModule("virtual:case-data", () => `export const caseValue = "CASE_VIRTUAL_" + (40 + 2);`);
  },
};
''',
    "components/case.client.ts": '''import { caseValue } from "virtual:case-data";

const out = document.getElementById("case-client-out");
if (out) out.textContent = "CASE_CLIENT_RAN:" + caseValue;
''',
    "pages/index.tsx": '''import { clientScript } from "@takazudo/zfb";
import { caseValue } from "virtual:case-data";

export default function PluginPage() {
  return (
    <html><body>
      <p data-case="virtual">{caseValue}</p>
      <p id="case-client-out">CASE_CLIENT_PENDING</p>
      <script type="module" src={clientScript("case")}></script>
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
        with tempfile.TemporaryDirectory(prefix="zfb-plugin-case-") as directory:
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
            assert "CASE_VIRTUAL_42" in page, "SSR page lacks the virtual module value"
            scripts = sorted((root / "dist/assets/client").glob("case-*.js"))
            assert len(scripts) == 1, f"expected one hashed assets/client/case-*.js, found {len(scripts)}"
            assert "CASE_VIRTUAL_" in scripts[0].read_text(errors="replace"), "client script lacks the value"
            report["client_script"] = scripts[0].name
            report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
