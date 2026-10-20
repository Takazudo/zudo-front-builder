#!/usr/bin/env python3
"""Drift guard: packed-consumer.py packs and overrides what the required smoke script does."""
import importlib.util
from pathlib import Path
import re
import unittest

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parents[1] / "scripts/smoke-packed-clean-room.sh"
SPEC = importlib.util.spec_from_file_location("packed_consumer", HERE / "packed-consumer.py")
packed_consumer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packed_consumer)


def override_blocks(script):
    """The non-platform package names of each `const overrides = {...}` block, in order."""
    return [{name for name in re.findall(r'"(@takazudo/[a-z0-9-]+)":', block) if not name.endswith("-gnu")}
            for block in re.findall(r"const overrides = \{(.*?)\};", script, re.S)]


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


if __name__ == "__main__":
    unittest.main()
