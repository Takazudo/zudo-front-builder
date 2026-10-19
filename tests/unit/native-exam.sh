#!/bin/sh
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
python3 "$root/tests/unit/native-exam.py"
