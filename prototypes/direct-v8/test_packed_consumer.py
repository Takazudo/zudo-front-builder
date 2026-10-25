#!/usr/bin/env python3
"""Drift guard: packed-consumer.py packs and overrides what the required smoke script does,
plus unit tests of its Stage 7 dist comparator and link check (stdlib only, any OS)."""
import importlib.util
import io
import os
from pathlib import Path
import re
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parents[1] / "scripts/smoke-packed-clean-room.sh"
SPEC = importlib.util.spec_from_file_location("packed_consumer", HERE / "packed-consumer.py")
packed_consumer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packed_consumer)


def override_blocks(script):
    """The non-platform package names of each tarball (`file:`) `const overrides = {...}` block, in order.

    Stage 7's linked-directory block (`link:`) is checked separately by LinkedStageParity.
    """
    return [{name for name in re.findall(r'"(@takazudo/[a-z0-9-]+)":', block) if not name.endswith("-gnu")}
            for block in re.findall(r"const overrides = \{(.*?)\};", script, re.S)
            if "file:" in block]


class SmokeParity(unittest.TestCase):
    def setUp(self):
        self.script = SMOKE.read_text()

    def test_packs_the_same_non_carrier_packages(self):
        packed = re.findall(r"pnpm_pack packages/([a-z0-9-]+)\)", self.script)
        self.assertEqual(sorted(packed), sorted(packed_consumer.NON_CARRIER_PACKAGES))

    def test_overrides_match_the_clean_room_and_site_stages(self):
        clean_room, site = override_blocks(self.script)
        self.assertEqual(clean_room, {f"@takazudo/{name}" for name in packed_consumer.CLEAN_ROOM_OVERRIDES})
        self.assertEqual(site, {f"@takazudo/{name}" for name in packed_consumer.CONSUMER_OVERRIDES})


class LinkedStageParity(unittest.TestCase):
    def setUp(self):
        self.script = SMOKE.read_text()

    def test_links_the_same_packages_into_the_site(self):
        (block,) = [block for block in re.findall(r"const overrides = \{(.*?)\};", self.script, re.S)
                    if "link:" in block]
        self.assertEqual(set(re.findall(r'"@takazudo/([a-z0-9-]+)":', block)),
                         set(packed_consumer.LINKED_SITE_PACKAGES))

    def test_extracts_the_same_workspace_members(self):
        workspace = re.search(r"pnpm-workspace\.yaml\" <<'YAML'\npackages:\n(.*?)\nlinkWorkspacePackages", self.script,
                              re.S).group(1)
        members = re.findall(r'- "([^"]+)"', workspace)
        self.assertEqual(members, packed_consumer.LINKED_WORKSPACE_PACKAGES + ["@PLATFORM_PKG@"])

    def test_requires_the_same_dist_globs(self):
        parity = self.script[self.script.index("bash scripts/assert-dist-byte-parity.sh"):]
        parity = parity[:parity.index('"$SITE_DIR/dist"')]
        self.assertEqual(re.findall(r"--require '([^']+)'", parity), packed_consumer.REQUIRED_DIST_GLOBS)


def make_dir_link(link, target):
    """A directory junction on Windows (what pnpm creates there), a symlink elsewhere."""
    if os.name == "nt":
        import _winapi
        _winapi.CreateJunction(str(target), str(link))
    else:
        os.symlink(target, link, target_is_directory=True)


class DistParity(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.first, self.second = self.root / "first", self.root / "second"
        for dist in (self.first, self.second):
            self.write(dist, "index.html", b"<html>token</html>")
            self.write(dist, "assets/islands-0123abcd.js", b'export const build:"0123456789abcdef";')
            self.write(dist, "blog/hello/index.html", b"<p>hello</p>")
        self.log = io.StringIO()

    @staticmethod
    def write(dist, relative, data):
        path = dist / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def compare(self, required=None):
        required = packed_consumer.REQUIRED_DIST_GLOBS if required is None else required
        return packed_consumer.assert_dist_parity(self.first, self.second, required, self.log)

    def assert_fails(self, pattern, required=None):
        with self.assertRaisesRegex(AssertionError, pattern):
            self.compare(required)

    def test_identical_trees_pass(self):
        result = self.compare()
        self.assertEqual(result, {"files": 3, "islands": ["assets/islands-0123abcd.js"]})
        self.assertIn("[PASS]", self.log.getvalue())

    def test_missing_file_fails_and_is_logged(self):
        (self.second / "blog/hello/index.html").unlink()
        self.assert_fails("1 missing")
        self.assertIn("missing from second: blog/hello/index.html", self.log.getvalue())

    def test_extra_file_fails_and_is_logged(self):
        self.write(self.second, "assets/extra.css", b"body{}")
        self.assert_fails("1 extra")
        self.assertIn("extra in second: assets/extra.css", self.log.getvalue())

    def test_one_byte_difference_fails_and_is_logged(self):
        self.write(self.second, "index.html", b"<html>tokeN</html>")
        self.assert_fails("1 mismatched")
        self.assertIn("mismatched: index.html", self.log.getvalue())
        self.assertIn("build tokens in", self.log.getvalue())

    def test_missing_required_glob_fails(self):
        for dist in (self.first, self.second):
            (dist / "assets/islands-0123abcd.js").unlink()
        self.assert_fails(r"required glob: assets/islands-\*\.js")

    def test_required_glob_crosses_directories(self):
        self.compare(["*/hello/index.html"])
        self.assert_fails("required glob: nothing/\\*", ["nothing/*"])

    def test_empty_tree_fails(self):
        empty = self.root / "empty"
        (empty / "assets").mkdir(parents=True)
        with self.assertRaisesRegex(AssertionError, "contains no files"):
            packed_consumer.assert_dist_parity(empty, empty, [], self.log)

    def test_missing_tree_fails(self):
        with self.assertRaisesRegex(AssertionError, "not a directory"):
            packed_consumer.assert_dist_parity(self.root / "absent", self.second, [], self.log)

    def test_directory_link_in_dist_fails(self):
        outside = self.root / "outside"
        self.write(outside, "page.html", b"x")
        for dist in (self.first, self.second):
            make_dir_link(dist / "linked", outside)
        self.assert_fails("symlink or junction: linked")

    def test_file_symlink_in_dist_fails(self):
        for dist in (self.first, self.second):
            try:
                os.symlink(dist / "index.html", dist / "alias.html")
            except OSError as error:  # unprivileged Windows cannot create file symlinks
                self.skipTest(f"cannot create a file symlink: {error}")
        self.assert_fails("symlink or junction: alias.html")


class LinkCheck(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.target, self.elsewhere = self.root / "packages-ws/zfb", self.root / "elsewhere/zfb"
        for directory in (self.target, self.elsewhere):
            directory.mkdir(parents=True)
            (directory / "package.json").write_text("{}")
        self.entry = self.root / "site/node_modules/@takazudo/zfb"
        self.entry.parent.mkdir(parents=True)

    def test_link_to_the_target_passes(self):
        make_dir_link(self.entry, self.target)
        resolved = packed_consumer.assert_links_to(self.entry, self.target)
        self.assertTrue(os.path.samefile(resolved, self.target))

    def test_link_resolving_elsewhere_fails(self):
        make_dir_link(self.entry, self.elsewhere)
        with self.assertRaisesRegex(AssertionError, "resolves to"):
            packed_consumer.assert_links_to(self.entry, self.target)

    def test_plain_directory_fails(self):
        self.entry.mkdir()
        with self.assertRaisesRegex(AssertionError, "not a symlink or junction"):
            packed_consumer.assert_links_to(self.entry, self.target)

    def test_missing_entry_fails(self):
        with self.assertRaisesRegex(AssertionError, "does not exist"):
            packed_consumer.assert_links_to(self.entry, self.target)


class RequiredRows(unittest.TestCase):
    def report(self, names, rolldown=False):
        return {"rolldown_prototype": rolldown, "rows": [{"name": name, "passed": True} for name in names]}

    def test_all_rows_pass(self):
        packed_consumer.assert_required_rows(self.report(packed_consumer.required_rows(False)))
        packed_consumer.assert_required_rows(self.report(packed_consumer.required_rows(True), rolldown=True))

    def test_missing_linked_build_parity_fails(self):
        names = [name for name in packed_consumer.required_rows(False) if name != "linked-build-parity"]
        with self.assertRaisesRegex(AssertionError, "linked-build-parity"):
            packed_consumer.assert_required_rows(self.report(names))

    def test_missing_rolldown_row_fails_on_the_rolldown_leg(self):
        with self.assertRaisesRegex(AssertionError, "rolldown-backend-observed"):
            packed_consumer.assert_required_rows(self.report(packed_consumer.required_rows(False), rolldown=True))


if __name__ == "__main__":
    unittest.main()
