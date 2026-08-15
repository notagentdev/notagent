#!/usr/bin/env bash
# Final parity audit (O-11): checks every packages/**/src file of the TS repo
# against all crates/*/PARITY.md ledgers and writes the gap report to
# plans/final-parity-audit.md.
#
#   scripts/parity-audit.sh [--ts-repo <path>] [--out <path>] [--check] [--quiet]
#
# --check exits 1 while files without any ledger trace remain (gate G4).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v node >/dev/null 2>&1; then
  echo "parity-audit: node is required (the TS repo's toolchain provides it)" >&2
  exit 2
fi

exec node "$repo_root/tools/parity-audit.mjs" "$@"
