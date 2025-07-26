# CDK LDK Node

CDK lightning backend for ldk-node, providing Lightning Network functionality for CDK with support for Cashu operations.

## Features

- Lightning Network payments (Bolt11 and Bolt12)
- Channel management
- Payment processing for Cashu mint operations
- Web management interface

## Web Management Interface

The CDK LDK Node includes a built-in web management interface that provides:

- **Dashboard**: Overview of node status, balance, and recent activity
- **Balance & On-chain**: View balances and manage on-chain transactions
- **Channel Management**: Open and close Lightning channels
- **Invoice Creation**: Create Bolt11 and Bolt12 invoices
- **Payment Sending**: Send Lightning payments

### Configuration

The web server can be configured through the configuration file or environment variables:

#### Config file (TOML):
```toml
[ldk_node]
# Web management interface configuration
webserver_host = "127.0.0.1"  # Default: 127.0.0.1
webserver_port = 8091
```

#### Environment variables:
- `CDK_MINTD_LDK_NODE_WEBSERVER_HOST`: Host address for the web interface
- `CDK_MINTD_LDK_NODE_WEBSERVER_PORT`: Port for the web interface

### Defaults

- **Host**: `127.0.0.1` (localhost)
- **Port**: `0` (automatically assigns an unused port)

When the port is set to `0`, the system will automatically find and assign an available port, which is logged when the server starts.

### Accessing the Interface

Once cdk-mintd is running with the LDK node backend, the web interface will be available at:
```
http://<webserver_host>:<assigned_port>
```

The actual port used will be displayed in the logs when the service starts.

## Configuration Example

```toml
[ln]
ln_backend = "ldk-node"

[ldk_node]
fee_percent = 0.04
reserve_fee_min = 4
bitcoin_network = "regtest"
chain_source_type = "esplora"
esplora_url = "https://mutinynet.com/api"
storage_dir_path = "~/.cdk-ldk-node/ldk-node"
ldk_node_host = "127.0.0.1"
ldk_node_port = 8090
gossip_source_type = "p2p"

# Web management interface
webserver_host = "127.0.0.1"
webserver_port = 8091
```

## Environment Variables

All configuration options can be set via environment variables with the `CDK_MINTD_LDK_NODE_` prefix:

- `CDK_MINTD_LDK_NODE_FEE_PERCENT`
- `CDK_MINTD_LDK_NODE_RESERVE_FEE_MIN`
- `CDK_MINTD_LDK_NODE_BITCOIN_NETWORK`
- `CDK_MINTD_LDK_NODE_WEBSERVER_HOST`
- `CDK_MINTD_LDK_NODE_WEBSERVER_PORT`
- ... (and other LDK node settings)
