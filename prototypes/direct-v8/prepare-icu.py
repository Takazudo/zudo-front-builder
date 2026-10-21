#!/usr/bin/env python3
"""Refresh or verify the vendored ICU data, without a Deno Rust dependency.

`crates/zfb-js-host/icu/icudtl.dat` is the copy every direct-V8 build embeds.
--verify checks it against the pin without network access. Without --verify this
downloads the checksum-pinned data archive, never executes its Rust code, and
rewrites only that file from the known data member (a V8/ICU refresh).
Pass --archive to use an already fetched Cargo archive without network access.
"""
import argparse
import hashlib
import io
import os
from pathlib import Path
import tarfile
import tempfile
import urllib.request

ARCHIVE_SHA256 = "a9efff8990a82c1ae664292507e1a5c6749ddd2312898cdf9cd7cb1fd4bc64c6"
DATA_SHA256 = "1cf67874b5a87a8363a86fb3f81e3cbbed54d389062dab8fb52308d5cf8c8612"
URL = "https://static.crates.io/crates/deno_core_icudata/deno_core_icudata-0.77.0.crate"
VENDORED = Path("crates/zfb-js-host/icu/icudtl.dat")


def publish_verified_data(destination, data):
    """Publish complete verified bytes; failed writers never touch the destination."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent, prefix=".icudtl-", delete=False) as output:
            temporary = Path(output.name)
            output.write(data)
        # Check after close, including buffered writes, before making bytes visible.
        if hashlib.sha256(temporary.read_bytes()).hexdigest() != DATA_SHA256:
            raise ValueError("staged ICU data differs from the characterized baseline bytes")
        os.replace(temporary, destination)
    except BaseException as failure:
        if temporary is not None:
            try:
                temporary.unlink(missing_ok=True)
            except OSError as cleanup_failure:
                # Preserve the publication failure and expose cleanup failure too.
                raise failure from cleanup_failure
        raise


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--verify", action="store_true", help="check the vendored file against the pin; no network")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    destination = root / VENDORED
    if args.verify:
        if not destination.is_file():
            raise SystemExit(f"vendored ICU data {destination} is missing")
        if hashlib.sha256(destination.read_bytes()).hexdigest() != DATA_SHA256:
            raise SystemExit(f"vendored ICU data {destination} differs from the characterized baseline bytes")
        print(destination)
        return
    archive = args.archive.read_bytes() if args.archive else urllib.request.urlopen(URL, timeout=60).read()
    if hashlib.sha256(archive).hexdigest() != ARCHIVE_SHA256:
        raise SystemExit("ICU archive checksum differs from the pinned baseline Cargo.lock")
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as package:
        member = package.extractfile("deno_core_icudata-0.77.0/src/icudtl.dat")
        if member is None:
            raise SystemExit("pinned ICU data member is missing")
        data = member.read()
    if hashlib.sha256(data).hexdigest() != DATA_SHA256:
        raise SystemExit("ICU data differs from the characterized baseline bytes")
    publish_verified_data(destination, data)
    print(destination)


if __name__ == "__main__":
    main()
