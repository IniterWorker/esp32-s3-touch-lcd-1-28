#!/usr/bin/env bash
# Hardware-in-the-loop test for the battery ADC bin: build, flash,
# capture UART, assert the BAT-REF line format. Values are NOT
# asserted: on USB power without a battery the sense node floats;
# plausibility is a human check with a battery attached.
#
# Usage: scripts/hil-battery.sh battery
# Env:   ESPFLASH_PORT (default /dev/ttyACM1)
set -euo pipefail

CAPTURE_S=15

BIN="${1:?usage: hil-battery.sh battery}"
PORT="${ESPFLASH_PORT:-/dev/ttyACM1}"
ELF="target/xtensa-esp32s3-none-elf/release/${BIN}"
LOG="target/hil-${BIN}.log"

[ "$BIN" = "battery" ] || { echo "hil-battery.sh: unknown bin '$BIN'" >&2; exit 2; }

fail() {
    echo "HIL ${BIN}: FAIL — $1" >&2
    echo "--- last 40 lines of ${LOG} ---" >&2
    tail -n 40 "$LOG" >&2 || true
    exit 1
}

echo "HIL ${BIN}: building (port ${PORT})"
cargo build --release --bin "$BIN"

echo "HIL ${BIN}: flashing ${ELF}"
espflash flash --port "$PORT" --chip esp32s3 --partition-table partitions.csv "$ELF"

echo "HIL ${BIN}: capturing ${CAPTURE_S}s of UART -> ${LOG}"
timeout "$CAPTURE_S" espflash monitor --non-interactive --port "$PORT" --chip esp32s3 --elf "$ELF" >"$LOG" 2>&1 || true

lines="$(grep -cE 'BAT-REF mv=[0-9]+ pct=[0-9]+ raw=[0-9]+' "$LOG" || true)"
[ "$lines" -ge 3 ] || fail "only ${lines} BAT-REF lines (need >= 3)"
if grep -qE 'result=FAIL|panicked' "$LOG"; then
    fail "FAIL/panicked present in log"
fi

echo "HIL ${BIN}: PASS"
grep -E 'BAT-REF' "$LOG" | head -n 5 || true
