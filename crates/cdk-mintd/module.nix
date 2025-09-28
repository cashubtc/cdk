{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.cdk-mintd;

  # Configuration file generation
  configFormat = pkgs.formats.toml { };

  configFile = configFormat.generate "config.toml" {
    info = {
      inherit (cfg.info) url listen_host listen_port;
      mnemonic = mkIf (cfg.info.mnemonic != "") cfg.info.mnemonic;
      input_fee_ppk = mkIf (cfg.info.inputFeePpk != null) cfg.info.inputFeePpk;
      enable_swagger_ui = mkIf (cfg.info.enableSwaggerUi != null) cfg.info.enableSwaggerUi;

      logging = {
        output = cfg.info.logging.output;
        console_level = mkIf (cfg.info.logging.consoleLevel != null) cfg.info.logging.consoleLevel;
        file_level = mkIf (cfg.info.logging.fileLevel != null) cfg.info.logging.fileLevel;
      };

      http_cache = {
        backend = cfg.info.httpCache.backend;
        ttl = cfg.info.httpCache.ttl;
        tti = cfg.info.httpCache.tti;
        key_prefix = mkIf (cfg.info.httpCache.keyPrefix != null) cfg.info.httpCache.keyPrefix;
        connection_string = mkIf (cfg.info.httpCache.connectionString != null) cfg.info.httpCache.connectionString;
      };
    };

    mint_management_rpc = mkIf cfg.mintManagementRpc.enabled {
      enabled = cfg.mintManagementRpc.enabled;
      address = cfg.mintManagementRpc.address;
      port = cfg.mintManagementRpc.port;
    };

    mint_info = mkIf cfg.mintInfo.enable (lib.filterAttrs (n: v: v != null) {
      name = cfg.mintInfo.name;
      pubkey = cfg.mintInfo.pubkey;
      description = cfg.mintInfo.description;
      description_long = cfg.mintInfo.descriptionLong;
      motd = cfg.mintInfo.motd;
      icon_url = cfg.mintInfo.iconUrl;
      contact_email = cfg.mintInfo.contactEmail;
      contact_nostr_public_key = cfg.mintInfo.contactNostrPublicKey;
      tos_url = cfg.mintInfo.tosUrl;
    });

    database = {
      engine = cfg.database.engine;
      postgres = mkIf (cfg.database.engine == "postgres") {
        url = mkIf (cfg.database.postgres.url != "") cfg.database.postgres.url;
        tls_mode = cfg.database.postgres.tlsMode;
        max_connections = cfg.database.postgres.maxConnections;
        connection_timeout_seconds = cfg.database.postgres.connectionTimeoutSeconds;
      };
    };

    ln = {
      ln_backend = cfg.lightning.backend;
      min_mint = mkIf (cfg.lightning.minMint != null) cfg.lightning.minMint;
      max_mint = mkIf (cfg.lightning.maxMint != null) cfg.lightning.maxMint;
      min_melt = mkIf (cfg.lightning.minMelt != null) cfg.lightning.minMelt;
      max_melt = mkIf (cfg.lightning.maxMelt != null) cfg.lightning.maxMelt;
    };

    # Lightning backend specific configurations
    cln = mkIf (cfg.lightning.backend == "cln") cfg.lightning.cln;
    lnd = mkIf (cfg.lightning.backend == "lnd") cfg.lightning.lnd;
    lnbits = mkIf (cfg.lightning.backend == "lnbits") {
      admin_api_key = mkIf (cfg.lightning.lnbits.admin_api_key != "") cfg.lightning.lnbits.admin_api_key;
      invoice_api_key = mkIf (cfg.lightning.lnbits.invoice_api_key != "") cfg.lightning.lnbits.invoice_api_key;
      lnbits_api = cfg.lightning.lnbits.lnbits_api;
    };
    ldk_node = mkIf (cfg.lightning.backend == "ldknode") {
      fee_percent = cfg.lightning.ldkNode.fee_percent;
      reserve_fee_min = cfg.lightning.ldkNode.reserve_fee_min;
      bitcoin_network = cfg.lightning.ldkNode.bitcoin_network;
      chain_source_type = cfg.lightning.ldkNode.chain_source_type;
      esplora_url = cfg.lightning.ldkNode.esplora_url;
      gossip_source_type = cfg.lightning.ldkNode.gossip_source_type;
      rgs_url = cfg.lightning.ldkNode.rgs_url;
      storage_dir_path = cfg.lightning.ldkNode.storage_dir_path;
      bitcoind_rpc_host = cfg.lightning.ldkNode.bitcoind_rpc_host;
      bitcoind_rpc_port = cfg.lightning.ldkNode.bitcoind_rpc_port;
      bitcoind_rpc_user = mkIf (cfg.lightning.ldkNode.bitcoind_rpc_user != "") cfg.lightning.ldkNode.bitcoind_rpc_user;
      bitcoind_rpc_password = mkIf (cfg.lightning.ldkNode.bitcoind_rpc_password != "") cfg.lightning.ldkNode.bitcoind_rpc_password;
      ldk_node_host = cfg.lightning.ldkNode.ldk_node_host;
      ldk_node_port = cfg.lightning.ldkNode.ldk_node_port;
      webserver_host = cfg.lightning.ldkNode.webserver_host;
      webserver_port = cfg.lightning.ldkNode.webserver_port;
    };
    fake_wallet = mkIf (cfg.lightning.backend == "fakewallet") cfg.lightning.fakeWallet;
    grpc_processor = mkIf (cfg.lightning.backend == "grpc_processor") cfg.lightning.grpcProcessor;

    # Authentication configuration
    auth = mkIf cfg.auth.enable {
      auth_enabled = cfg.auth.enable;
      openid_discovery = cfg.auth.openidDiscovery;
      openid_client_id = cfg.auth.openidClientId;
      mint_max_bat = cfg.auth.mintMaxBat;

      # Endpoint authentication settings
      mint = cfg.auth.endpoints.mint;
      get_mint_quote = cfg.auth.endpoints.getMintQuote;
      check_mint_quote = cfg.auth.endpoints.checkMintQuote;
      melt = cfg.auth.endpoints.melt;
      get_melt_quote = cfg.auth.endpoints.getMeltQuote;
      check_melt_quote = cfg.auth.endpoints.checkMeltQuote;
      swap = cfg.auth.endpoints.swap;
      restore = cfg.auth.endpoints.restore;
      check_proof_state = cfg.auth.endpoints.checkProofState;
    };
  };

in {
  options.services.cdk-mintd = {
    enable = mkEnableOption "cdk-mintd Cashu mint daemon";

    package = mkOption {
      type = types.package;
      default = pkgs.cdk-mintd or (throw "cdk-mintd package not available, please add it to nixpkgs or override this option");
      description = "The cdk-mintd package to use";
    };

    user = mkOption {
      type = types.str;
      default = "cdk-mintd";
      description = "User account under which cdk-mintd runs";
    };

    group = mkOption {
      type = types.str;
      default = "cdk-mintd";
      description = "Group under which cdk-mintd runs";
    };

    stateDir = mkOption {
      type = types.str;
      default = "cdk-mintd";
      description = "State directory for cdk-mintd data";
    };

    configFile = mkOption {
      type = types.path;
      default = configFile;
      defaultText = literalExpression "generated config file";
      description = "Path to the cdk-mintd configuration file";
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Path to environment file containing sensitive configuration like API keys, mnemonics, database passwords.
        The file should contain key=value pairs, one per line.
        Example variables: CDK_MINTD_POSTGRES_URL, CDK_MINTD_MNEMONIC, etc.

        Note: This is an alternative to using individual secret paths. If you use sops-nix,
        consider using the secrets.* options instead for better integration.
      '';
    };

    secrets = {
      mnemonic = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to file containing the seed mnemonic for the mint.
          When using sops-nix: config.sops.secrets."cdk-mintd/mnemonic".path
          The file should contain only the mnemonic phrase.
        '';
      };

      postgresUrl = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to file containing the PostgreSQL connection URL.
          When using sops-nix: config.sops.secrets."cdk-mintd/postgres-url".path
          The file should contain only the connection URL.
        '';
      };

      lnbitsAdminKey = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to file containing the LNBits admin API key.
          When using sops-nix: config.sops.secrets."cdk-mintd/lnbits-admin-key".path
          Only needed when using LNBits backend.
        '';
      };

      lnbitsInvoiceKey = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to file containing the LNBits invoice API key.
          When using sops-nix: config.sops.secrets."cdk-mintd/lnbits-invoice-key".path
          Only needed when using LNBits backend.
        '';
      };

      bitcoindRpcUser = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to file containing the Bitcoin RPC username.
          When using sops-nix: config.sops.secrets."cdk-mintd/bitcoind-rpc-user".path
          Only needed when using LDK Node with Bitcoin RPC.
        '';
      };

      bitcoindRpcPassword = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Path to file containing the Bitcoin RPC password.
          When using sops-nix: config.sops.secrets."cdk-mintd/bitcoind-rpc-password".path
          Only needed when using LDK Node with Bitcoin RPC.
        '';
      };
    };

    info = {
      url = mkOption {
        type = types.str;
        example = "https://mint.example.com/";
        description = "Public URL of the mint";
      };

      listen_host = mkOption {
        type = types.str;
        default = "127.0.0.1";
        description = "Host address to listen on";
      };

      listen_port = mkOption {
        type = types.port;
        default = 8085;
        description = "Port to listen on";
      };

      mnemonic = mkOption {
        type = types.str;
        default = "";
        description = ''
          Seed mnemonic for the mint. SECURITY WARNING: Do not set this option directly.
          Instead, use environmentFile and set CDK_MINTD_MNEMONIC environment variable.
          This option is only provided for initial testing and should remain empty in production.
        '';
      };

      inputFeePpk = mkOption {
        type = types.nullOr types.ints.unsigned;
        default = null;
        description = "Input fee in parts per thousand";
      };

      enableSwaggerUi = mkOption {
        type = types.nullOr types.bool;
        default = null;
        description = "Enable Swagger UI for API documentation";
      };

      logging = {
        output = mkOption {
          type = types.enum [ "stdout" "file" "both" ];
          default = "both";
          description = "Where to output logs: stdout (stderr), file, or both";
        };

        consoleLevel = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "info";
          description = "Log level for console output";
        };

        fileLevel = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "debug";
          description = "Log level for file output";
        };
      };

      httpCache = {
        backend = mkOption {
          type = types.enum [ "memory" "redis" ];
          default = "memory";
          description = "HTTP cache backend to use";
        };

        ttl = mkOption {
          type = types.ints.unsigned;
          default = 60;
          description = "Time to live for cache entries in seconds";
        };

        tti = mkOption {
          type = types.ints.unsigned;
          default = 60;
          description = "Time to idle for cache entries in seconds";
        };

        keyPrefix = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "mintd";
          description = "Key prefix for Redis cache (required for Redis backend)";
        };

        connectionString = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "redis://localhost";
          description = "Redis connection string (required for Redis backend)";
        };
      };
    };

    mintManagementRpc = {
      enabled = mkOption {
        type = types.bool;
        default = false;
        description = "Enable mint management RPC interface";
      };

      address = mkOption {
        type = types.str;
        default = "127.0.0.1";
        description = "Address for mint management RPC";
      };

      port = mkOption {
        type = types.port;
        default = 8086;
        description = "Port for mint management RPC";
      };
    };

    mintInfo = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to configure mint information metadata";
      };

      name = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "My Cashu Mint";
        description = "Name of the mint";
      };

      pubkey = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Hex pubkey of mint";
      };

      description = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Short description of the mint";
      };

      descriptionLong = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Long description of the mint";
      };

      motd = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Message of the day";
      };

      iconUrl = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "URL to mint icon";
      };

      contactEmail = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Contact email for the mint";
      };

      contactNostrPublicKey = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Nostr pubkey of mint (Hex)";
      };

      tosUrl = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "URL to terms of service";
      };
    };

    database = {
      engine = mkOption {
        type = types.enum [ "sqlite" "postgres" ];
        default = "sqlite";
        description = "Database engine to use";
      };

      postgres = {
        url = mkOption {
          type = types.str;
          default = "";
          description = ''
            PostgreSQL connection URL. SECURITY WARNING: Do not set this option directly.
            Instead, use environmentFile and set CDK_MINTD_POSTGRES_URL environment variable.
            This option is only provided for initial testing and should remain empty in production.
          '';
        };

        tlsMode = mkOption {
          type = types.enum [ "disable" "prefer" "require" ];
          default = "disable";
          description = "PostgreSQL TLS mode";
        };

        maxConnections = mkOption {
          type = types.ints.unsigned;
          default = 20;
          description = "Maximum number of connections in the pool";
        };

        connectionTimeoutSeconds = mkOption {
          type = types.ints.unsigned;
          default = 10;
          description = "Connection timeout in seconds";
        };
      };
    };

    lightning = {
      backend = mkOption {
        type = types.enum [ "cln" "lnd" "fakewallet" "lnbits" "ldknode" "grpc_processor" ];
        description = "Lightning Network backend to use";
      };

      minMint = mkOption {
        type = types.nullOr types.ints.unsigned;
        default = null;
        description = "Minimum mint amount in satoshis";
      };

      maxMint = mkOption {
        type = types.nullOr types.ints.unsigned;
        default = null;
        description = "Maximum mint amount in satoshis";
      };

      minMelt = mkOption {
        type = types.nullOr types.ints.unsigned;
        default = null;
        description = "Minimum melt amount in satoshis";
      };

      maxMelt = mkOption {
        type = types.nullOr types.ints.unsigned;
        default = null;
        description = "Maximum melt amount in satoshis";
      };

      # CLN backend configuration
      cln = mkOption {
        type = types.submodule {
          options = {
            rpc_path = mkOption {
              type = types.str;
              description = "Path to CLN RPC socket";
            };
            fee_percent = mkOption {
              type = types.float;
              default = 0.04;
              description = "Fee percentage for lightning operations";
            };
            reserve_fee_min = mkOption {
              type = types.ints.unsigned;
              default = 4;
              description = "Minimum reserve fee in satoshis";
            };
          };
        };
        default = {};
        description = "Core Lightning (CLN) backend configuration";
      };

      # LND backend configuration
      lnd = mkOption {
        type = types.submodule {
          options = {
            address = mkOption {
              type = types.str;
              description = "LND gRPC address";
            };
            macaroon_file = mkOption {
              type = types.str;
              description = "Path to LND macaroon file";
            };
            cert_file = mkOption {
              type = types.str;
              description = "Path to LND TLS certificate file";
            };
            fee_percent = mkOption {
              type = types.float;
              default = 0.04;
              description = "Fee percentage for lightning operations";
            };
            reserve_fee_min = mkOption {
              type = types.ints.unsigned;
              default = 4;
              description = "Minimum reserve fee in satoshis";
            };
          };
        };
        default = {};
        description = "LND backend configuration";
      };

      # LNBits backend configuration
      lnbits = mkOption {
        type = types.submodule {
          options = {
            admin_api_key = mkOption {
              type = types.str;
              description = "LNBits admin API key (provide via environmentFile)";
            };
            invoice_api_key = mkOption {
              type = types.str;
              description = "LNBits invoice API key (provide via environmentFile)";
            };
            lnbits_api = mkOption {
              type = types.str;
              description = "LNBits API URL";
            };
          };
        };
        default = {};
        description = "LNBits backend configuration";
      };

      # LDK Node backend configuration
      ldkNode = mkOption {
        type = types.submodule {
          options = {
            fee_percent = mkOption {
              type = types.float;
              default = 0.04;
              description = "Fee percentage for lightning operations";
            };
            reserve_fee_min = mkOption {
              type = types.ints.unsigned;
              default = 4;
              description = "Minimum reserve fee in satoshis";
            };
            bitcoin_network = mkOption {
              type = types.enum [ "mainnet" "testnet" "signet" "regtest" ];
              default = "signet";
              description = "Bitcoin network to use";
            };
            chain_source_type = mkOption {
              type = types.enum [ "esplora" "bitcoinrpc" ];
              default = "esplora";
              description = "Chain source type";
            };
            esplora_url = mkOption {
              type = types.str;
              default = "https://mutinynet.com/api";
              description = "Esplora API URL";
            };
            gossip_source_type = mkOption {
              type = types.enum [ "p2p" "rgs" ];
              default = "rgs";
              description = "Gossip source type";
            };
            rgs_url = mkOption {
              type = types.str;
              default = "https://rgs.mutinynet.com/snapshot/0";
              description = "RGS URL for gossip sync";
            };
            storage_dir_path = mkOption {
              type = types.str;
              default = "~/.cdk-ldk-node";
              description = "Storage directory path";
            };
            bitcoind_rpc_host = mkOption {
              type = types.str;
              default = "127.0.0.1";
              description = "Bitcoin RPC host (when using bitcoinrpc)";
            };
            bitcoind_rpc_port = mkOption {
              type = types.port;
              default = 18443;
              description = "Bitcoin RPC port (when using bitcoinrpc)";
            };
            bitcoind_rpc_user = mkOption {
              type = types.str;
              default = "";
              description = "Bitcoin RPC username (provide via environmentFile)";
            };
            bitcoind_rpc_password = mkOption {
              type = types.str;
              default = "";
              description = "Bitcoin RPC password (provide via environmentFile)";
            };
            ldk_node_host = mkOption {
              type = types.str;
              default = "127.0.0.1";
              description = "LDK node host";
            };
            ldk_node_port = mkOption {
              type = types.port;
              default = 8090;
              description = "LDK node port";
            };
            webserver_host = mkOption {
              type = types.str;
              default = "127.0.0.1";
              description = "Webserver host for LDK node management";
            };
            webserver_port = mkOption {
              type = types.port;
              default = 0;
              description = "Webserver port (0 = auto-assign)";
            };
          };
        };
        default = {};
        description = "LDK Node backend configuration";
      };

      # Fake wallet backend configuration
      fakeWallet = mkOption {
        type = types.submodule {
          options = {
            supported_units = mkOption {
              type = types.listOf types.str;
              default = [ "sat" ];
              description = "Supported currency units";
            };
            fee_percent = mkOption {
              type = types.float;
              default = 0.02;
              description = "Fee percentage";
            };
            reserve_fee_min = mkOption {
              type = types.ints.unsigned;
              default = 1;
              description = "Minimum reserve fee";
            };
            min_delay_time = mkOption {
              type = types.ints.unsigned;
              default = 1;
              description = "Minimum delay time in seconds";
            };
            max_delay_time = mkOption {
              type = types.ints.unsigned;
              default = 3;
              description = "Maximum delay time in seconds";
            };
          };
        };
        default = {};
        description = "Fake wallet backend configuration (for testing)";
      };

      # gRPC processor backend configuration
      grpcProcessor = mkOption {
        type = types.submodule {
          options = {
            supported_units = mkOption {
              type = types.listOf types.str;
              default = [ "sat" ];
              description = "Supported currency units";
            };
            addr = mkOption {
              type = types.str;
              default = "127.0.0.1";
              description = "gRPC processor address";
            };
            port = mkOption {
              type = types.port;
              default = 50051;
              description = "gRPC processor port";
            };
            tls_dir = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Path to TLS directory";
            };
          };
        };
        default = {};
        description = "gRPC Payment Processor configuration";
      };
    };

    auth = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = "Enable authentication features";
      };

      openidDiscovery = mkOption {
        type = types.str;
        default = "";
        description = "OpenID Connect discovery URL";
      };

      openidClientId = mkOption {
        type = types.str;
        default = "";
        description = "OpenID Connect client ID";
      };

      mintMaxBat = mkOption {
        type = types.ints.unsigned;
        default = 50;
        description = "Maximum mint BAT";
      };

      endpoints = {
        mint = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "blind";
          description = "Authentication type for mint endpoint";
        };

        getMintQuote = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "none";
          description = "Authentication type for get mint quote endpoint";
        };

        checkMintQuote = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "none";
          description = "Authentication type for check mint quote endpoint";
        };

        melt = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "none";
          description = "Authentication type for melt endpoint";
        };

        getMeltQuote = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "none";
          description = "Authentication type for get melt quote endpoint";
        };

        checkMeltQuote = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "none";
          description = "Authentication type for check melt quote endpoint";
        };

        swap = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "blind";
          description = "Authentication type for swap endpoint";
        };

        restore = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "blind";
          description = "Authentication type for restore endpoint";
        };

        checkProofState = mkOption {
          type = types.enum [ "clear" "blind" "none" ];
          default = "none";
          description = "Authentication type for check proof state endpoint";
        };
      };
    };
  };

  config = mkIf cfg.enable {
    users.users = mkIf (cfg.user == "cdk-mintd") {
      cdk-mintd = {
        isSystemUser = true;
        group = cfg.group;
        description = "cdk-mintd Cashu mint daemon user";
        home = "/var/lib/${cfg.stateDir}";
        createHome = true;
      };
    };

    users.groups = mkIf (cfg.group == "cdk-mintd") {
      cdk-mintd = {};
    };

    systemd.services.cdk-mintd = {
      description = "cdk-mintd Cashu mint daemon";
      after = [ "network.target" ]
        ++ optional (cfg.database.engine == "postgres") "postgresql.service";
      wants = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        Restart = "always";
        RestartSec = "10s";

        # Security settings
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectHostname = true;
        ProtectClock = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        RemoveIPC = true;
        PrivateMounts = true;

        # Working directory and state
        WorkingDirectory = "/var/lib/${cfg.stateDir}";
        StateDirectory = cfg.stateDir;
        StateDirectoryMode = "0750";

        # Logging
        StandardOutput = "journal";
        StandardError = "journal";
        SyslogIdentifier = "cdk-mintd";

        # Environment
        EnvironmentFile = mkIf (cfg.environmentFile != null) cfg.environmentFile;

        # Load secrets as credentials (available as environment variables)
        LoadCredential = let
          loadCredentials = lib.optionals (cfg.secrets.mnemonic != null) [ "CDK_MINTD_MNEMONIC:${cfg.secrets.mnemonic}" ]
            ++ lib.optionals (cfg.secrets.postgresUrl != null) [ "CDK_MINTD_POSTGRES_URL:${cfg.secrets.postgresUrl}" ]
            ++ lib.optionals (cfg.secrets.lnbitsAdminKey != null) [ "CDK_MINTD_LNBITS_ADMIN_API_KEY:${cfg.secrets.lnbitsAdminKey}" ]
            ++ lib.optionals (cfg.secrets.lnbitsInvoiceKey != null) [ "CDK_MINTD_LNBITS_INVOICE_API_KEY:${cfg.secrets.lnbitsInvoiceKey}" ]
            ++ lib.optionals (cfg.secrets.bitcoindRpcUser != null) [ "CDK_MINTD_BITCOIND_RPC_USER:${cfg.secrets.bitcoindRpcUser}" ]
            ++ lib.optionals (cfg.secrets.bitcoindRpcPassword != null) [ "CDK_MINTD_BITCOIND_RPC_PASSWORD:${cfg.secrets.bitcoindRpcPassword}" ];
        in mkIf (loadCredentials != []) loadCredentials;

        # Process execution
        ExecStart = "${cfg.package}/bin/cdk-mintd --config ${cfg.configFile}";

        # Capability restrictions
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
      };
    };

    # Open firewall port if listening on all interfaces
    networking.firewall.allowedTCPPorts = mkIf (cfg.info.listen_host == "0.0.0.0") [ cfg.info.listen_port ];

    # Validation
    assertions = [
      {
        assertion = cfg.info.url != "";
        message = "services.cdk-mintd.info.url must be set";
      }
      {
        assertion = cfg.lightning.backend != null;
        message = "services.cdk-mintd.lightning.backend must be set";
      }
      {
        assertion = (cfg.lightning.backend == "cln") -> (cfg.lightning.cln.rpc_path != "");
        message = "services.cdk-mintd.lightning.cln.rpc_path must be set when using CLN backend";
      }
      {
        assertion = (cfg.lightning.backend == "lnd") -> (cfg.lightning.lnd.address != "" && cfg.lightning.lnd.macaroon_file != "" && cfg.lightning.lnd.cert_file != "");
        message = "services.cdk-mintd.lightning.lnd.{address,macaroon_file,cert_file} must be set when using LND backend";
      }
      {
        assertion = (cfg.lightning.backend == "lnbits") -> (cfg.lightning.lnbits.lnbits_api != "" && (cfg.environmentFile != null || (cfg.secrets.lnbitsAdminKey != null && cfg.secrets.lnbitsInvoiceKey != null)));
        message = "services.cdk-mintd.lightning.lnbits.lnbits_api must be set and either environmentFile or secrets.{lnbitsAdminKey,lnbitsInvoiceKey} must be provided when using LNBits backend";
      }
      {
        assertion = (cfg.database.engine == "postgres") -> (cfg.database.postgres.url != "" || cfg.environmentFile != null || cfg.secrets.postgresUrl != null);
        message = "services.cdk-mintd.database.postgres.url must be set, or environmentFile provided, or secrets.postgresUrl configured when using PostgreSQL";
      }
      {
        assertion = (cfg.info.httpCache.backend == "redis") -> (cfg.info.httpCache.keyPrefix != null && cfg.info.httpCache.connectionString != null);
        message = "services.cdk-mintd.info.httpCache.{keyPrefix,connectionString} must be set when using Redis cache backend";
      }
      {
        assertion = cfg.auth.enable -> (cfg.auth.openidDiscovery != "" && cfg.auth.openidClientId != "");
        message = "services.cdk-mintd.auth.{openidDiscovery,openidClientId} must be set when authentication is enabled";
      }
    ];

    warnings = [
      (mkIf (cfg.info.mnemonic != "")
        "cdk-mintd: SECURITY WARNING: mnemonic is set in configuration and will be stored in the Nix store. Use environmentFile or secrets.mnemonic instead.")
      (mkIf (cfg.database.engine == "postgres" && cfg.database.postgres.url != "")
        "cdk-mintd: SECURITY WARNING: PostgreSQL URL is set in configuration and will be stored in the Nix store. Use environmentFile or secrets.postgresUrl instead.")
      (mkIf (cfg.lightning.backend == "lnbits" && (cfg.lightning.lnbits.admin_api_key != "" || cfg.lightning.lnbits.invoice_api_key != ""))
        "cdk-mintd: SECURITY WARNING: LNBits API keys are set in configuration and will be stored in the Nix store. Use environmentFile or secrets.{lnbitsAdminKey,lnbitsInvoiceKey} instead.")
    ];
  };
}
