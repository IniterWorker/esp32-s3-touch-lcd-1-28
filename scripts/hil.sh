#!/usr/bin/env bash
# Hardware-in-the-loop test for one firmware bin: build, flash, capture the
# UART markers of one full reference-test cycle, assert them.
#
# Usage: scripts/hil.sh <blocking|embassy_async>
# Env:   ESPFLASH_PORT (default /dev/ttyACM1)
#
# Connect/reset caveat: `espflash monitor` resets the board on attach (the
# CH343 DTR/RTS auto-reset circuit), so the full sequence re-runs inside
# the capture window.
set -euo pipefail

# Capture window in seconds. One cycle is ~10 s at the spec'd delays; raise
# this single constant if that ever stops being true.
CAPTURE_S=30

BIN="${1:?usage: hil.sh <blocking|embassy_async|flash_store>}"
PORT="${ESPFLASH_PORT:-/dev/ttyACM1}"
ELF="target/xtensa-esp32s3-none-elf/release/${BIN}"
LOG="target/hil-${BIN}.log"

case "$BIN" in
blocking) MODE="blocking" ;;
flash_store) MODE="flash"; CAPTURE_S=15 ;;
embassy_async) MODE="async" ;;
*)
    echo "hil.sh: unknown bin '$BIN'" >&2
    exit 2
    ;;
esac

fail() {
    echo "HIL ${BIN}: FAIL — $1" >&2
    echo "--- last 40 lines of ${LOG} ---" >&2
    tail -n 40 "$LOG" >&2 || true
    exit 1
}

# Line number of the first match, or empty.
first_line() { grep -n -m1 -- "$1" "$LOG" | cut -d: -f1 || true; }

echo "HIL ${BIN}: building (port ${PORT})"
cargo build --release --bin "$BIN"

echo "HIL ${BIN}: flashing ${ELF}"
espflash flash --port "$PORT" --chip esp32s3 --partition-table partitions.csv "$ELF"

echo "HIL ${BIN}: capturing ${CAPTURE_S}s of UART -> ${LOG}"
timeout "$CAPTURE_S" espflash monitor --non-interactive --port "$PORT" --chip esp32s3 --elf "$ELF" >"$LOG" 2>&1 || true

if [ "$BIN" = "flash_store" ]; then
    grep -q "FLASH-REF step=probe-ok" "$LOG" || fail "no probe-ok marker"
    grep -qE "FLASH-REF step=write-read-ok seq=[0-9]+" "$LOG" || fail "no write-read-ok marker"
    if grep -qE 'FAIL|panicked' "$LOG"; then
        fail "FAIL/panicked present in log"
    fi
    echo "HIL ${BIN}: PASS"
    grep -E 'FLASH-REF' "$LOG" | head -n 6 || true
    exit 0
fi

grep -q "mode=${MODE} step=boot" "$LOG" || fail "no boot marker"
grep -q "step=basic-ok" "$LOG" || fail "BasicMode phase marker missing"
grep -q "step=card-ok" "$LOG" || fail "test-card marker missing"
grep -q "result=ALL-PASS" "$LOG" || fail "ALL-PASS marker missing"
if grep -qE 'FAIL|panicked' "$LOG"; then
    fail "FAIL/panicked present in log"
fi

boot_line="$(first_line "step=boot")"
pass_line="$(first_line "result=ALL-PASS")"
[ -n "$boot_line" ] && [ -n "$pass_line" ] && [ "$boot_line" -lt "$pass_line" ] ||
    fail "boot marker does not precede ALL-PASS"

if [ "$MODE" = "async" ]; then
    heartbeats="$(grep -c 'step=heartbeat' "$LOG" || true)"
    [ "$heartbeats" -ge 3 ] || fail "only ${heartbeats} heartbeat lines (need >= 3)"

    fill_line="$(first_line 'step=fill')"
    hb_between="$(grep -n 'step=heartbeat' "$LOG" | cut -d: -f1 |
        awk -v lo="$fill_line" -v hi="$pass_line" '$1 > lo && $1 < hi { n++ } END { print n + 0 }')"
    [ "$hb_between" -ge 1 ] ||
        fail "no heartbeat between first fill and ALL-PASS (executor starved?)"
fi

echo "HIL ${BIN}: PASS"
grep -E 'step=(fill|card-ok)' "$LOG" | head -n 6 || true
