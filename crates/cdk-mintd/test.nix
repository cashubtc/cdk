{ self
, pkgs
,
}:
let
  mockCdkMintd = pkgs.writeScriptBin "cdk-mintd" ''
    #!/bin/sh
    echo "Mock cdk-mintd starting with config: $@"

    # Simple HTTP server that responds to /info endpoint
    ${pkgs.python3}/bin/python3 -c '
    from http.server import HTTPServer, BaseHTTPRequestHandler
    import json

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/info" or self.path == "/v1/info":
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                response = {"name": "test-mint", "version": "0.0.1"}
                self.wfile.write(json.dumps(response).encode())
            else:
                self.send_response(404)
                self.end_headers()

        def log_message(self, format, *args):
            pass

    server = HTTPServer(("127.0.0.1", 8080), Handler)
    print("Mock cdk-mintd listening on 127.0.0.1:8080")
    server.serve_forever()
    '
  '';
  module = self.nixosModules.default;
in
pkgs.nixosTest {
  name = "cdk-mintd-basic";

  nodes.mint =
    { config
    , pkgs
    , ...
    }: {
      imports = [ module ];

      services.cdk-mintd = {
        enable = true;
        package = pkgs.callPackage ./package.nix { };

        settings = {
          info = {
            url = "http://localhost:8080/";
            listen_host = "127.0.0.1";
            listen_port = 8080;
            mnemonic = "test seed phrase for testing only not secure";
          };

          database = {
            engine = "sqlite";
          };

          lightning = {
            backend = "fakewallet";
            fakeWallet = {
              fee_percent = 0.02;
              reserve_fee_min = 1;
            };
          };
        };
      };
    };

  testScript = ''
    start_all()

    # Wait for the service to start
    mint.wait_for_unit("cdk-mintd.service")

    # Check that the service is active
    mint.succeed("systemctl is-active cdk-mintd.service")

    # Wait for the port to be open
    mint.wait_for_open_port(8080)

    # Check that the /info endpoint responds
    mint.succeed("curl -f http://127.0.0.1:8080/info")

    # Verify the config file was generated
    mint.succeed("test -f /var/lib/cdk-mintd/config.toml")

    # Check security hardening is applied
    mint.succeed("systemctl show cdk-mintd.service | grep NoNewPrivileges=yes")
    mint.succeed("systemctl show cdk-mintd.service | grep ProtectSystem=strict")

    # Check that the state directory exists with correct permissions
    mint.succeed("test -d /var/lib/cdk-mintd")
    mint.succeed("stat -c '%U:%G' /var/lib/cdk-mintd | grep cdk-mintd:cdk-mintd")

    print("✅ All basic tests passed!")
  '';
}
