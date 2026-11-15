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

# Build both firmware ELFs (release).
build:
    cargo build --release --bins

# Host recipes run on the rustup `stable` toolchain: the `esp` toolchain +
# this repo's `[unstable] build-std` config cannot build host test
# binaries, and its clippy must not shadow stable's (see stable_bin).

# Host probe-point tests for the pure test-card core (no hardware).
test-host:
    PATH="{{stable_bin}}:$PATH" rustup run stable cargo test --lib --no-default-features --target {{host}}


# Flash + monitor the blocking firmware (interactive visual check).
flash-blocking:
    cargo run --release --bin blocking

# Flash + monitor the async firmware (interactive visual check).
flash-async:
    cargo run --release --bin embassy_async

# Automated hardware test: blocking.
hil-blocking:
    ESPFLASH_PORT={{port}} scripts/hil.sh blocking

# Automated hardware test: async.
hil-async:
    ESPFLASH_PORT={{port}} scripts/hil.sh embassy_async

# Automated hardware test: both bins.
hil: hil-blocking hil-async

# Serial monitor only.
monitor:
    espflash monitor --port {{port}} --chip esp32s3
