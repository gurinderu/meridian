{
  description = "meridian — local proxy exposing Claude Code as the Anthropic + OpenAI APIs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    let
      # Release build of the `meridian` binary. Pure-Rust workspace (no
      # openssl/ring), so the only native input is the darwin link shim;
      # `curl`/`security` are resolved from PATH at runtime, not linked.
      # Takes a plain nixpkgs `pkgs`, so it also works through the overlay
      # below (no rust-overlay required in the consumer's config).
      meridianPackage = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "meridian";
        version = "0.0.0";
        src = self;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];
        buildType = "release"; # default, pinned here so it's explicit
        # Skipped: the test suite spawns the `claude` CLI and reaches the
        # network, neither of which exists in the Nix sandbox.
        doCheck = false;
        meta.mainProgram = "meridian";
      };
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # Pinned stable toolchain for the dev shell (clippy/rustfmt/analyzer).
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];
          packages = [
            rustToolchain
            pkgs.curl # runtime dep: OAuth refresh shells out to curl
          ];
          env.RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };

        packages.default = meridianPackage pkgs;

        # `nix run github:gurinderu/meridian -- serve`
        apps.default = flake-utils.lib.mkApp {
          drv = self.packages.${system}.default;
        };

        formatter = pkgs.nixpkgs-fmt;
      })
    // {
      # System-agnostic overlay for consuming configs:
      #   nixpkgs.overlays = [ meridian.overlays.default ];  # → pkgs.meridian
      overlays.default = final: _prev: {
        meridian = meridianPackage final;
      };
    };
}
