{
  description = "CDK Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

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

    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
  };

  outputs =
    { self
    , nixpkgs
    , rust-overlay
    , flake-utils
    , pre-commit-hooks
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
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
          ];

        # Dependencies
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Toolchains
        # latest stable
        stable_toolchain = pkgs.rust-bin.stable."1.91.1".default.override {
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

        # Common inputs
        envVars = {
          # rust analyzer needs  NIX_PATH for some reason.
          NIX_PATH = "nixpkgs=${inputs.nixpkgs}";
        };
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
            clightning
            bitcoind
            sqlx-cli
            mprocs

            cargo-outdated
            cargo-mutants

            # Needed for github ci
            libz
            openssl
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
                pathsToLink = [
                  "/"
                  "/bin"
                ];
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
            msrv = pkgs.mkShell (
              {
                shellHook = "
                  cargo update
                  cargo update home --precise 0.5.11
                  cargo update typed-index-collections --precise 3.3.0
              ${_shellHook}
              ";
                buildInputs = buildInputs ++ [ msrv_toolchain ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            stable = pkgs.mkShell (
              {
                shellHook = ''
                  ${_shellHook}
                  # Needed for github ci
                  export LD_LIBRARY_PATH=${
                    pkgs.lib.makeLibraryPath [
                      pkgs.zlib
                      pkgs.openssl
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
                  ${_shellHook}
                  # Needed for github ci
                  export LD_LIBRARY_PATH=${
                    pkgs.lib.makeLibraryPath [
                      pkgs.zlib
                    ]
                  }:$LD_LIBRARY_PATH
                '';
                buildInputs = buildInputs ++ [ nightly_toolchain ];
                inherit nativeBuildInputs;
              }
              // envVars
            );

            # Shell with Docker for integration tests
            integration = pkgs.mkShell (
              {
                shellHook = ''
                  ${_shellHook}
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

          in
          {
            inherit
              msrv
              stable
              nightly
              integration
              ;
            default = stable;
          };
      }
    );
}
