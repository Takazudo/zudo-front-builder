#!/usr/bin/env python3
"""Read-only source/packed-byte checks plus disposable carrier extraction.

This never calls a registry, publisher, release API or native builder. Builds
and actual Node 22 execution belong to the caller's native hosted row.
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tarfile

VERSION = "4.4.0"
CARRIERS = ("darwin-arm64", "darwin-x64", "linux-arm64-gnu", "linux-x64-gnu", "win32-x64-msvc")


def digest(value):
    return hashlib.sha256(value).hexdigest()


def save(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + "\n")


def tracked_snapshot():
    names = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\0")[:-1]
    return {name: digest(os.readlink(name).encode() if Path(name).is_symlink() else Path(name).read_bytes()) for name in sorted(names)}


def checked_tar(path):
    """Validate paths/types/duplicates before extracting any bytes."""
    result = {}
    with tarfile.open(path, "r:gz") as archive:
        for member in archive.getmembers():
            raw = member.name
            path = PurePosixPath(raw)
            # POSIX tar names are later joined by the host's native Path.
            # Reject Windows separators/drives even while validating on Linux.
            if "\\" in raw or path.is_absolute() or ".." in path.parts or any(re.match(r"^[A-Za-z]:", part) for part in path.parts):
                raise ValueError(f"unsafe archive path: {raw}")
            name = path.as_posix()
            if name in result or not (member.isfile() or member.isdir()):
                raise ValueError(f"duplicate or nonregular archive entry: {name}")
            result[name] = {
                "mode": member.mode & 0o777,
                "bytes": archive.extractfile(member).read() if member.isfile() else None,
            }
    return result


def checksum_file(directory, filename):
    path = Path(directory) / filename
    line = (Path(directory) / (filename + ".sha256")).read_text().strip()
    parts = line.split("  ")
    if len(parts) != 2 or parts[1] != filename or parts[0] != digest(path.read_bytes()):
        raise ValueError(f"checksum mismatch: {filename}")
    return parts[0]


def fast_mac_bytes(directory):
    # Same independent GNU-format archive/addon checksum validation as the
    # existing release ingestion, with sole-root-entry validation before staging.
    archive = f"zfb-{VERSION}-x86_64-apple-darwin.tar.gz"
    addon = f"zfb-css-{VERSION}-x86_64-apple-darwin.node"
    archive_hash = checksum_file(directory, archive)
    addon_hash = checksum_file(directory, addon)
    entries = checked_tar(Path(directory) / archive)
    if set(entries) != {"zfb"} or entries["zfb"]["bytes"] is None or entries["zfb"]["mode"] != 0o755:
        raise ValueError("fast-Mac archive must contain only a 0755 regular root zfb")
    return entries["zfb"]["bytes"], (Path(directory) / addon).read_bytes(), {
        "archive": archive_hash, "addon": addon_hash,
        "archiveChecksum": digest((Path(directory) / (archive + ".sha256")).read_bytes()),
        "addonChecksum": digest((Path(directory) / (addon + ".sha256")).read_bytes()),
    }


def verify_pack(tarball, carrier, cli, addon):
    if carrier not in CARRIERS:
        raise ValueError("unsupported carrier")
    entries = checked_tar(tarball)
    manifest = json.loads(entries["package/package.json"]["bytes"])
    if manifest["name"] != "@takazudo/zfb-" + carrier or manifest["version"] != VERSION:
        raise ValueError("carrier name/version mismatch")
    binary = "zfb.exe" if carrier.startswith("win32") else "zfb"
    expected = {binary: Path(cli).read_bytes(), "zfb-css.node": Path(addon).read_bytes()}
    modes = {}
    for name, contents in expected.items():
        entry = entries["package/" + name]
        if entry["bytes"] != contents:
            raise ValueError(f"packed native bytes differ: {name}")
        if name == "zfb-css.node" and entry["mode"] != 0o644:
            raise ValueError("packed addon mode must be 0644")
        # Windows execution is PE/Node loader evidence, not a POSIX exec bit.
        if name == "zfb" and entry["mode"] != 0o755:
            raise ValueError("packed Unix CLI mode must be 0755")
        modes[name] = oct(entry["mode"])
    return {"name": manifest["name"], "version": VERSION,
            "originalCLIPath": str(cli), "originalLibraryPath": str(addon),
            "tarball": digest(Path(tarball).read_bytes()), "modes": modes,
            "cli": digest(expected[binary]), "addon": digest(expected["zfb-css.node"])}


def stage_consumer(pack_directory, carrier, room):
    room = Path(room)
    room.mkdir(parents=True, exist_ok=False)
    hashes = {}
    for name in ("zfb", "zfb-slugify", "zfb-" + carrier):
        matches = list(Path(pack_directory).glob(f"takazudo-{name}-{VERSION}.tgz"))
        if len(matches) != 1:
            raise ValueError(f"missing/ambiguous packed {name}")
        entries = checked_tar(matches[0])
        manifest = json.loads(entries["package/package.json"]["bytes"])
        if manifest["name"] != "@takazudo/" + name or manifest["version"] != VERSION:
            raise ValueError("packed consumer manifest mismatch")
        destination = room / "node_modules" / "@takazudo" / name
        for filename, entry in entries.items():
            path = PurePosixPath(filename)
            if not path.parts or path.parts[0] != "package":
                raise ValueError("package tarball entry must be rooted at package/")
            target = destination.joinpath(*path.parts[1:])
            if entry["bytes"] is not None:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(entry["bytes"])
                target.chmod(entry["mode"])
        hashes[name] = digest(matches[0].read_bytes())
    sdk = json.loads((room / "node_modules/@takazudo/zfb/package.json").read_text())
    if sdk["exports"]["./css"]["default"] != "./dist/css.js":
        raise ValueError("packed public CSS export must resolve emitted dist/css.js")
    save(room / "package.json", {"private": True, "type": "module"})
    save(room / "packed-inputs.json", hashes)
    shutil.copyfile(Path(__file__).with_name("css-carrier-public-smoke.mjs"), room / "consumer.mjs")


def main(args):
    command, *values = args
    if command == "snapshot":
        save(values[0], tracked_snapshot())
    elif command == "assert-source":
        if tracked_snapshot() != json.loads(Path(values[0]).read_text()):
            raise ValueError("tracked source/lock/recipe bytes changed during verification")
    elif command == "pack":
        save(values[4], verify_pack(*values[:4]))
    elif command == "stage-consumer":
        stage_consumer(*values)
    elif command == "ingest":
        received, package, output = map(Path, values)
        cli, addon, hashes = fast_mac_bytes(received)  # all checks before staging
        shutil.copytree(package, output)
        (output / "zfb").write_bytes(cli)
        (output / "zfb").chmod(0o755)
        (output / "zfb-css.node").write_bytes(addon)
        (output / "zfb-css.node").chmod(0o644)
        save(output.parent / "ingestion.json", {**hashes, "stagedCLI": digest(cli), "stagedAddon": digest(addon)})
    elif command == "health-size-step":
        marker = "      - name: " + values[0] + "\n"
        text = Path(".github/workflows/health.yml").read_text()
        start = text.index(marker) + len(marker)
        block = text[start:text.find("\n      - ", start)]
        lines = block[block.index("        run: |\n") + len("        run: |\n"):].splitlines()
        script = []
        for line in lines:
            if line and not line.startswith("          "):
                break
            script.append(line[10:])
        if not script:
            raise ValueError("missing literal health size recipe")
        subprocess.run(["bash", "-e", "-o", "pipefail", "-c", "\n".join(script)], check=True)
    else:
        raise ValueError("unknown verification operation")


if __name__ == "__main__":
    main(sys.argv[1:])
