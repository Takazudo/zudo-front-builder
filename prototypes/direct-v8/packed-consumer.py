#!/usr/bin/env python3
"""Packed clean-room consumer for one platform carrier, on any host (#4302 packaging row).

The cross-platform counterpart of scripts/smoke-packed-clean-room.sh, which is
the Linux x86_64 required check and stays unchanged. Run from the repository
root after `pnpm install` and the JS package builds. It injects the given CLI
binary and CSS addon into the carrier package, packs the carrier with `npm pack`
(keeping the POSIX exec bit, asserted where it applies) and the other packages
with `pnpm pack`, installs create-zfb from its tarball in a temporary directory
outside the workspace with every @takazudo package overridden to its local
tarball, scaffolds the default site, installs and builds it, and checks dist/.

`--rolldown` builds the scaffolded site with ZFB_ROLLDOWN_PROTOTYPE=1 (the binary
must carry the rolldown-prototype feature) and ZFB_DEV_TIMING=1, and requires the
backend's own timing line in the build output as proof that Rolldown bundled it;
without it an inherited ZFB_ROLLDOWN_PROTOTYPE is removed.

The injected files are removed again afterwards. Exit 0 PASS, 1 FAIL.

Usage:
  python3 prototypes/direct-v8/packed-consumer.py --carrier win32-x64-msvc \
    --binary target/release/zfb.exe --css-addon packages/zfb-win32-x64-msvc/zfb-css.node \
    --output target/direct-v8-packed
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]
NON_CARRIER_PACKAGES = ["zfb-slugify", "zfb", "zfb-runtime", "zfb-adapter-cloudflare", "create-zfb"]
CLEAN_ROOM_OVERRIDES = ["zfb", "zfb-slugify", "zfb-runtime", "zfb-adapter-cloudflare"]
CONSUMER_OVERRIDES = ["zfb", "zfb-slugify", "zfb-runtime"]
SITE_NAME = "packed-e2e-site"
# Printed by crates/zfb-rolldown-prototype/src/native.rs when ZFB_DEV_TIMING is set.
ROLLDOWN_TIMING_LINE = "[zfb-timing] rolldown-prototype:"


def tool(name):
    found = shutil.which(name)
    if not found:
        raise AssertionError(f"{name} is not on PATH")
    return found


def run(log, command, cwd, env=None, combined=False):
    log.write(f"$ {' '.join(map(str, command))}  (in {cwd})\n")
    log.flush()
    # stdout stays separate: callers parse its last line, and npm/pnpm notices go to stderr.
    result = subprocess.run([str(part) for part in command], cwd=cwd, env=env, capture_output=True, text=True,
                            errors="replace")
    log.write(result.stdout)
    log.write(result.stderr)
    log.flush()
    assert result.returncode == 0, f"{Path(command[0]).name} {command[1] if len(command) > 1 else ''} exit {result.returncode}"
    return result.stdout + result.stderr if combined else result.stdout


def file_spec(path):
    return "file:" + Path(path).resolve().as_posix()


def write_overrides(directory, tarballs, names):
    lines = ["minimumReleaseAge: 0", "overrides:"]
    lines += [f'  "{name}": "{file_spec(tarballs[name])}"' for name in names]
    (directory / "pnpm-workspace.yaml").write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--carrier", required=True, help="Platform package suffix, e.g. win32-x64-msvc")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--css-addon", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="New directory for the report and log")
    parser.add_argument("--rolldown", action="store_true", help="Build the site with the Rolldown prototype")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    carrier_dir = ROOT / "packages" / f"zfb-{args.carrier}"
    report = {"carrier": args.carrier, "binary": str(args.binary.resolve()), "rolldown_prototype": args.rolldown,
              "rows": [], "passed": False}
    build_env = {key: value for key, value in os.environ.items() if key != "ZFB_ROLLDOWN_PROTOTYPE"}
    if args.rolldown:
        build_env.update(ZFB_ROLLDOWN_PROTOTYPE="1", ZFB_DEV_TIMING="1")
    injected, preexisting, workdir = [], {}, None
    with (output / "packed-consumer.log").open("w") as log:
        try:
            manifest = json.loads((carrier_dir / "package.json").read_text())
            carrier_name = report["carrier"] = manifest["name"]
            executable = manifest["main"]
            injected = [carrier_dir / executable, carrier_dir / "zfb-css.node"]
            # Snapshot pre-existing injected files so cleanup never deletes a caller's own build output.
            preexisting = {path: path.read_bytes() for path in injected if path.is_file()}
            npm, pnpm, node = tool("npm"), tool("pnpm"), tool("node")
            workdir = Path(tempfile.mkdtemp(prefix="zfb-packed-consumer-"))
            addon = args.css_addon.resolve()
            if addon != (carrier_dir / "zfb-css.node").resolve():
                shutil.copyfile(addon, carrier_dir / "zfb-css.node")
            shutil.copyfile(args.binary, carrier_dir / executable)
            if os.name == "posix":
                os.chmod(carrier_dir / executable, 0o755)
            run(log, [node, "scripts/smoke-css-node.mjs", carrier_dir / "zfb-css.node"], ROOT)
            report["rows"].append({"name": "css-addon-loads", "passed": True})

            tarballs_dir = workdir / "tarballs"
            tarballs_dir.mkdir()
            # npm pack, not pnpm pack: pnpm pack normalizes file modes and would strip the exec bit.
            name = run(log, [npm, "pack", "--pack-destination", tarballs_dir], carrier_dir).strip().splitlines()[-1]
            tarballs = {carrier_name: tarballs_dir / name}
            with tarfile.open(tarballs[carrier_name]) as archive:
                member = archive.getmember(f"package/{executable}")
                addon_member = archive.getmember("package/zfb-css.node")
            assert member.size == args.binary.stat().st_size, "packed binary size differs from the injected binary"
            assert addon_member.size > 0, "packed CSS addon is empty"
            if os.name == "posix":
                assert member.mode & 0o777 == 0o755, f"packed binary mode {oct(member.mode)} is not 0755"
            report["rows"].append({"name": "carrier-packed", "mode": oct(member.mode & 0o777), "passed": True})

            for package in NON_CARRIER_PACKAGES:
                out = run(log, [pnpm, "pack", "--pack-destination", tarballs_dir], ROOT / "packages" / package)
                path = Path(out.strip().splitlines()[-1])
                assert path.is_file(), f"pnpm pack produced no tarball for {package}"
                tarballs[f"@takazudo/{package}" if package != "create-zfb" else package] = path
            report["rows"].append({"name": "packages-packed", "passed": True})

            room = workdir / "clean-room"
            room.mkdir()
            (room / "package.json").write_text(json.dumps({
                "name": "zfb-packed-clean-room", "private": True, "version": "0.0.0",
                "dependencies": {"create-zfb": file_spec(tarballs["create-zfb"])}}, indent=2) + "\n")
            write_overrides(room, tarballs, [f"@takazudo/{name}" for name in CLEAN_ROOM_OVERRIDES] + [carrier_name])
            run(log, [pnpm, "install"], room)
            installed = list((room / "node_modules").rglob(executable))
            installed = [path for path in installed if path.parent.name == carrier_name.split("/")[1]]
            assert installed, f"{carrier_name} (override) did not install the platform binary"
            if os.name == "posix":
                assert installed[0].stat().st_mode & 0o777 == 0o755, "installed platform binary lost its exec bit"
            report["rows"].append({"name": "clean-room-install", "installed": str(installed[0]), "passed": True})

            run(log, [node, room / "node_modules/create-zfb/bin/create-zfb.mjs", SITE_NAME], room)
            site = room / SITE_NAME
            assert (site / "package.json").is_file(), "scaffold did not produce package.json"
            report["rows"].append({"name": "scaffold", "passed": True})

            # create-zfb's own best-effort install targets the registry version; start clean.
            shutil.rmtree(site / "node_modules", ignore_errors=True)
            (site / "pnpm-lock.yaml").unlink(missing_ok=True)
            write_overrides(site, tarballs, [f"@takazudo/{name}" for name in CONSUMER_OVERRIDES] + [carrier_name])
            run(log, [pnpm, "install"], site)
            build_output = run(log, [pnpm, "build"], site, env=build_env, combined=True)
            index = site / "dist/index.html"
            assert index.is_file(), "dist/index.html not found after build"
            html = index.read_text(errors="replace")
            for marker in ["basic-blog", "Hello, zfb"]:
                assert marker in html, f"dist/index.html does not contain {marker!r}"
            report["rows"].append({"name": "site-build", "passed": True})
            if args.rolldown:
                assert ROLLDOWN_TIMING_LINE in build_output, "the site build printed no Rolldown backend timing line"
                report["rows"].append({"name": "rolldown-backend-observed", "passed": True})
            report["passed"] = True
        except Exception as error:
            report["error"] = f"{type(error).__name__}: {error}"
        finally:
            for path in injected:
                if path in preexisting:
                    path.write_bytes(preexisting[path])
                else:
                    path.unlink(missing_ok=True)
            if workdir:
                shutil.rmtree(workdir, ignore_errors=True)
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
