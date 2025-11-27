{ lib
, rustPlatform
, pkg-config
, openssl
, sqlite
, postgresql
, protobuf
}:

rustPlatform.buildRustPackage rec {
  pname = "cdk-mintd";
  version = "0.12.0";

  src = ../../.; # Points to workspace root since cdk-mintd is a workspace member

  cargoLock = {
    lockFile = ../../Cargo.lock;
    outputHashes = {
      "ln-regtest-rs-0.1.0" = "sha256-d5z81KeQiVJXP8Rg29aGprdJ7TGoT82SpkEBZdznG1U=";
    };
  };

  cargoBuildFlags = [ "--package" "cdk-mintd" ];
  cargoTestFlags = [ "--package" "cdk-mintd" ];

  nativeBuildInputs = [
    pkg-config
    protobuf
  ];

  buildInputs = [
    openssl
    sqlite
    postgresql
  ];

  meta = with lib; {
    description = "Cashu mint daemon - CDK implementation";
    homepage = "https://github.com/cashubtc/cdk";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "cdk-mintd";
  };
}
