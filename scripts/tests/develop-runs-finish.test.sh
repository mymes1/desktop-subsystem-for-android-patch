#!/usr/bin/env bash
# Checks that no newer merge cancels a develop run in progress: every test workflow puts
# each push in a concurrency group of its own (its commit), so every develop commit is
# checked to the end.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
for workflow in "$root"/.github/workflows/test*.yml; do
  if ! grep -F "  group: " "$workflow" | grep -Fq "{{ github.event_name == 'push' && github.sha ||"; then
    echo "$(basename "$workflow"): a newer push to develop can cancel the run in progress" >&2
    exit 1
  fi
done
echo "develop runs finish: ok"
