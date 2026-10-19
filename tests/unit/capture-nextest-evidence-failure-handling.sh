#!/bin/sh
set -eu
exec python3 "$(dirname "$0")/capture-nextest-evidence-failure-handling.py"
