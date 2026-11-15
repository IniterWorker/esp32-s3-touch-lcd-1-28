{
  description = "GC9A01 0.5 reference test for Waveshare ESP32-S3-Touch-LCD-1.28 (esp-hal + Embassy)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      version = cargoToml.package.version;

      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

      # Host toolchain only: host-side lib tests + clippy/rust-analyzer.
      # The Xtensa (`esp`) toolchain cannot come from rust-overlay; it is
      # installed impurely via espup/rustup (see shellHook / `just
      # setup-toolchain`).
      toolchainFor =
        pkgs:
        pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "clippy"
            "rust-src"
            "rustfmt"
            "rust-analyzer"
          ];
        };

      # The only sandbox-able gate: host lib tests with no board features
      # (pure test-card drawing core). Firmware/xtensa builds are devShell
      # gates (`just build`, `just hil`), NOT Nix checks — they need the
      # impure esp toolchain and real hardware. Do not "fix" that here.
      hostTestPackageFor =
        system:
        let
          pkgs = pkgsFor system;
          rustToolchain = toolchainFor pkgs;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
        in
        rustPlatform.buildRustPackage {
          pname = "esp32-s3-touch-lcd-1-28-host-tests";
          inherit version;
          src = self;
          # Host tests must not see the xtensa default target / runner.
          postPatch = ''
            rm -f .cargo/config.toml rust-toolchain.toml
          '';
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "--lib"
            "--no-default-features"
          ];
          cargoTestFlags = [
            "--lib"
            "--no-default-features"
          ];
          # Library-only check package; the runnable deliverables are the
          # firmware ELFs built in the devShell.
          doInstall = false;
          meta = {
            description = "Host-side test-card tests for the GC9A01 reference test";
            license = with pkgs.lib.licenses; [
              mit
              asl20
            ];
          };
        };
    in
    {
      packages = forEachSystem (system: {
        default = hostTestPackageFor system;
      });

      checks = forEachSystem (system: {
        host-lib-tests = hostTestPackageFor system;
      });

      devShells = forEachSystem (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            name = "esp32-s3-touch-lcd-1-28-dev";
            packages = [
              (toolchainFor pkgs)
              pkgs.rustup
              pkgs.espflash
              pkgs.just
              pkgs.git
              pkgs.pkg-config
              pkgs.libudev-zero
              pkgs.nixfmt
            ];
            env.ESPFLASH_PORT = "/dev/ttyACM1";
            shellHook = ''
              export ESPFLASH_PORT="''${ESPFLASH_PORT:-/dev/ttyACM1}"
              # The impure esp toolchain must shadow the overlay cargo for
              # firmware builds, and espup's export file puts the Xtensa
              # linker on PATH (build.rs hard-fails without it).
              if rustup toolchain list 2>/dev/null | grep -q '^esp'; then
                esp_bin="$(rustup which --toolchain esp cargo 2>/dev/null | xargs -r dirname)"
                [ -n "$esp_bin" ] && export PATH="$esp_bin:$PATH"
                [ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
              else
                echo "Run: just setup-toolchain  (espup install --targets esp32s3, one-time, ~network)"
              fi
              echo "esp32-s3-touch-lcd-1-28 dev shell — $(rustc --version 2>/dev/null) · $(espflash --version 2>/dev/null) · ESPFLASH_PORT=$ESPFLASH_PORT"
              just --list 2>/dev/null || true
            '';
          };
        }
      );

      formatter = forEachSystem (system: (pkgsFor system).nixfmt);
    };
}
