{
  description = "CDK Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

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
    };
  };

  outputs =
    { self
    , nixpkgs
    , rust-overlay
    , flake-utils
    , crane
    , ...
    }@inputs:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        lib = pkgs.lib;
        stdenv = pkgs.stdenv;
        isDarwin = stdenv.isDarwin;
        libsDarwin =
          with pkgs;
          lib.optionals isDarwin [
            # Additional darwin specific inputs can be set here
            # Note: Security and SystemConfiguration frameworks are provided by the default SDK
          ];

        # Dependencies
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Toolchains
        # latest stable
        stable_toolchain = pkgs.rust-bin.stable."1.92.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
          extensions = [
            "rustfmt"
            "clippy"
            "rust-analyzer"
          ];
        };

        # MSRV stable
        msrv_toolchain = pkgs.rust-bin.stable."1.85.0".default.override {
          targets = [ "wasm32-unknown-unknown" ]; # wasm
          extensions = [
            "rustfmt"
            "clippy"
            "rust-analyzer"
          ];
        };

        # Nightly used for formatting
        nightly_toolchain = pkgs.rust-bin.selectLatestNightlyWith (
          toolchain:
          toolchain.default.override {
            extensions = [
              "rustfmt"
              "clippy"
              "rust-analyzer"
              "rust-src"
            ];
            targets = [ "wasm32-unknown-unknown" ]; # wasm
          }
        );

        # ========================================
        # Crane setup for cached builds
        # ========================================
        craneLib = (crane.mkLib pkgs).overrideToolchain stable_toolchain;
        craneLibMsrv = (crane.mkLib pkgs).overrideToolchain msrv_toolchain;

        # Source for crane builds
        src = builtins.path {
          path = ./.;
          name = "cdk-source";
        };

        # Source for MSRV builds - uses Cargo.lock.msrv with MSRV-compatible deps
        srcMsrv = pkgs.runCommand "cdk-source-msrv" { } ''
          cp -r ${src} $out
          chmod -R +w $out
          cp $out/Cargo.lock.msrv $out/Cargo.lock
        '';

        # Common args for all Crane builds
        commonCraneArgs = {
          inherit src;
          pname = "cdk";
          version = "0.14.0";

          nativeBuildInputs = with pkgs; [
            pkg-config
            protobuf
          ];

          buildInputs = with pkgs; [
            openssl
            sqlite
            zlib
          ] ++ libsDarwin;

          # Environment variables
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          PROTOC_INCLUDE = "${pkgs.protobuf}/include";
        };

        # Common args for MSRV builds - uses srcMsrv with pinned deps
        commonCraneArgsMsrv = commonCraneArgs // {
          src = srcMsrv;
        };

        # Build ALL dependencies once - this is what gets cached by Cachix
        # Note: We exclude swagger feature as it tries to download assets during build
        workspaceDeps = craneLib.buildDepsOnly (commonCraneArgs // {
          pname = "cdk-deps";
          # Build deps for workspace - swagger excluded (downloads during build)
          cargoExtraArgs = "--workspace";
        });

        # MSRV dependencies (separate cache due to different toolchain)
        workspaceDepsMsrv = craneLibMsrv.buildDepsOnly (commonCraneArgsMsrv // {
          pname = "cdk-deps-msrv";
          cargoExtraArgs = "--workspace";
        });

        # Helper function to create clippy checks
        mkClippy = name: cargoArgs: craneLib.cargoClippy (commonCraneArgs // {
          pname = "cdk-clippy-${name}";
          cargoArtifacts = workspaceDeps;
          cargoClippyExtraArgs = "${cargoArgs} -- -D warnings";
        });

        # Helper function to create example checks (compile only, no network access in sandbox)
        mkExample = name: craneLib.mkCargoDerivation (commonCraneArgs // {
          pname = "cdk-example-${name}";
          cargoArtifacts = workspaceDeps;
          buildPhaseCargoCommand = "cargo build --example ${name}";
          # Examples are compiled but not run (no network in Nix sandbox)
          installPhaseCommand = "mkdir -p $out";
        });

        # Helper function to create example packages (outputs binary for running outside sandbox)
        mkExamplePackage = name: craneLib.mkCargoDerivation (commonCraneArgs // {
          pname = "cdk-example-${name}";
          cargoArtifacts = workspaceDeps;
          buildPhaseCargoCommand = "cargo build --release --example ${name}";
          installPhaseCommand = ''
            mkdir -p $out/bin
            cp target/release/examples/${name} $out/bin/
          '';
        });

        # Helper function to create MSRV build checks
        mkMsrvBuild = name: cargoArgs: craneLibMsrv.cargoBuild (commonCraneArgsMsrv // {
          pname = "cdk-msrv-${name}";
          cargoArtifacts = workspaceDepsMsrv;
          cargoExtraArgs = cargoArgs;
        });

        # Helper function to create WASM build checks
        # WASM builds don't need native libs like openssl
        mkWasmBuild = name: cargoArgs: craneLib.cargoBuild ({
          inherit src;
          pname = "cdk-wasm-${name}";
          version = "0.14.0";
          cargoArtifacts = workspaceDeps;
          cargoExtraArgs = "${cargoArgs} --target wasm32-unknown-unknown";
          # WASM doesn't need native build inputs
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = [ ];
          # Disable tests for WASM (can't run in sandbox)
          doCheck = false;
        });

        # Doc tests check
        docTests = craneLib.cargoTest (commonCraneArgs // {
          pname = "cdk-doc-tests";
          cargoArtifacts = workspaceDeps;
          cargoTestExtraArgs = "--doc";
        });

        # Strict docs check - build docs with warnings as errors
        # Uses mkCargoDerivation for custom RUSTDOCFLAGS
        strictDocs = craneLib.mkCargoDerivation (commonCraneArgs // {
          pname = "cdk-strict-docs";
          cargoArtifacts = workspaceDeps;
          buildPhaseCargoCommand = ''
            export RUSTDOCFLAGS="-D warnings"
            cargo doc --no-deps \
              -p cashu \
              -p cdk-common \
              -p cdk-sql-common \
              -p cdk \
              -p cdk-redb \
              -p cdk-sqlite \
              -p cdk-axum \
              -p cdk-cln \
              -p cdk-lnd \
              -p cdk-lnbits \
              -p cdk-fake-wallet \
              -p cdk-mint-rpc \
              -p cdk-payment-processor \
              -p cdk-signatory \
              -p cdk-cli \
              -p cdk-mintd
          '';
          installPhaseCommand = "mkdir -p $out";
        });

        # FFI Python tests
        ffiTests = craneLib.mkCargoDerivation (commonCraneArgs // {
          pname = "cdk-ffi-tests";
          cargoArtifacts = workspaceDeps;
          nativeBuildInputs = commonCraneArgs.nativeBuildInputs ++ [
            pkgs.python311
          ];
          buildPhaseCargoCommand = ''
            # Build the FFI library
            cargo build --release --package cdk-ffi --features postgres

            # Generate Python bindings
            cargo run --bin uniffi-bindgen generate \
              --library target/release/libcdk_ffi.so \
              --language python \
              --out-dir target/bindings/python

            # Copy library to bindings directory
            cp target/release/libcdk_ffi.so target/bindings/python/

            # Run Python tests
            python3 crates/cdk-ffi/tests/test_transactions.py
          '';
          installPhaseCommand = "mkdir -p $out";
        });

        # ========================================
        # Example definitions - single source of truth
        # ========================================
        exampleChecks = [
          "mint-token"
          "melt-token"
          "p2pk"
          "proof-selection"
          "wallet"
        ];

        # ========================================
        # Clippy check definitions - single source of truth
        # ========================================
        clippyChecks = {
          # Core crate: cashu
          "cashu" = "-p cashu";
          "cashu-no-default" = "-p cashu --no-default-features";
          "cashu-wallet" = "-p cashu --no-default-features --features wallet";
          "cashu-mint" = "-p cashu --no-default-features --features mint";
          "cashu-auth" = "-p cashu --no-default-features --features auth";

          # Core crate: cdk-common
          "cdk-common" = "-p cdk-common";
          "cdk-common-no-default" = "-p cdk-common --no-default-features";
          "cdk-common-wallet" = "-p cdk-common --no-default-features --features wallet";
          "cdk-common-mint" = "-p cdk-common --no-default-features --features mint";
          "cdk-common-auth" = "-p cdk-common --no-default-features --features auth";

          # Core crate: cdk
          "cdk" = "-p cdk";
          "cdk-no-default" = "-p cdk --no-default-features";
          "cdk-wallet" = "-p cdk --no-default-features --features wallet";
          "cdk-mint" = "-p cdk --no-default-features --features mint";
          "cdk-auth" = "-p cdk --no-default-features --features auth";

          # SQL crates
          "cdk-sql-common" = "-p cdk-sql-common";
          "cdk-sql-common-wallet" = "-p cdk-sql-common --no-default-features --features wallet";
          "cdk-sql-common-mint" = "-p cdk-sql-common --no-default-features --features mint";

          # Database crates
          "cdk-redb" = "-p cdk-redb";
          "cdk-sqlite" = "-p cdk-sqlite";
          "cdk-sqlite-sqlcipher" = "-p cdk-sqlite --features sqlcipher";

          # HTTP/API layer
          # Note: swagger feature excluded - downloads assets during build, incompatible with Nix sandbox
          "cdk-axum" = "-p cdk-axum";
          "cdk-axum-no-default" = "-p cdk-axum --no-default-features";
          "cdk-axum-redis" = "-p cdk-axum --no-default-features --features redis";

          # Lightning backends
          "cdk-cln" = "-p cdk-cln";
          "cdk-lnd" = "-p cdk-lnd";
          "cdk-lnbits" = "-p cdk-lnbits";
          "cdk-fake-wallet" = "-p cdk-fake-wallet";
          "cdk-payment-processor" = "-p cdk-payment-processor";
          "cdk-ldk-node" = "-p cdk-ldk-node";

          # Other crates
          "cdk-signatory" = "-p cdk-signatory";
          "cdk-mint-rpc" = "-p cdk-mint-rpc";
          "cdk-prometheus" = "-p cdk-prometheus";
          "cdk-ffi" = "-p cdk-ffi";

          # Binaries: cdk-cli
          "bin-cdk-cli" = "--bin cdk-cli";
          "bin-cdk-cli-sqlcipher" = "--bin cdk-cli --features sqlcipher";
          "bin-cdk-cli-redb" = "--bin cdk-cli --features redb";

          # Binaries: cdk-mintd
          "bin-cdk-mintd" = "--bin cdk-mintd";
          "bin-cdk-mintd-redis" = "--bin cdk-mintd --features redis";
          "bin-cdk-mintd-sqlcipher" = "--bin cdk-mintd --features sqlcipher";
          "bin-cdk-mintd-lnd-sqlite" = "--bin cdk-mintd --no-default-features --features lnd,sqlite";
          "bin-cdk-mintd-cln-postgres" = "--bin cdk-mintd --no-default-features --features cln,postgres";
          "bin-cdk-mintd-lnbits-sqlite" = "--bin cdk-mintd --no-default-features --features lnbits,sqlite";
          "bin-cdk-mintd-fakewallet-sqlite" = "--bin cdk-mintd --no-default-features --features fakewallet,sqlite";
          "bin-cdk-mintd-grpc-processor-sqlite" = "--bin cdk-mintd --no-default-features --features grpc-processor,sqlite";
          "bin-cdk-mintd-management-rpc-lnd-sqlite" = "--bin cdk-mintd --no-default-features --features management-rpc,lnd,sqlite";
          "bin-cdk-mintd-cln-sqlite" = "--bin cdk-mintd --no-default-features --features cln,sqlite";
          "bin-cdk-mintd-lnd-postgres" = "--bin cdk-mintd --no-default-features --features lnd,postgres";
          "bin-cdk-mintd-lnbits-postgres" = "--bin cdk-mintd --no-default-features --features lnbits,postgres";
          "bin-cdk-mintd-fakewallet-postgres" = "--bin cdk-mintd --no-default-features --features fakewallet,postgres";
          "bin-cdk-mintd-grpc-processor-postgres" = "--bin cdk-mintd --no-default-features --features grpc-processor,postgres";
          "bin-cdk-mintd-management-rpc-cln-postgres" = "--bin cdk-mintd --no-default-features --features management-rpc,cln,postgres";
          "bin-cdk-mintd-auth-sqlite-fakewallet" = "--bin cdk-mintd --no-default-features --features auth,sqlite,fakewallet";
          "bin-cdk-mintd-auth-postgres-lnd" = "--bin cdk-mintd --no-default-features --features auth,postgres,lnd";

          # Binaries: cdk-mint-cli
          "bin-cdk-mint-cli" = "--bin cdk-mint-cli";
        };

        # ========================================
        # MSRV build check definitions
        # ========================================
        msrvChecks = {
          # Core library with all features (except swagger which breaks MSRV)
          "cdk-all-features" = "-p cdk --features \"mint,wallet,auth\"";

          # Mintd with all backends, databases, and features (no swagger)
          "cdk-mintd-all" = "-p cdk-mintd --no-default-features --features \"cln,lnd,lnbits,fakewallet,ldk-node,grpc-processor,sqlite,postgres,auth,redis,management-rpc\"";

          # CLI - default features (excludes redb which breaks MSRV)
          "cdk-cli" = "-p cdk-cli";

          # Minimal builds to ensure no-default-features works
          "cdk-wallet-only" = "-p cdk --no-default-features --features wallet";
        };

        # ========================================
        # WASM build check definitions
        # ========================================
        wasmChecks = {
          "cdk" = "-p cdk";
          "cdk-no-default" = "-p cdk --no-default-features";
          "cdk-wallet" = "-p cdk --no-default-features --features wallet";
        };

        # Common inputs
        envVars = {
          # rust analyzer needs  NIX_PATH for some reason.
          NIX_PATH = "nixpkgs=${inputs.nixpkgs}";
        };
        # Override clightning to include mako dependency and fix compilation bug
        clightningWithMako = pkgs.clightning.overrideAttrs (oldAttrs: {
          nativeBuildInputs = (oldAttrs.nativeBuildInputs or [ ]) ++ [
            pkgs.python311Packages.mako
          ];

          # Disable -Werror to work around multiple compilation bugs in 25.09.2 on macOS
          # See: https://github.com/ElementsProject/lightning/issues/7961
          env = (oldAttrs.env or { }) // {
            NIX_CFLAGS_COMPILE = toString ((oldAttrs.env.NIX_CFLAGS_COMPILE or "") + " -Wno-error");
          };
        });

        buildInputs =
          with pkgs;
          [
            # Add additional build inputs here
            git
            pkg-config
            curl
            just
            protobuf
            nixpkgs-fmt
            typos
            lnd
            clightningWithMako
            bitcoind
            sqlx-cli
            mprocs

            cargo-outdated
            cargo-mutants

            # Needed for github ci
            libz
          ]
          ++ libsDarwin;

        # PostgreSQL configuration
        postgresConf = {
          pgUser = "cdk_user";
          pgPassword = "cdk_password";
          pgDatabase = "cdk_mint";
          pgPort = "5432";
        };

        # Script to start PostgreSQL
        startPostgres = pkgs.writeShellScriptBin "start-postgres" ''
          set -e
          PGDATA="$PWD/.pg_data"
          PGPORT="${postgresConf.pgPort}"
          PGUSER="${postgresConf.pgUser}"
          PGPASSWORD="${postgresConf.pgPassword}"
          PGDATABASE="${postgresConf.pgDatabase}"

          # Stop any existing instance first
          if [ -d "$PGDATA" ] && ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" status > /dev/null 2>&1; then
            echo "Stopping existing PostgreSQL instance..."
            ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" stop > /dev/null 2>&1
          fi

          if [ ! -d "$PGDATA" ]; then
            echo "Initializing PostgreSQL database..."
            ${pkgs.postgresql_16}/bin/initdb -D "$PGDATA" --auth=trust --no-locale --encoding=UTF8

            # Configure PostgreSQL
            echo "listen_addresses = 'localhost'" >> "$PGDATA/postgresql.conf"
            echo "port = $PGPORT" >> "$PGDATA/postgresql.conf"
            echo "unix_socket_directories = '$PGDATA'" >> "$PGDATA/postgresql.conf"

            # Start temporarily to create user and database
            ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" -l "$PGDATA/logfile" start
            sleep 2

            # Create user and database
            ${pkgs.postgresql_16}/bin/createuser -h localhost -p $PGPORT -s "$PGUSER" || true
            ${pkgs.postgresql_16}/bin/psql -h localhost -p $PGPORT -c "ALTER USER $PGUSER WITH PASSWORD '$PGPASSWORD';" postgres
            ${pkgs.postgresql_16}/bin/createdb -h localhost -p $PGPORT -O "$PGUSER" "$PGDATABASE" || true

            ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" stop
            echo "PostgreSQL initialized."
          fi

          echo "Starting PostgreSQL on port $PGPORT..."
          ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" -l "$PGDATA/logfile" start
          echo "PostgreSQL started. Connection URL: postgresql://$PGUSER:$PGPASSWORD@localhost:$PGPORT/$PGDATABASE"
        '';

        # Script to stop PostgreSQL
        stopPostgres = pkgs.writeShellScriptBin "stop-postgres" ''
          PGDATA="$PWD/.pg_data"
          if [ -d "$PGDATA" ]; then
            echo "Stopping PostgreSQL..."
            ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" stop || echo "PostgreSQL was not running."
          else
            echo "No PostgreSQL data directory found."
          fi
        '';

        # Script to check PostgreSQL status
        pgStatus = pkgs.writeShellScriptBin "pg-status" ''
          PGDATA="$PWD/.pg_data"
          if [ -d "$PGDATA" ]; then
            ${pkgs.postgresql_16}/bin/pg_ctl -D "$PGDATA" status
          else
            echo "No PostgreSQL data directory found. Run 'start-postgres' first."
          fi
        '';

        # Script to connect to PostgreSQL
        pgConnect = pkgs.writeShellScriptBin "pg-connect" ''
          ${pkgs.postgresql_16}/bin/psql "postgresql://${postgresConf.pgUser}:${postgresConf.pgPassword}@localhost:${postgresConf.pgPort}/${postgresConf.pgDatabase}"
        '';

        # Common arguments can be set here to avoid repeating them later
        nativeBuildInputs = [
          #Add additional build inputs here
        ]
        ++ lib.optionals isDarwin [
          # Additional darwin specific native inputs can be set here
        ];
      in
      {
        # Expose deps for explicit cache warming
        packages = {
          deps = workspaceDeps;
          deps-msrv = workspaceDepsMsrv;
        }
        # Example packages (binaries that can be run outside sandbox with network access)
        // (builtins.listToAttrs (map (name: { name = "example-${name}"; value = mkExamplePackage name; }) exampleChecks));
        checks =
          # Generate clippy checks from clippyChecks attrset
          (builtins.mapAttrs (name: args: mkClippy name args) clippyChecks)
          # Generate MSRV build checks (prefixed with msrv-)
          // (builtins.listToAttrs (map (name: { name = "msrv-${name}"; value = mkMsrvBuild name msrvChecks.${name}; }) (builtins.attrNames msrvChecks)))
          # Generate WASM build checks (prefixed with wasm-)
          // (builtins.listToAttrs (map (name: { name = "wasm-${name}"; value = mkWasmBuild name wasmChecks.${name}; }) (builtins.attrNames wasmChecks)))
          # Generate example checks from exampleChecks list
          // (builtins.listToAttrs (map (name: { name = "example-${name}"; value = mkExample name; }) exampleChecks))
          // {
            # Doc tests
            doc-tests = docTests;

            # Strict docs check
            strict-docs = strictDocs;

            # FFI Python tests
            ffi-tests = ffiTests;
          };

        devShells =
          let
            # devShells
            msrv = pkgs.mkShell (
              {
                shellHook = "
                  cargo update
                  cargo update home --precise 0.5.11
                  cargo update typed-index-collections --precise 3.3.0
              ";
                buildInputs = buildInputs ++ [ msrv_toolchain ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            stable = pkgs.mkShell (
              {
                shellHook = ''
                  # Needed for github ci
                  export LD_LIBRARY_PATH=${
                    pkgs.lib.makeLibraryPath [
                      pkgs.zlib
                    ]
                  }:$LD_LIBRARY_PATH

                  # PostgreSQL environment variables
                  export CDK_MINTD_DATABASE_URL="postgresql://${postgresConf.pgUser}:${postgresConf.pgPassword}@localhost:${postgresConf.pgPort}/${postgresConf.pgDatabase}"

                  echo ""
                  echo "PostgreSQL commands available:"
                  echo "  start-postgres  - Initialize and start PostgreSQL"
                  echo "  stop-postgres   - Stop PostgreSQL (run before exiting)"
                  echo "  pg-status       - Check PostgreSQL status"
                  echo "  pg-connect      - Connect to PostgreSQL with psql"
                  echo ""
                '';
                buildInputs = buildInputs ++ [
                  stable_toolchain
                  pkgs.postgresql_16
                  startPostgres
                  stopPostgres
                  pgStatus
                  pgConnect
                ];
                inherit nativeBuildInputs;

              }
              // envVars
            );

            nightly = pkgs.mkShell (
              {
                shellHook = ''
                  # Needed for github ci
                  export LD_LIBRARY_PATH=${
                    pkgs.lib.makeLibraryPath [
                      pkgs.zlib
                    ]
                  }:$LD_LIBRARY_PATH

                  # PostgreSQL environment variables
                  export CDK_MINTD_DATABASE_URL="postgresql://${postgresConf.pgUser}:${postgresConf.pgPassword}@localhost:${postgresConf.pgPort}/${postgresConf.pgDatabase}"

                  echo ""
                  echo "PostgreSQL commands available:"
                  echo "  start-postgres  - Initialize and start PostgreSQL"
                  echo "  stop-postgres   - Stop PostgreSQL (run before exiting)"
                  echo "  pg-status       - Check PostgreSQL status"
                  echo "  pg-connect      - Connect to PostgreSQL with psql"
                  echo ""
                '';
                buildInputs = buildInputs ++ [
                  nightly_toolchain
                  pkgs.postgresql_16
                  startPostgres
                  stopPostgres
                  pgStatus
                  pgConnect
                ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            # Shell with Docker for integration tests
            integration = pkgs.mkShell (
              {
                shellHook = ''
                  # Ensure Docker is available
                  if ! command -v docker &> /dev/null; then
                    echo "Docker is not installed or not in PATH"
                    echo "Please install Docker to run integration tests"
                    exit 1
                  fi
                  echo "Docker is available at $(which docker)"
                  echo "Docker version: $(docker --version)"
                '';
                buildInputs = buildInputs ++ [
                  stable_toolchain
                  pkgs.docker-client
                  pkgs.python311
                ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            # Shell for FFI development (Python bindings)
            ffi = pkgs.mkShell (
              {
                shellHook = ''
                  echo "FFI development shell"
                  echo "  just ffi-test        - Run Python FFI tests"
                  echo "  just ffi-dev-python  - Launch Python REPL with CDK FFI"
                '';
                buildInputs = buildInputs ++ [
                  stable_toolchain
                  pkgs.python311
                ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

          in
          {
            inherit
              msrv
              stable
              nightly
              integration
              ffi
              ;
            default = stable;
          };
      }
    );
}
