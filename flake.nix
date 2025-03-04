{
  description = "CDK Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };

    flake-utils.url = "github:numtide/flake-utils";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, pre-commit-hooks, crane, fenix, ... }:
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
        stable_toolchain = pkgs.rust-bin.stable."1.83.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
          extensions = [ "rustfmt" "clippy" "rust-analyzer" ];
        };

        # MSRV stable
        msrv_toolchain = pkgs.rust-bin.stable."1.75.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
        };

        # Nightly used for formatting
        nightly_toolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override {
          extensions = [ "rustfmt" "clippy" "rust-analyzer" "rust-src" ];
          targets = [ "wasm32-unknown-unknown" ]; # wasm
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
          cargo-outdated

          # Needed for github ci
          libz
        ] ++ libsDarwin;

        # WASM deps
        WASMInputs = with pkgs; [
        ];



        craneLib = crane.mkLib pkgs;
        src = craneLib.cleanCargoSource ./.;

        # Common arguments can be set here to avoid repeating them later
        commonArgs = {
          inherit src;
          strictDeps = true;

          buildInputs = [
            # Add additional build inputs here
            pkgs.protobuf
            pkgs.pkg-config
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            # Additional darwin specific inputs can be set here
            pkgs.libiconv
          ];

          # Additional environment variables can be set directly
          # MY_CUSTOM_VAR = "some value";
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          PROTOC_INCLUDE = "${pkgs.protobuf}/include";
        };


        craneLibLLvmTools = craneLib.overrideToolchain
          (fenix.packages.${system}.complete.withComponents [
            "cargo"
            "llvm-tools"
            "rustc"
          ]);

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        individualCrateArgs = commonArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          # NB: we disable tests since we'll run them all via cargo-nextest
          doCheck = false;
        };

        fileSetForCrate = crate: lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            (craneLib.fileset.commonCargoSources ./crates/cdk)
            (craneLib.fileset.commonCargoSources ./crates/cdk-axum)
            (craneLib.fileset.commonCargoSources ./crates/cdk-cln)
            (craneLib.fileset.commonCargoSources ./crates/cdk-lnd)
            (craneLib.fileset.commonCargoSources ./crates/cdk-fake-wallet)
            (craneLib.fileset.commonCargoSources ./crates/cdk-lnbits)
            (craneLib.fileset.commonCargoSources ./crates/cdk-strike)
            (craneLib.fileset.commonCargoSources ./crates/cdk-phoenixd)
            (craneLib.fileset.commonCargoSources ./crates/cdk-redb)
            (craneLib.fileset.commonCargoSources ./crates/cdk-sqlite)
            ./crates/cdk-sqlite/src/mint/migrations
            ./crates/cdk-sqlite/src/wallet/migrations
            (craneLib.fileset.commonCargoSources crate)
          ];
        };

        cdk-mintd = craneLib.buildPackage (individualCrateArgs // {
          pname = "cdk-mintd";
          name = "cdk-mintd-${individualCrateArgs.version}";
          cargoExtraArgs = "-p cdk-mintd";
          src = fileSetForCrate ./crates/cdk-mintd;
        });


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


        packages = {
          inherit cdk-mintd;
          default = cdk-mintd;
        } // lib.optionalAttrs (!pkgs.stdenv.isDarwin) {
          my-workspace-llvm-coverage = craneLibLLvmTools.cargoLlvmCov (commonArgs // {
            inherit cargoArtifacts;
          });
        };

        apps = {
          cdk-mintd = flake-utils.lib.mkApp {
            drv = cdk-mintd;
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
              cargo update
              cargo update -p async-compression --precise 0.4.3

              cargo update -p home --precise 0.5.5
              cargo update -p zerofrom --precise 0.1.5

              cargo update -p url --precise 2.5.2

              # For wasm32-unknown-unknown target
              cargo update -p triomphe --precise 0.1.11

              ";
              buildInputs = buildInputs ++ WASMInputs ++ [ msrv_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

            stable = pkgs.mkShell ({
              shellHook = ''${_shellHook}'';
              buildInputs = buildInputs ++ WASMInputs ++ [ stable_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);


            nightly = pkgs.mkShell ({
              shellHook = ''
                ${_shellHook}
                # Needed for github ci
                export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath [
                  pkgs.zlib
                  ]}:$LD_LIBRARY_PATH
                export RUST_SRC_PATH=${nightly_toolchain}/lib/rustlib/src/rust/library
              '';
              buildInputs = buildInputs ++ [ nightly_toolchain ];
              inherit nativeBuildInputs;
            } // envVars);

          in
          {
            inherit msrv stable nightly;
            default = stable;
          };
      }
    );
}
