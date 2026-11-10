# Release tools are selected by the monorepo's flake.lock. Native project
# outputs are deliberately built in fresh directories, outside the Nix cache.
{ pkgs, lib, cargoVendorDir, uniffiBindgenGo, nixpkgs, system, dartSdk }:
let
  inputs = builtins.fromJSON (builtins.readFile ../bindings/release/inputs.json);
  releaseVendor = pkgs.runCommand "cdk-release-vendor" { nativeBuildInputs = [ pkgs.python3 ]; } ''
    python3 ${../scripts/bindings/vendor.py} ${cargoVendorDir} "$out"
  '';
  rustVersion = (builtins.fromTOML (builtins.readFile ../rust-toolchain.toml)).toolchain.channel;
  linux = pkgs.stdenv.hostPlatform.isLinux;
  x86 = pkgs.stdenv.hostPlatform.isx86_64;
  rust = pkgs.rust-bin.stable.${rustVersion}.minimal.override {
    targets =
      if linux then
        lib.optionals x86 [
          "aarch64-linux-android"
          "armv7-linux-androideabi"
          "i686-linux-android"
          "x86_64-linux-android"
          "x86_64-pc-windows-msvc"
        ]
      else [
        "aarch64-apple-darwin"
        "x86_64-apple-darwin"
        "aarch64-apple-ios"
        "aarch64-apple-ios-sim"
        "x86_64-apple-ios"
      ];
  };
  tools = [ pkgs.python3 pkgs.git pkgs.jujutsu pkgs.nix pkgs.gh pkgs.gnupg ];
  common = tools ++ lib.optionals linux [ pkgs.docker-client pkgs.podman ] ++ [
    rust
    uniffiBindgenGo
    dartSdk
    pkgs.pkg-config
    pkgs.cmake
    pkgs.ninja
    pkgs.perl
    pkgs.go
    pkgs.jdk17
    pkgs.gnupg
    pkgs.curl
  ];
  androidPkgs = import nixpkgs {
    inherit system;
    config = { android_sdk.accept_license = true; allowUnfree = true; };
  };
  android = androidPkgs.androidenv.composeAndroidPackages {
    platformVersions = [ inputs.android.sdk ];
    buildToolsVersions = [ inputs.android.build_tools ];
    includeNDK = true;
    ndkVersions = [ inputs.android.ndk ];
    includeEmulator = false;
    includeSystemImages = false;
  };
  gradleArchive = pkgs.fetchurl {
    url = "https://downloads.gradle.org/distributions/gradle-${inputs.gradle.version}-bin.zip";
    sha256 = inputs.gradle.sha256;
  };
  gradle = pkgs.runCommand "cdk-release-gradle" { nativeBuildInputs = [ pkgs.unzip ]; } ''
    unzip -q ${gradleArchive}
    mv gradle-${inputs.gradle.version} $out
  '';
  windowsSdk = pkgs.stdenvNoCC.mkDerivation {
    name = "cdk-windows-sdk-${inputs.windows.sdk}-${inputs.windows.crt}";
    nativeBuildInputs = [ pkgs.cargo-xwin pkgs.cacert ];
    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = inputs.windows.sdk_hash;
    XWIN_VERSION = inputs.windows.visual_studio;
    XWIN_CRT_VERSION = inputs.windows.crt;
    XWIN_SDK_VERSION = inputs.windows.sdk;
    XWIN_ARCH = "x86_64";
    buildCommand = ''
      export XWIN_CACHE_DIR="$TMPDIR/sdk"
      cargo-xwin cache xwin
      mkdir -p $out
      cp -r "$XWIN_CACHE_DIR"/xwin/{crt,sdk,DONE} $out/
    '';
  };
  environment = {
    CDK_RELEASE_ENV = "1";
    CDK_RELEASE_RUST = rust;
    CDK_RELEASE_VENDOR = releaseVendor;
    JAVA_HOME = "${pkgs.jdk17}";
  };
in
{
  inherit windowsSdk;
  shells = {
    bindings-release-tools = pkgs.mkShell { packages = tools; };
    bindings-release-checks = pkgs.mkShell {
      packages = tools ++ [ pkgs.ruff pkgs.shellcheck pkgs.actionlint pkgs.nixpkgs-fmt ];
    };
    bindings-release = pkgs.mkShell (environment // {
      packages = common ++ lib.optionals linux [ pkgs.patchelf pkgs.binutils ];
    });
    bindings-release-android = pkgs.mkShell (environment // {
      packages = common ++ [ gradle ];
      CDK_RELEASE_GRADLE = "${gradle}/bin/gradle";
      ANDROID_HOME = "${android.androidsdk}/libexec/android-sdk";
      ANDROID_NDK_HOME = "${android.androidsdk}/libexec/android-sdk/ndk/${inputs.android.ndk}";
    });
    bindings-release-windows = pkgs.mkShell (environment // {
      packages = common ++ [
        pkgs.cargo-xwin
        pkgs.llvmPackages.clang-unwrapped
        pkgs.llvmPackages.llvm
        pkgs.llvmPackages.lld
        pkgs.nasm
      ];
      CDK_RELEASE_WINDOWS_SDK = windowsSdk;
    });
  };
}
