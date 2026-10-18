#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
python3 "$root/tests/unit/native-exam.py"
