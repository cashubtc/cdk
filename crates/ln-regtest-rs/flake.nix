{
  description = "ln-regtest-rs Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };

    flake-utils.url = "github:numtide/flake-utils";

    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, pre-commit-hooks, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        lib = pkgs.lib;
        stdenv = pkgs.stdenv;
        isDarwin = stdenv.isDarwin;
        libsDarwin = with pkgs; lib.optionals isDarwin [
          # Additional darwin specific inputs can be set here
          darwin.apple_sdk.frameworks.Security
          darwin.apple_sdk.frameworks.SystemConfiguration
        ];

        # Dependencies
        pkgs = import nixpkgs {
          inherit system overlays;
        };


        # Toolchains
        # latest stable
        stable_toolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };


        # Nightly for creating lock files
        nightly_toolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);

        # Common inputs
        envVars = { };
        buildInputs = with pkgs; [
          # Add additional build inputs here
          git
          pkg-config
          curl
          just
          protobuf3_20
          nixpkgs-fmt
          rust-analyzer
          typos
          lnd
          clightning
          bitcoind
        ] ++ libsDarwin;

        nativeBuildInputs = with pkgs; [
          # Add additional build inputs here
        ] ++ lib.optionals isDarwin [
          # Additional darwin specific native inputs can be set here
        ];
      in
      {
        checks = {
        };

        devShells =
          let
            # pre-commit-checks
            _shellHook = (self.checks.${system}.pre-commit-check.shellHook or "");

            stable = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ [ stable_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

            nightly = pkgs.mkShell ({
              shellHook = "${_shellHook}";
              buildInputs = buildInputs ++ [ nightly_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

          in
          {
            inherit stable nightly;
            default = stable;
          };
      }
    );
}

