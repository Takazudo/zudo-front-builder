#!/usr/bin/env bash
#
# Sum nextest JUnit testcase wall times, including retry attempts.
#
# Usage:
#   scripts/sum-junit-times.sh <junit.xml> [<junit.xml> ...]
#
# A nextest testcase that passes on retry has one or more nested
# <flakyFailure> elements. Its total wall time is the testcase's own `time`
# plus every nested flakyFailure's `time`; omitting the latter makes a cold or
# racing failure look cheaper than it was.

set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <junit.xml> [<junit.xml> ...]" >&2
    exit 2
fi

python3 - "$@" <<'PYEOF'
from decimal import Decimal, InvalidOperation
import sys
import xml.etree.ElementTree as ET


def duration(element: ET.Element, attribute: str, path: str) -> Decimal:
    value = element.get(attribute)
    if value is None:
        return Decimal("0")
    try:
        parsed = Decimal(value)
    except InvalidOperation as exc:
        raise ValueError(
            f"{path}: non-numeric {attribute}={value!r} on <{element.tag}>"
        ) from exc
    if not parsed.is_finite() or parsed < 0:
        raise ValueError(
            f"{path}: {attribute}={value!r} on <{element.tag}> is not a finite duration"
        )
    return parsed


total = Decimal("0")
for path in sys.argv[1:]:
    try:
        root = ET.parse(path).getroot()
    except (ET.ParseError, OSError) as exc:
        print(f"{path}: cannot read JUnit XML: {exc}", file=sys.stderr)
        sys.exit(1)

    try:
        for testcase in root.iter("testcase"):
            total += duration(testcase, "time", path)
            for flaky_failure in testcase.iter("flakyFailure"):
                total += duration(flaky_failure, "time", path)
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        sys.exit(1)

print(f"{total:.6f}")
PYEOF
