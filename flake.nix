{
  description = "CDK Flake";

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
        stable_toolchain = pkgs.rust-bin.stable."1.82.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
          extensions = [ "rustfmt" "clippy" "rust-analyzer" ];
        };

        # MSRV stable
        msrv_toolchain = pkgs.rust-bin.stable."1.63.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };


        # DB MSRV stable
        db_msrv_toolchain = pkgs.rust-bin.stable."1.66.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };

        # Nightly for creating lock files
        nightly_toolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override {
          extensions = [ "rustfmt" "clippy" "rust-analyzer" ];
        });

        # Common inputs
        envVars = { };
        buildInputs = with pkgs; [
          # Add additional build inputs here
          git
          pkg-config
          curl
          just
          protobuf
          nixpkgs-fmt
          typos
          lnd
          clightning
          bitcoind
          sqlx-cli

          # Needed for github ci
          libz
        ] ++ libsDarwin;

        # WASM deps
        WASMInputs = with pkgs; [
        ];

        nativeBuildInputs = with pkgs; [
          #Add additional build inputs here
        ] ++ lib.optionals isDarwin [
          # Additional darwin specific native inputs can be set here
        ];
      in
      {
        checks = {
          # Pre-commit checks
          pre-commit-check =
            let
              # this is a hack based on https://github.com/cachix/pre-commit-hooks.nix/issues/126
              # we want to use our own rust stuff from oxalica's overlay
              _rust = pkgs.rust-bin.stable.latest.default;
              rust = pkgs.buildEnv {
                name = _rust.name;
                inherit (_rust) meta;
                buildInputs = [ pkgs.makeWrapper ];
                paths = [ _rust ];
                pathsToLink = [ "/" "/bin" ];
                postBuild = ''
                  for i in $out/bin/*; do
                    wrapProgram "$i" --prefix PATH : "$out/bin"
                  done
                '';
              };
            in
            pre-commit-hooks.lib.${system}.run {
              src = ./.;
              hooks = {
                rustfmt = {
                  enable = true;
                  entry = lib.mkForce "${rust}/bin/cargo-fmt fmt --all -- --config format_code_in_doc_comments=true --check --color always";
                };
                nixpkgs-fmt.enable = true;
                typos.enable = true;
                commitizen.enable = true; # conventional commits
              };
            };
        };

        devShells =
          let
            # pre-commit-checks
            _shellHook = (self.checks.${system}.pre-commit-check.shellHook or "");

            # devShells
            msrv = pkgs.mkShell ({
              shellHook = "
              ${_shellHook}
              cargo update -p half --precise 2.2.1
              cargo update -p tokio --precise 1.38.1
              cargo update -p tokio-util --precise 0.7.11
              cargo update -p tokio-stream --precise 0.1.15
              cargo update -p reqwest --precise 0.12.4
              cargo update -p serde_with --precise 3.1.0
              cargo update -p regex --precise 1.9.6
              cargo update -p backtrace --precise 0.3.58
              # For wasm32-unknown-unknown target
              cargo update -p bumpalo --precise 3.12.0
              cargo update -p moka --precise 0.11.1
              cargo update -p triomphe --precise 0.1.11
              cargo update -p url --precise 2.5.2
              ";
              buildInputs = buildInputs ++ WASMInputs ++ [ msrv_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

            stable = pkgs.mkShell ({
              shellHook = ''${_shellHook}'';
              buildInputs = buildInputs ++ WASMInputs ++ [ stable_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);


            db_shell = pkgs.mkShell ({
              shellHook = ''
                ${_shellHook}
                cargo update -p half --precise 2.2.1
                cargo update -p home --precise 0.5.5
                cargo update -p tokio --precise 1.38.1
                cargo update -p tokio-stream --precise 0.1.15
                cargo update -p serde_with --precise 3.1.0
                cargo update -p reqwest --precise 0.12.4
                cargo update -p url --precise 2.5.2
                cargo update -p allocator-api2 --precise 0.2.18
              '';
              buildInputs = buildInputs ++ WASMInputs ++ [ db_msrv_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);



            nightly = pkgs.mkShell ({
              shellHook = ''
                ${_shellHook}
                # Needed for github ci
                export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath [
                  pkgs.zlib
                  ]}:$LD_LIBRARY_PATH
              '';
              buildInputs = buildInputs ++ [ nightly_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

          in
          {
            inherit msrv stable nightly db_shell;
            default = stable;
          };
      }
    );
}
