#!/usr/bin/env bash
# Gate-Check des Gesamtworkspaces: Format, Lints, Tests.
# Muss vor jedem Commit/Merge grün sein (Master-Plan, Abschnitt Gates).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace

echo "==> check.sh OK"
