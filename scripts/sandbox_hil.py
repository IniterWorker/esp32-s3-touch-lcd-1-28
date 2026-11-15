#!/usr/bin/env python3
"""Interactive HIL for the SBX sandbox UART protocol.

Assumes the sandbox bin is already flashed (see `just hil-sandbox`).
Sequence: reset -> boot markers (settings/touch/imu/ui) -> SBX GET
-> SET backlight=37 -> settings-saved -> SET backlight=100 ->
settings-saved -> bad value rejected -> RTS/DTR reset ->
settings-loaded with the saved seq -> GET shows backlight=100
(proves flash persistence across a reboot) -> STREAM on/off.

Env: ESPFLASH_PORT (default /dev/ttyACM1).
"""
import os
import re
import sys
import time

import serial

PORT = os.environ.get("ESPFLASH_PORT", "/dev/ttyACM1")


def fail(msg, log):
    print(f"HIL sandbox: FAIL — {msg}", file=sys.stderr)
    print("--- last 40 lines ---", file=sys.stderr)
    print("\n".join(log[-40:]), file=sys.stderr)
    sys.exit(1)


def main():
    log = []
    rxbuf = bytearray()
    cursor = 0  # next log index wait_for() will scan from
    ser = serial.Serial(PORT, 115200, timeout=0.2)

    def pump():
        """Read available bytes, append complete lines to log."""
        chunk = ser.read(1024)
        if not chunk:
            return
        rxbuf.extend(chunk)
        while b"\n" in rxbuf:
            line, _, rest = bytes(rxbuf).partition(b"\n")
            del rxbuf[: len(line) + 1]
            log.append(line.decode(errors="replace").rstrip("\r"))
            rxbuf[:] = rest

    def wait_for(pattern, timeout_s, what, backlog=False):
        nonlocal cursor
        rx = re.compile(pattern)
        start = 0 if backlog else cursor
        deadline = time.monotonic() + timeout_s
        while True:
            for i in range(start, len(log)):
                m = rx.search(log[i])
                if m:
                    cursor = i + 1
                    return m, log[i]
            if time.monotonic() >= deadline:
                fail(f"timeout waiting for {what} ({pattern})", log)
            pump()

    def reset_board():
        nonlocal cursor
        # CH343 auto-reset (2-transistor): DTR drives GPIO0, RTS
        # drives EN. Release EN with DTR deasserted or the ROM lands
        # in download mode and the app never runs.
        ser.dtr = False
        ser.rts = True
        time.sleep(0.1)
        ser.rts = False
        ser.reset_input_buffer()
        del rxbuf[:]
        cursor = len(log)

    def send(line):
        ser.write((line + "\n").encode())

    reset_board()

    m, _ = wait_for(r"SBX step=settings-loaded seq=(\w+)", 15, "settings-loaded")
    seq0 = 0 if m.group(1) == "defaults" else int(m.group(1))
    wait_for(r"SBX step=touch-ok", 15, "touch-ok", backlog=True)
    wait_for(r"SBX step=imu-ok", 15, "imu-ok", backlog=True)
    wait_for(r"SBX step=ui-ok", 15, "ui-ok", backlog=True)

    send("SBX GET")
    deadline = time.monotonic() + 5
    mark = len(log)
    while time.monotonic() < deadline:
        pump()
        if sum(1 for l in log[mark:] if l.startswith("SBX-STATE ")) >= 6:
            break
    states = sum(1 for l in log[mark:] if l.startswith("SBX-STATE "))
    if states < 6:
        fail(f"only {states} SBX-STATE lines after GET (need >= 6)", log)

    send("SBX SET backlight=37")
    wait_for(r"SBX OK backlight=37", 5, "OK backlight=37")
    m, _ = wait_for(r"SBX step=settings-saved seq=(\d+)", 5, "settings-saved #1")
    seq1 = int(m.group(1))
    if seq1 != seq0 + 1:
        fail(f"saved seq {seq1} != loaded {seq0} + 1", log)

    send("SBX SET backlight=100")
    wait_for(r"SBX OK backlight=100", 5, "OK backlight=100")
    m, _ = wait_for(r"SBX step=settings-saved seq=(\d+)", 5, "settings-saved #2")
    seq2 = int(m.group(1))
    if seq2 != seq1 + 1:
        fail(f"saved seq {seq2} != {seq1} + 1", log)

    # Bad input must be rejected without wedging the protocol.
    send("SBX SET backlight=101")
    wait_for(r"SBX ERR backlight", 5, "ERR backlight")

    # Hardware reset; settings must survive.
    reset_board()
    m, _ = wait_for(r"SBX step=settings-loaded seq=(\d+)", 15, "settings-loaded after reset")
    if int(m.group(1)) != seq2:
        fail(f"after reset loaded seq {m.group(1)} != saved {seq2}", log)
    wait_for(r"SBX step=ui-ok", 15, "ui-ok after reset")
    send("SBX GET")
    wait_for(r"SBX-STATE backlight=100", 5, "backlight=100 after reset")

    # Live stream last: stream toggles persist and would bump the
    # settings seq asserted above.
    send("SBX STREAM on")
    wait_for(r"SBX-DATA ax_mg=-?\d+ ay_mg=-?\d+ az_mg=-?\d+", 5, "SBX-DATA stream")
    send("SBX STREAM off")
    wait_for(r"SBX OK stream=off", 5, "OK stream=off")

    if any("panicked" in l or "result=FAIL" in l for l in log):
        fail("panic/FAIL present in log", log)

    print("HIL sandbox: PASS")
    for l in log:
        if "SBX" in l:
            print(l)


if __name__ == "__main__":
    main()
