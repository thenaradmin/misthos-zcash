#!/bin/bash
set -eu
cd "$(dirname "$0")/../.."
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo build --offline --bin payroll --lib > audit/logs/build.txt 2>&1
build=$?
echo "build exit=$build"
if [ "$build" -ne 0 ]; then exit "$build"; fi
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo test --offline > audit/logs/cargo-test.txt 2>&1
echo "cargo test exit=$?"
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo test --offline --lib > audit/logs/cargo-lib.txt 2>&1
echo "cargo lib exit=$?"
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo test --offline --test cli_label --test cli_serve --test proof_real_tx --test sig_attacks > audit/logs/cargo-integrations.txt 2>&1
echo "cargo integrations exit=$?"
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 rustc --edition=2021 audit/run/probe.rs --extern zec_payroll=target/debug/libzec_payroll.rlib -L dependency=target/debug/deps -o target/audit-probe > audit/logs/probe-build.txt 2>&1
if [ "$?" -eq 0 ]; then env -i PATH="$PATH" target/audit-probe > audit/logs/probe.txt 2>&1; fi
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 cargo metadata --offline --no-deps --format-version 1 > audit/logs/cargo-metadata.json
# Reuse only synthetic fixture helpers to capture the real offline verifier output.
env -i PATH="$PATH" HOME="$HOME" CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/target" cargo build --offline --manifest-path audit/run/proof-tool/Cargo.toml > audit/logs/offline-proof-build.txt 2>&1
env -i PATH="$PATH" PAYROLL_BIN="$PWD/target/debug/payroll" target/debug/audit-proof-output > audit/logs/offline-proof-output.txt 2>&1
python3 audit/run/nonbrowser.py cli
