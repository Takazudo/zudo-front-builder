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

Stage 7 of the smoke script is ported too (`linked-build-parity`): a pristine
copy of the scaffold is built a second time against the same packed packages,
extracted into a small pnpm workspace and `link:`ed into the site, and the two
dist/ trees must be byte-identical (scripts/assert-dist-byte-parity.sh semantics).

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
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
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
# Stage 7 (smoke-packed-clean-room.sh): extracted workspace members, and the two the site `link:`s.
LINKED_WORKSPACE_PACKAGES = ["zfb", "zfb-runtime", "zfb-slugify"]
LINKED_SITE_PACKAGES = ["zfb", "zfb-runtime"]
REQUIRED_DIST_GLOBS = ["assets/islands-*.js", "index.html"]
BUILD_TOKEN = re.compile(rb'(?:data-zfb-build="|build:")([0-9a-f]{16})')


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


def is_link(path):
    """A symlink, or on Windows any reparse point (pnpm links directories with junctions there)."""
    if os.path.islink(path):
        return True
    attributes = getattr(os.lstat(path), "st_file_attributes", 0)
    return bool(attributes & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0))


def assert_links_to(entry, target):
    """`entry` must be a link (symlink or junction) to the directory `target`; returns its resolved path.

    Identity is os.path.samefile, so Windows verbatim-prefixed and 8.3 short-name spellings (#4499)
    cannot mismatch; the resolved paths are for the diagnostic only.
    """
    entry, target = Path(entry), Path(target)
    assert os.path.lexists(entry), f"{entry} does not exist"
    assert is_link(entry), f"{entry} is not a symlink or junction"
    resolved = entry.resolve(strict=True)
    assert os.path.samefile(entry, target), f"{entry} resolves to {resolved}, expected {target.resolve(strict=True)}"
    return resolved


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def dist_manifest(root):
    """{relative `/` path: sha256} of every regular file under `root`; any link or non-regular entry fails."""
    root = Path(root)
    assert root.is_dir() and not is_link(root), f"not a directory: {root}"

    def unreadable(error):
        raise error

    manifest = {}
    for dirpath, dirnames, filenames in os.walk(root, onerror=unreadable):
        for name in sorted(dirnames + filenames):
            path = Path(dirpath) / name
            relative = path.relative_to(root).as_posix()
            assert not is_link(path), f"{root}: dist entry is a symlink or junction: {relative}"
            if name in filenames:
                assert stat.S_ISREG(os.lstat(path).st_mode), f"{root}: dist entry is not a regular file: {relative}"
                manifest[relative] = sha256_file(path)
    return manifest


def build_tokens(root, manifest):
    tokens = {}
    for relative in manifest:
        for token in BUILD_TOKEN.findall((Path(root) / relative).read_bytes()):
            tokens[token.decode()] = tokens.get(token.decode(), 0) + 1
    return tokens


def assert_dist_parity(first, second, required, log):
    """Python port of scripts/assert-dist-byte-parity.sh: same relative file set, same sha256 per file.

    Stricter than the script on one point: any symlink or junction in either tree fails outright,
    where the script compares symlink targets.

    Every `required` glob (fnmatch, so `*` also crosses `/`, as bash `[[ == ]]` does) must match a
    file, and an empty tree fails. Differences are written to `log` before raising.
    """
    manifest_a, manifest_b = dist_manifest(first), dist_manifest(second)
    assert manifest_a, f"{first} contains no files"
    for glob in required:
        assert any(fnmatch.fnmatchcase(path, glob) for path in manifest_a), \
            f"no file in {first} matches required glob: {glob}"
    missing = sorted(set(manifest_a) - set(manifest_b))
    extra = sorted(set(manifest_b) - set(manifest_a))
    mismatched = sorted(path for path in set(manifest_a) & set(manifest_b) if manifest_a[path] != manifest_b[path])
    if missing or extra or mismatched:
        log.write(f"[FAIL] {first} and {second} differ:\n")
        log.writelines(f"  missing from second: {path} {manifest_a[path]}\n" for path in missing)
        log.writelines(f"  extra in second: {path} {manifest_b[path]}\n" for path in extra)
        log.writelines(f"  mismatched: {path} {manifest_a[path]} != {manifest_b[path]}\n" for path in mismatched)
        for root, manifest in [(first, manifest_a), (second, manifest_b)]:
            log.write(f"  build tokens in {root}: {build_tokens(root, manifest) or '<none>'}\n")
        log.flush()
        raise AssertionError(f"dist trees differ: {len(missing)} missing, {len(extra)} extra, "
                             f"{len(mismatched)} mismatched (see the log)")
    islands = sorted(path for path in manifest_a if fnmatch.fnmatchcase(path, "assets/islands-*.js"))
    log.write(f"[PASS] {first} and {second} are byte-identical ({len(manifest_a)} files)\n")
    log.writelines(f"  {manifest_a[path]}  {path}\n" for path in islands)
    log.flush()
    return {"files": len(manifest_a), "islands": islands}


def required_rows(rolldown):
    rows = ["css-addon-loads", "carrier-packed", "packages-packed", "clean-room-install", "scaffold", "site-build"]
    return rows + (["rolldown-backend-observed"] if rolldown else []) + ["linked-build-parity"]


def assert_required_rows(report):
    recorded = [row["name"] for row in report["rows"] if row.get("passed") is True]
    missing = [name for name in required_rows(report["rolldown_prototype"]) if name not in recorded]
    assert not missing, f"required rows missing: {', '.join(missing)}"


KEPT_TEMPDIR = re.compile(r"kept (.+?) tempdir at (.+?) \(cleanup is manual\)")


def describe_link(path):
    """Reparse attributes/tag, readlink, realpath and (Windows) `fsutil reparsepoint query` of one entry."""
    path = Path(path)
    lines = [f"  entry {path}"]
    try:
        info = os.lstat(path)
        attributes = getattr(info, "st_file_attributes", None)
        tag = getattr(info, "st_reparse_tag", None)
        lines.append(f"    lstat mode={oct(info.st_mode)} attributes={attributes if attributes is None else hex(attributes)}"
                     f" reparse_tag={tag if tag is None else hex(tag)} is_link={is_link(path)}")
    except OSError as error:
        lines.append(f"    lstat error: {error}")
    for label, probe in [("readlink", os.readlink), ("realpath", os.path.realpath)]:
        try:
            lines.append(f"    {label}: {probe(path)}")
        except OSError as error:
            lines.append(f"    {label} error: {error}")
    if os.name == "nt":
        result = subprocess.run(["fsutil", "reparsepoint", "query", str(path)], capture_output=True, text=True,
                                errors="replace")
        lines.append(f"    fsutil reparsepoint query (exit {result.returncode}):")
        lines += [f"      {line}" for line in (result.stdout + result.stderr).splitlines()]
    return "\n".join(lines) + "\n"


def list_entries(directory):
    directory = Path(directory)
    lines = [f"  listing {directory}"]
    try:
        for entry in sorted(os.scandir(directory), key=lambda entry: entry.name):
            kind = "link" if is_link(entry.path) else "dir" if entry.is_dir(follow_symlinks=False) else "file"
            lines.append(f"    {kind:4} {entry.name}")
    except OSError as error:
        lines.append(f"    error: {error}")
    return "\n".join(lines) + "\n"


def linked_build_diagnostics(log, pnpm, linked_site, packages_ws, build_env):
    """Temporary #4545 evidence, collected only after the linked build already failed; never changes the verdict.

    Records how pnpm wrote the out-of-root `link:` entries, then reruns the build with ZFB_KEEP_BUILD_SHADOW=1
    and records the kept shadow's node_modules link plus a verbose esbuild resolve probe run inside it. Kept
    directories are left for the ephemeral runner: they hold junctions into the live site.
    """
    def section(title, action):
        log.write(f"--- [#4545] {title} ---\n")
        try:
            action()
        except Exception as error:
            log.write(f"  (diagnostic failed: {type(error).__name__}: {error})\n")
        log.flush()

    def site_links():
        log.write(list_entries(linked_site / "node_modules"))
        log.write(list_entries(linked_site / "node_modules/@takazudo"))
        for package in LINKED_SITE_PACKAGES:
            log.write(describe_link(linked_site / "node_modules/@takazudo" / package))

    def workspace_links():
        for package in LINKED_SITE_PACKAGES:
            scope = packages_ws / package / "node_modules/@takazudo"
            if scope.is_dir():
                log.write(list_entries(scope))
                for entry in sorted(scope.iterdir()):
                    log.write(describe_link(entry))

    kept = []

    def rerun_keeping_shadow():
        env = dict(build_env, ZFB_KEEP_BUILD_SHADOW="1")
        result = subprocess.run([pnpm, "build"], cwd=linked_site, env=env, capture_output=True, text=True,
                                errors="replace")
        output = result.stdout + result.stderr
        log.write(f"  pnpm build with ZFB_KEEP_BUILD_SHADOW=1 exit {result.returncode}\n{output}\n")
        kept.extend((role, Path(path)) for role, path in KEPT_TEMPDIR.findall(output))

    def kept_shadows():
        if not kept:
            log.write("  no kept tempdir was reported\n")
        for role, path in kept:
            log.write(f"  kept {role}: {path}\n")
            log.write(list_entries(path))
            if os.path.lexists(path / "node_modules"):
                log.write(describe_link(path / "node_modules"))
                log.write(list_entries(path / "node_modules/@takazudo"))
                for package in LINKED_SITE_PACKAGES:
                    log.write(describe_link(path / "node_modules/@takazudo" / package))

    def esbuild_probe():
        slot = ROOT / "crates/zfb/binaries/esbuild"
        binaries = [Path(os.environ["ZFB_ESBUILD_BIN"])] if os.environ.get("ZFB_ESBUILD_BIN") else []
        binaries += [path for path in slot.glob("esbuild*") if path.is_file() and path.stem == "esbuild"]
        shadows = [path for _, path in kept if os.path.lexists(path / "node_modules")]
        if not binaries or not shadows:
            log.write(f"  skipped: esbuild in {slot}: {binaries}; shadows with node_modules: {shadows}\n")
            return
        shadow = shadows[0]
        probe = shadow / "zfb-4545-probe.mjs"
        probe.write_text('import * as zfb from "@takazudo/zfb";\n'
                         'import * as server from "@takazudo/zfb-runtime/server";\n'
                         'console.log(zfb, server);\n')
        out_dir = Path(tempfile.mkdtemp(prefix="zfb-4545-probe-"))
        for extra in ([], ["--preserve-symlinks"]):
            command = [str(binaries[0]), "--bundle", "--platform=neutral", "--format=esm", "--log-level=verbose",
                       f"--outfile={out_dir / 'probe-out.mjs'}", *extra, probe.name]
            result = subprocess.run(command, cwd=shadow, capture_output=True, text=True, errors="replace")
            log.write(f"  $ {' '.join(command)}  (in {shadow})\n  exit {result.returncode}\n"
                      f"{result.stdout}{result.stderr}\n")
        shutil.rmtree(out_dir, ignore_errors=True)

    log.write("[#4545] the linked build failed; collecting link diagnostics (the row still fails)\n")
    section("site node_modules links", site_links)
    section("linked workspace package links", workspace_links)
    section("rerun with ZFB_KEEP_BUILD_SHADOW=1", rerun_keeping_shadow)
    section("kept shadow links", kept_shadows)
    section("verbose esbuild probe in the kept shadow", esbuild_probe)


def extract_package(tarball, destination):
    """`tar -xzf <tarball> -C <destination> --strip-components=1`."""
    destination.mkdir(parents=True)
    with tarfile.open(tarball) as archive:
        members = []
        for member in archive.getmembers():
            stripped = member.name.split("/", 1)[1] if "/" in member.name else ""
            if stripped:
                member.name = stripped
                members.append(member)
        extra = {"filter": "data"} if hasattr(tarfile, "data_filter") else {}
        archive.extractall(destination, members=members, **extra)


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
    with (output / "packed-consumer.log").open("w", encoding="utf-8", errors="replace") as log:
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

            # Stage 7 snapshot: the pristine scaffold, before any install or build output exists.
            linked_room = workdir / "linked-room"
            linked_site = linked_room / SITE_NAME
            shutil.copytree(site, linked_site, symlinks=True, ignore=lambda directory, names: [
                name for name in names
                if Path(directory) == site and name in ("node_modules", "pnpm-lock.yaml")])

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

            # Stage 7: the same packed contents, extracted into a pnpm workspace and `link:`ed into the
            # snapshot, must emit byte-identical dist/ (#4231). The site stays outside that workspace.
            carrier_dir_name = carrier_name.split("/")[1]
            packages_ws = linked_room / "packages-ws"
            for package in LINKED_WORKSPACE_PACKAGES:
                extract_package(tarballs[f"@takazudo/{package}"], packages_ws / package)
            extract_package(tarballs[carrier_name], packages_ws / carrier_dir_name)
            (packages_ws / "package.json").write_text(json.dumps({
                "name": "zfb-linked-packages", "private": True, "version": "0.0.0",
                "dependencies": {"@takazudo/zfb": "workspace:*"}}, indent=2) + "\n")
            members = LINKED_WORKSPACE_PACKAGES + [carrier_dir_name]
            (packages_ws / "pnpm-workspace.yaml").write_text("\n".join(
                ["packages:"] + [f'  - "{member}"' for member in members]
                + ["linkWorkspacePackages: true", "minimumReleaseAge: 0", "overrides:"]
                + [f'  "@takazudo/{member}": "workspace:*"' for member in members]) + "\n")
            run(log, [pnpm, "install", "--prod"], packages_ws)
            (linked_site / "pnpm-workspace.yaml").write_text("\n".join(
                ["minimumReleaseAge: 0", "overrides:"]
                + [f'  "@takazudo/{package}": "link:{(packages_ws / package).resolve().as_posix()}"'
                   for package in LINKED_SITE_PACKAGES]) + "\n")
            run(log, [pnpm, "install"], linked_site)
            links = {}
            for package in LINKED_SITE_PACKAGES:
                resolved = assert_links_to(linked_site / "node_modules/@takazudo" / package, packages_ws / package)
                links[package] = str(resolved)
                log.write(f"linked @takazudo/{package} -> {resolved}\n")
            linked_binary = packages_ws / "zfb/node_modules/@takazudo" / carrier_dir_name / executable
            assert linked_binary.is_file(), f"linked zfb does not reach the platform binary at {linked_binary}"
            assert sha256_file(linked_binary) == sha256_file(args.binary), \
                f"linked zfb does not reach the injected binary (got {linked_binary.resolve()})"
            try:
                linked_output = run(log, [pnpm, "build"], linked_site, env=build_env, combined=True)
            except AssertionError:
                linked_build_diagnostics(log, pnpm, linked_site, packages_ws, build_env)
                raise
            if args.rolldown:
                assert ROLLDOWN_TIMING_LINE in linked_output, "the linked build printed no Rolldown backend timing line"
            parity = assert_dist_parity(site / "dist", linked_site / "dist", REQUIRED_DIST_GLOBS, log)
            report["rows"].append({"name": "linked-build-parity", "links": links,
                                   "binary": str(linked_binary.resolve()), **parity, "passed": True})
            assert_required_rows(report)
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
