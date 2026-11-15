#!/usr/bin/env bash
# Hardware-in-the-loop test for one CST816S touch firmware bin: build,
# flash, capture UART markers, assert them. Cloned from hil.sh (GC9A01)
# with the CST816S-REF marker set; kept separate so the display HIL
# logic stays untouched.
#
# Usage: scripts/hil-touch.sh <touch_blocking|touch_async>
# Env:   ESPFLASH_PORT (default /dev/ttyACM1)
#
# The automated sequence needs no finger: probe/config/idle/sleep-wake
# all run hands-off. Interactive touch verification is
# `just flash-touch-async` + a finger (see README).
set -euo pipefail

CAPTURE_S=25

BIN="${1:?usage: hil-touch.sh <touch_blocking|touch_async>}"
PORT="${ESPFLASH_PORT:-/dev/ttyACM1}"
ELF="target/xtensa-esp32s3-none-elf/release/${BIN}"
LOG="target/hil-${BIN}.log"

case "$BIN" in
touch_blocking) MODE="blocking" ;;
touch_async) MODE="async" ;;
*)
    echo "hil-touch.sh: unknown bin '$BIN'" >&2
    exit 2
    ;;
esac

fail() {
    echo "HIL ${BIN}: FAIL — $1" >&2
    echo "--- last 40 lines of ${LOG} ---" >&2
    tail -n 40 "$LOG" >&2 || true
    exit 1
}

first_line() { grep -n -m1 -- "$1" "$LOG" | cut -d: -f1 || true; }

echo "HIL ${BIN}: building (port ${PORT})"
cargo build --release --bin "$BIN"

echo "HIL ${BIN}: flashing ${ELF}"
espflash flash --port "$PORT" --chip esp32s3 --partition-table partitions.csv "$ELF"

echo "HIL ${BIN}: capturing ${CAPTURE_S}s of UART -> ${LOG}"
timeout "$CAPTURE_S" espflash monitor --non-interactive --port "$PORT" --chip esp32s3 --elf "$ELF" >"$LOG" 2>&1 || true

grep -q "CST816S-REF mode=${MODE} step=boot" "$LOG" || fail "no boot marker"
grep -q "step=probe-ok" "$LOG" || fail "probe-ok marker missing"
grep -q "step=config-ok" "$LOG" || fail "config-ok marker missing"
grep -q "step=idle-ok" "$LOG" || fail "idle-ok marker missing"
grep -q "step=sleep-wake-ok" "$LOG" || fail "sleep-wake-ok marker missing"
grep -q "result=ALL-PASS" "$LOG" || fail "ALL-PASS marker missing"
if grep -qE 'FAIL|panicked' "$LOG"; then
    fail "FAIL/panicked present in log"
fi

probe_line="$(first_line "step=probe-ok")"
pass_line="$(first_line "result=ALL-PASS")"
[ -n "$probe_line" ] && [ -n "$pass_line" ] && [ "$probe_line" -lt "$pass_line" ] ||
    fail "probe-ok does not precede ALL-PASS"

if [ "$MODE" = "async" ]; then
    heartbeats="$(grep -c 'step=heartbeat' "$LOG" || true)"
    [ "$heartbeats" -ge 3 ] || fail "only ${heartbeats} heartbeat lines (need >= 3)"

    hb_between="$(grep -n 'step=heartbeat' "$LOG" | cut -d: -f1 |
        awk -v lo="$probe_line" -v hi="$pass_line" '$1 > lo && $1 < hi { n++ } END { print n + 0 }')"
    [ "$hb_between" -ge 1 ] ||
        fail "no heartbeat between probe-ok and ALL-PASS (executor starved?)"
fi

echo "HIL ${BIN}: PASS"
grep -E 'CST816S-REF' "$LOG" | head -n 10 || true
