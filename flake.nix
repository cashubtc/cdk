{
  description = "CDK Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

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
    , nixpkgs-unstable
    , rust-overlay
    , flake-utils
    , crane
    , ...
    }@inputs:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        # Architecture-specific configuration for static musl builds
        muslTarget = {
          "x86_64-linux" = "x86_64-unknown-linux-musl";
          "aarch64-linux" = "aarch64-unknown-linux-musl";
        }.${system} or null;

        archSuffix = {
          "x86_64-linux" = "x86_64";
          "aarch64-linux" = "aarch64";
        }.${system} or null;

        cargoTargetEnvName = {
          "x86_64-linux" = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER";
          "aarch64-linux" = "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER";
        }.${system} or null;

        overlays = [ (import rust-overlay) ];

        # Derive version from Cargo.toml so there is a single source of truth
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

        lib = pkgs.lib;
        stdenv = pkgs.stdenv;
        isDarwin = stdenv.isDarwin;
        libsDarwin =
          lib.optionals isDarwin [
            # Additional drwin specific inputs can be set here
            # Note: Security and SystemConfiguration frameworks are provided by the default SDK
          ];

        # Dependencies
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        pkgsUnstable = import nixpkgs-unstable {
          inherit system;
        };

        # Static/musl packages for fully static binary builds (Linux only)
        pkgsMusl = import nixpkgs {
          localSystem = system;
          crossSystem = {
            config = muslTarget;
            isStatic = true;
          };
        };

        # Toolchains
        # latest stable
        stable_toolchain = pkgs.rust-bin.stable."1.93.0".default.override {
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

        # Stable toolchain with musl target for static builds
        static_toolchain = pkgs.rust-bin.stable."1.93.0".default.override {
          targets = [ muslTarget ];
        };

        # ========================================
        # Crane setup for cached builds
        # ========================================
        craneLib = (crane.mkLib pkgs).overrideToolchain stable_toolchain;
        craneLibMsrv = (crane.mkLib pkgs).overrideToolchain msrv_toolchain;
        craneLibStatic = (crane.mkLib pkgs).overrideToolchain static_toolchain;

        # Source for crane builds - uses lib.fileset for efficient filtering
        # This is much faster than nix-gitignore when large directories (like target/) exist
        # because it uses a whitelist approach rather than scanning everything first
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.intersection
            (lib.fileset.fromSource (lib.sources.cleanSource ./.))
            (lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./Cargo.lock.msrv
              ./README.md
              ./.cargo
              ./crates
              ./fuzz
            ]);
        };

        # Source for MSRV builds - uses Cargo.lock.msrv with MSRV-compatible deps
        # Use lib.fileset approach (same as src) but substitute Cargo.lock with Cargo.lock.msrv
        # We include both lock files and use cargoLock override to point to MSRV version
        srcMsrv = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.intersection
            (lib.fileset.fromSource (lib.sources.cleanSource ./.))
            (lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock.msrv
              ./README.md
              ./.cargo
              ./crates
              ./fuzz
            ]);
        };

        # Common args for all Crane builds
        commonCraneArgs = {
          inherit src version;
          pname = "cdk";

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
        # Override cargoLock to use Cargo.lock.msrv instead of Cargo.lock
        commonCraneArgsMsrv = commonCraneArgs // {
          src = srcMsrv;
          cargoLock = ./Cargo.lock.msrv;
        };

        # Musl-targeting C compiler for crates that compile bundled C code
        # (libsqlite3-sys, secp256k1-sys, aws-lc-sys, etc.)
        muslCC = pkgs.pkgsStatic.stdenv.cc;

        # Common args for static musl builds (Linux only)
        # Produces fully statically-linked binaries that run on any Linux system
        commonCraneArgsStatic = {
          inherit src version;
          pname = "cdk-static";

          # Cross-compile to musl for fully static linking
          CARGO_BUILD_TARGET = muslTarget;
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";

          # Host-side build tools (run on build machine)
          nativeBuildInputs = with pkgs; [
            pkg-config
            protobuf
            muslCC
          ];

          # Target-side libraries (musl static libs linked into the binary)
          buildInputs = with pkgsMusl; [
            openssl.dev
            zlib.static
          ];

          # Tell the cc crate and cargo to use the musl-targeting C compiler/linker
          TARGET_CC = "${muslCC}/bin/${muslCC.targetPrefix}cc";

          # Force static OpenSSL linking (needed by postgres/native-tls)
          OPENSSL_STATIC = "1";
          OPENSSL_DIR = "${pkgsMusl.openssl.dev}";
          OPENSSL_LIB_DIR = "${pkgsMusl.openssl.out}/lib";
          OPENSSL_INCLUDE_DIR = "${pkgsMusl.openssl.dev}/include";

          # Protobuf (build-time code generation, runs on host)
          PROTOC = "${pkgs.protobuf}/bin/protoc";
          PROTOC_INCLUDE = "${pkgs.protobuf}/include";

          # Tell pkg-config to find musl static libraries
          PKG_CONFIG_ALL_STATIC = "1";

          # Use the release-static profile for reproducible, optimized builds
          CARGO_PROFILE = "release-static";
        } // {
          # Dynamic attribute name for the cargo linker env var (arch-specific)
          ${cargoTargetEnvName} = "${muslCC}/bin/${muslCC.targetPrefix}cc";
        };

        # Build ALL dependencies once - this is what gets cached by Cachix
        # Note: We exclude swagger feature as it tries to download assets during build
        workspaceDeps = craneLib.buildDepsOnly (commonCraneArgs // {
          pname = "cdk-deps";
          # Build deps for workspace - swagger excluded (downloads during build)
          cargoExtraArgs = "--workspace";
        });

        # MSRV dependencies (separate cache due to different toolchain)
        # Exclude cdk-redb (and its dependents) since redb requires a higher MSRV
        workspaceDepsMsrv = craneLibMsrv.buildDepsOnly (commonCraneArgsMsrv // {
          pname = "cdk-deps-msrv";
          cargoExtraArgs = "--workspace --exclude cdk-redb --exclude cdk-integration-tests";
        });

        # Static musl dependencies (separate cache for static builds)
        workspaceDepsStatic = craneLibStatic.buildDepsOnly (commonCraneArgsStatic // {
          pname = "cdk-deps-static";
          cargoExtraArgs = "--workspace";
        });

        # Helper function to create combined clippy + test checks
        # Runs both in a single derivation to share build artifacts
        mkClippyAndTest = name: cargoArgs: craneLib.mkCargoDerivation (commonCraneArgs // {
          pname = "cdk-check-${name}";
          cargoArtifacts = workspaceDeps;
          buildPhaseCargoCommand = ''
            cargo clippy ${cargoArgs} -- -D warnings
            cargo test ${cargoArgs}
          '';
          installPhaseCommand = "mkdir -p $out";
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
          inherit src version;
          pname = "cdk-wasm-${name}";
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
            python3 crates/cdk-ffi/tests/test_kvstore.py
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
        # Clippy + test check definitions - single source of truth
        # These run both clippy and unit tests in a single derivation
        # ========================================
        clippyAndTestChecks = {
          # Core crate: cashu
          "cashu" = "-p cashu";
          "cashu-no-default" = "-p cashu --no-default-features";
          "cashu-wallet" = "-p cashu --no-default-features --features wallet";
          "cashu-mint" = "-p cashu --no-default-features --features mint";

          # Core crate: cdk-common
          "cdk-common" = "-p cdk-common";
          "cdk-common-no-default" = "-p cdk-common --no-default-features";
          "cdk-common-wallet" = "-p cdk-common --no-default-features --features wallet";
          "cdk-common-mint" = "-p cdk-common --no-default-features --features mint";

          # Core crate: cdk
          "cdk" = "-p cdk";
          "cdk-no-default" = "-p cdk --no-default-features";
          "cdk-wallet" = "-p cdk --no-default-features --features wallet";
          "cdk-mint" = "-p cdk --no-default-features --features mint";

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
          "cdk-npubcash" = "-p cdk-npubcash";

          # Binaries: cdk-cli
          "cdk-cli" = "-p cdk-cli";
          "cdk-cli-sqlcipher" = "-p cdk-cli --features sqlcipher";
          "cdk-cli-redb" = "-p cdk-cli --features redb";

          # Binaries: cdk-mintd
          "cdk-mintd" = "-p cdk-mintd";
          "cdk-mintd-redis" = "-p cdk-mintd --features redis";
          "cdk-mintd-sqlcipher" = "-p cdk-mintd --features sqlcipher";
          "cdk-mintd-lnd-sqlite" = "-p cdk-mintd --no-default-features --features lnd,sqlite";
          "cdk-mintd-cln-postgres" = "-p cdk-mintd --no-default-features --features cln,postgres";
          "cdk-mintd-lnbits-sqlite" = "-p cdk-mintd --no-default-features --features lnbits,sqlite";
          "cdk-mintd-fakewallet-sqlite" = "-p cdk-mintd --no-default-features --features fakewallet,sqlite";
          "cdk-mintd-grpc-processor-sqlite" = "-p cdk-mintd --no-default-features --features grpc-processor,sqlite";
          "cdk-mintd-management-rpc-lnd-sqlite" = "-p cdk-mintd --no-default-features --features management-rpc,lnd,sqlite";
          "cdk-mintd-cln-sqlite" = "-p cdk-mintd --no-default-features --features cln,sqlite";
          "cdk-mintd-lnd-postgres" = "-p cdk-mintd --no-default-features --features lnd,postgres";
          "cdk-mintd-lnbits-postgres" = "-p cdk-mintd --no-default-features --features lnbits,postgres";
          "cdk-mintd-fakewallet-postgres" = "-p cdk-mintd --no-default-features --features fakewallet,postgres";
          "cdk-mintd-grpc-processor-postgres" = "-p cdk-mintd --no-default-features --features grpc-processor,postgres";
          "cdk-mintd-management-rpc-cln-postgres" = "-p cdk-mintd --no-default-features --features management-rpc,cln,postgres";

          # Binaries: cdk-mint-cli (binary name, package is cdk-mint-rpc)
          "cdk-mint-cli" = "-p cdk-mint-rpc";
        };

        # ========================================
        # MSRV build check definitions
        # ========================================
        msrvChecks = {
          # Core library with all features (except swagger which breaks MSRV)
          "cdk-all-features" = "-p cdk --features \"mint,wallet\"";

          # Mintd with all backends, databases, and features (no swagger)
          "cdk-mintd-all" = "-p cdk-mintd --no-default-features --features \"cln,lnd,lnbits,fakewallet,ldk-node,grpc-processor,sqlite,postgres,redis,management-rpc\"";

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

        baseBuildInputs =
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

            cargo-outdated
            cargo-mutants
            cargo-fuzz
            cargo-nextest

            # Database
            postgresql_16
            startPostgres
            stopPostgres
            pgStatus
            pgConnect

            # Needed for github ci
            libz
          ]
          ++ libsDarwin;

        regtestBuildInputs =
          with pkgs;
          [
            lnd
            pkgsUnstable.clightning
            bitcoind
            mprocs
          ];

        commonShellHook = ''
          # Needed for github ci
          export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath [ pkgs.zlib ]}:$LD_LIBRARY_PATH
        '';

        pgShellHook = ''
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

        # Helper to build a statically-linked binary package
        # bin: the cargo binary name (e.g. "cdk-mintd")
        # name: the output binary name prefix (e.g. "cdk-mintd-ldk")
        # cargoExtraArgs: additional cargo args (e.g. "--bin cdk-mintd --features ldk-node")
        staticVersion = commonCraneArgsStatic.version;
        mkStaticPackage = { bin, name, cargoExtraArgs }: craneLibStatic.buildPackage (commonCraneArgsStatic // {
          pname = name;
          cargoArtifacts = workspaceDepsStatic;
          inherit cargoExtraArgs;
          nativeBuildInputs = commonCraneArgsStatic.nativeBuildInputs ++ [
            pkgs.removeReferencesTo
          ];
          installPhaseCommand = ''
            mkdir -p $out/bin
            cp target/${muslTarget}/release-static/${bin} $out/bin/${name}-${staticVersion}-${archSuffix}
          '';
          # Strip Nix store references from binaries for reproducibility
          postFixup = ''
            find "$out" -type f -executable -exec remove-references-to -t ${static_toolchain} '{}' +
          '';
        });

        # ========================================
        # Integration test harness binaries (pre-built via Crane, cached by Cachix)
        # These are used by CI integration test scripts instead of cargo build/run
        # ========================================
        mkItestBinary = name: cargoExtraArgs: craneLib.buildPackage (commonCraneArgs // {
          pname = "cdk-itest-${name}";
          cargoArtifacts = workspaceDeps;
          inherit cargoExtraArgs;
          # Only install the specific binary, not the entire workspace
          doCheck = false;
        });

        itestBinaries = {
          start-fake-mint = mkItestBinary "start-fake-mint" "--bin start_fake_mint";
          start-regtest-mints = mkItestBinary "start-regtest-mints" "--bin start_regtest_mints";
          start-fake-auth-mint = mkItestBinary "start-fake-auth-mint" "--bin start_fake_auth_mint";
          start-regtest = mkItestBinary "start-regtest" "--bin start_regtest";
          signatory = mkItestBinary "signatory" "--bin signatory";
          cdk-payment-processor = mkItestBinary "cdk-payment-processor" "--bin cdk-payment-processor";
          cdk-mintd-grpc = mkItestBinary "cdk-mintd-grpc" "--bin cdk-mintd --no-default-features --features grpc-processor";
        };

        # Nextest archive for integration tests (pre-compiled test binaries)
        itestArchive = craneLib.mkCargoDerivation (commonCraneArgs // {
          pname = "cdk-itest-archive";
          cargoArtifacts = workspaceDeps;
          nativeBuildInputs = commonCraneArgs.nativeBuildInputs ++ [
            pkgs.cargo-nextest
          ];
          buildPhaseCargoCommand = ''
            mkdir -p $out
            cargo nextest archive \
              -p cdk-integration-tests \
              --archive-file $out/itest-archive.tar.zst
          '';
          installPhaseCommand = "";
        });

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
          deps-static = workspaceDepsStatic;
        }
        # Static binary packages (fully statically-linked, runs on any x86_64 Linux)
        // {
          cdk-mintd-static = mkStaticPackage {
            bin = "cdk-mintd";
            name = "cdk-mintd";
            cargoExtraArgs = "--bin cdk-mintd --features postgres,prometheus,redis";
          };

          cdk-mintd-ldk-static = mkStaticPackage {
            bin = "cdk-mintd";
            name = "cdk-mintd-ldk";
            cargoExtraArgs = "--bin cdk-mintd --features ldk-node,postgres,prometheus,redis";
          };

          cdk-cli-static = mkStaticPackage {
            bin = "cdk-cli";
            name = "cdk-cli";
            cargoExtraArgs = "--bin cdk-cli";
          };
        }
        # Integration test harness binaries (pre-built for CI)
        // itestBinaries
        // {
          itest-archive = itestArchive;
        }
        # Example packages (binaries that can be run outside sandbox with network access)
        // (builtins.listToAttrs (map (name: { name = "example-${name}"; value = mkExamplePackage name; }) exampleChecks));
        checks =
          # Generate clippy + test checks from clippyAndTestChecks attrset
          (builtins.mapAttrs (name: args: mkClippyAndTest name args) clippyAndTestChecks)
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
                  cargo update simple_asn1 --precise 0.6.3
                  cargo update cookie_store --precise 0.22.0
                  cargo update time --precise 0.3.44
               ";
                buildInputs = baseBuildInputs ++ [ msrv_toolchain ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            stable = pkgs.mkShell (
              {
                shellHook = commonShellHook + pgShellHook;
                buildInputs = baseBuildInputs ++ [
                  stable_toolchain
                ];
                inherit nativeBuildInputs;

              }
              // envVars
            );

            regtest = pkgs.mkShell (
              {
                shellHook = commonShellHook + pgShellHook;
                buildInputs = baseBuildInputs ++ regtestBuildInputs ++ [
                  stable_toolchain
                ] ++ builtins.attrValues itestBinaries;
                inherit nativeBuildInputs;
              }
              // envVars
            );

            nightly = pkgs.mkShell (
              {
                shellHook = commonShellHook + pgShellHook;
                buildInputs = baseBuildInputs ++ [
                  nightly_toolchain
                ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            nightly-regtest = pkgs.mkShell (
              {
                shellHook = commonShellHook + pgShellHook;
                buildInputs = baseBuildInputs ++ regtestBuildInputs ++ [
                  nightly_toolchain
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
                '' + commonShellHook + pgShellHook;
                buildInputs = baseBuildInputs ++ regtestBuildInputs ++ [
                  stable_toolchain
                  pkgs.docker-client
                  pkgs.python311
                ] ++ builtins.attrValues itestBinaries;
                inherit nativeBuildInputs;
              }
              // envVars
            );

            # Shell for FFI development (Python bindings)
            ffi = pkgs.mkShell (
              {
                shellHook = commonShellHook + pgShellHook + ''
                  echo "FFI development shell"
                  echo "  just ffi-test        - Run Python FFI tests"
                  echo "  just ffi-dev-python  - Launch Python REPL with CDK FFI"
                '';
                buildInputs = baseBuildInputs ++ [
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
              regtest
              nightly
              nightly-regtest
              integration
              ffi
              ;
            default = stable;
          };
      }
    );
}
