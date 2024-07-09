{
  description = "Cashu Development Kit";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";

    flakebox = {
      url = "github:rustshop/flakebox";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flakebox, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { system = system; };
        lib = pkgs.lib;
        flakeboxLib = flakebox.lib.${system} { };
        rustSrc = flakeboxLib.filterSubPaths {
          root = builtins.path {
            name = "cdk";
            path = ./.;
          };
          paths = [ "crates/cashu" "crates/cashu-sdk" ];
        };

        targetsStd = flakeboxLib.mkStdTargets { };
        toolchainsStd = flakeboxLib.mkStdToolchains { };

        toolchainNative = flakeboxLib.mkFenixToolchain {
          targets = (pkgs.lib.getAttrs [ "default" "wasm32-unknown" ] targetsStd);
        };

        commonArgs = {
          buildInputs = [ pkgs.openssl ] ++ lib.optionals pkgs.stdenv.isDarwin
            [ pkgs.darwin.apple_sdk.frameworks.SystemConfiguration ];
          nativeBuildInputs = [ pkgs.pkg-config ];
        };
        outputs = (flakeboxLib.craneMultiBuild { toolchains = toolchainsStd; })
          (craneLib':
            let
              craneLib = (craneLib'.overrideArgs {
                pname = "flexbox-multibuild";
                src = rustSrc;
              }).overrideArgs commonArgs;
            in rec {
              workspaceDeps = craneLib.buildWorkspaceDepsOnly { };
              workspaceBuild =
                craneLib.buildWorkspace { cargoArtifacts = workspaceDeps; };
            });
      in {
        devShells = flakeboxLib.mkShells {
          toolchain = toolchainNative;
          packages = [ ];
          nativeBuildInputs = with pkgs; [ wasm-pack sqlx-cli ];
        };
      });
}
