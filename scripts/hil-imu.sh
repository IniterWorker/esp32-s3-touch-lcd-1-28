#!/usr/bin/env bash
# Hardware-in-the-loop test for one QMI8658 IMU firmware bin: build,
# flash, capture UART markers, assert them. Cloned from hil.sh
# (GC9A01) with the QMI8658-HIL marker set.
#
# Usage: scripts/hil-imu.sh <imu_blocking|imu_embassy>
# Env:   ESPFLASH_PORT (default /dev/ttyACM1)
#
# IMPORTANT: the board must rest flat and still — the gravity-ok
# assert averages the first samples and expects |a| ~ 1 g.
set -euo pipefail

CAPTURE_S=30

BIN="${1:?usage: hil-imu.sh <imu_blocking|imu_embassy>}"
PORT="${ESPFLASH_PORT:-/dev/ttyACM1}"
ELF="target/xtensa-esp32s3-none-elf/release/${BIN}"
LOG="target/hil-${BIN}.log"

case "$BIN" in
imu_blocking) MODE="blocking" ;;
imu_embassy) MODE="async" ;;
*)
    echo "hil-imu.sh: unknown bin '$BIN'" >&2
    exit 2
    ;;
esac

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

grep -q "QMI8658-HIL mode=${MODE} step=boot" "$LOG" || fail "no boot marker"
grep -q "step=init-ok" "$LOG" || fail "init-ok marker missing (probe failed?)"
grep -q "step=gravity-ok" "$LOG" || fail "gravity-ok missing — board must rest flat and still during HIL"
samples="$(grep -c 'step=sample' "$LOG" || true)"
[ "$samples" -ge 3 ] || fail "only ${samples} sample lines (need >= 3)"
if grep -qE 'result=FAIL|panicked' "$LOG"; then
    fail "FAIL/panicked present in log"
fi

echo "HIL ${BIN}: PASS"
grep -E 'QMI8658-HIL' "$LOG" | head -n 8 || true
