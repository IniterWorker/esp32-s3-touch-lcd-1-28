# GC9A01 0.5 reference test — Waveshare ESP32-S3-Touch-LCD-1.28.
# Run inside `nix develop` (or direnv). See README.md for the full guide.

port := env_var_or_default("ESPFLASH_PORT", "/dev/ttyACM1")
host := `rustup run stable rustc -vV 2>/dev/null | sed -n 's/host: //p'`
# The devShell prepends the esp toolchain's bin dir, whose clippy-driver
# would otherwise shadow stable's in the host recipes below.
stable_bin := `dirname "$(rustup which --toolchain stable cargo)"`

# List recipes.
default:
    @just --list

# One-time: install espup (if missing) and the Xtensa `esp` toolchain.
setup-toolchain:
    command -v espup >/dev/null || cargo install espup --locked
    espup install --targets esp32s3

# Build all firmware ELFs (release).
build:
    cargo build --release --bins
    cargo build --release --bin sandbox --features sandbox

# Host recipes run on the rustup `stable` toolchain: the `esp` toolchain +
# this repo's `[unstable] build-std` config cannot build host test
# binaries, and its clippy must not shadow stable's (see stable_bin).

# Host probe-point tests for the pure test-card core (no hardware).
test-host:
    PATH="{{stable_bin}}:$PATH" rustup run stable cargo test --lib --no-default-features --features ui --target {{host}}


# Flash + monitor the blocking firmware (interactive visual check).
flash-blocking:
    cargo run --release --bin blocking

# Flash + monitor the async firmware (interactive visual check).
flash-async:
    cargo run --release --bin embassy_async


# Flash + monitor the QMI8658 blocking IMU HIL firmware.
flash-imu-blocking:
    cargo run --release --bin imu_blocking

# Flash + monitor the QMI8658 Embassy async IMU HIL firmware.
flash-imu-embassy:
    cargo run --release --bin imu_embassy


# Flash + monitor the CST816S blocking touch HIL firmware.
flash-touch-blocking:
    cargo run --release --bin touch_blocking

# Flash + monitor the CST816S async touch HIL firmware (interactive finger check).
flash-touch-async:
    cargo run --release --bin touch_async

# Flash + monitor the flash settings-store reference bin.
flash-store:
    cargo run --release --bin flash_store

# Flash + monitor the battery ADC reference bin.
flash-battery:
    cargo run --release --bin battery

# Flash + monitor the SBX sandbox application.
flash-sandbox:
    cargo run --release --bin sandbox --features sandbox
# Automated hardware test: blocking.
hil-blocking:
    ESPFLASH_PORT={{port}} scripts/hil.sh blocking

# Automated hardware test: async.
hil-async:
    ESPFLASH_PORT={{port}} scripts/hil.sh embassy_async

# Automated hardware test: both bins.
hil: hil-blocking hil-async

# Automated hardware test: flash settings store.
hil-flash:
    ESPFLASH_PORT={{port}} scripts/hil.sh flash_store


# Automated sandbox test: flash, then UART protocol + persistence HIL.
hil-sandbox:
    cargo build --release --bin sandbox --features sandbox
    espflash flash --port {{port}} --chip esp32s3 --partition-table partitions.csv target/xtensa-esp32s3-none-elf/release/sandbox
    ESPFLASH_PORT={{port}} python3 scripts/sandbox_hil.py
# Automated hardware test: battery ADC (format only).
hil-battery:
    ESPFLASH_PORT={{port}} scripts/hil-battery.sh battery

# Automated IMU hardware test: blocking (board flat + still!).
hil-imu-blocking:
    ESPFLASH_PORT={{port}} scripts/hil-imu.sh imu_blocking

# Automated IMU hardware test: Embassy async (board flat + still!).
hil-imu-embassy:
    ESPFLASH_PORT={{port}} scripts/hil-imu.sh imu_embassy

# Automated IMU hardware test: both bins.
hil-imu: hil-imu-blocking hil-imu-embassy

# Automated touch hardware test: blocking.
hil-touch-blocking:
    ESPFLASH_PORT={{port}} scripts/hil-touch.sh touch_blocking

# Automated touch hardware test: async.
hil-touch-async:
    ESPFLASH_PORT={{port}} scripts/hil-touch.sh touch_async

# Automated touch hardware test: both bins.
hil-touch: hil-touch-blocking hil-touch-async

# Serial monitor only.
monitor:
    espflash monitor --port {{port}} --chip esp32s3
