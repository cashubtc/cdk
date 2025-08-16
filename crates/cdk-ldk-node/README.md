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
- **Payment History**: Paginated view of all Lightning and on-chain payments

### Payment History Pagination

The payment history page includes comprehensive pagination support to efficiently handle large numbers of payments:

#### Features:
- **Bottom-only pagination**: Clean interface with pagination controls only at the bottom after viewing payments
- **Page-based navigation**: Navigate through payments using Previous/Next buttons and page numbers
- **Customizable page size**: Choose between 10, 25, 50, or 100 payments per page (selector at bottom)
- **Smart pagination**: Shows ellipsis (...) for large page ranges with quick access to first/last pages
- **Filtering**: Filter payments by direction (All, Incoming, Outgoing) while maintaining pagination state
- **Payment counter**: Shows current range (e.g., "Showing 1 to 25 of 147 payments")
- **Responsive design**: Optimized for both desktop and mobile devices

#### URL Parameters:
- `page`: Current page number (default: 1)
- `per_page`: Number of payments per page (default: 25, range: 10-100)
- `filter`: Payment direction filter ("all", "incoming", "outgoing")

#### Example URLs:
- `/payments` - First page with default settings
- `/payments?page=3&per_page=50&filter=outgoing` - Page 3, 50 per page, outgoing only
- `/payments?filter=incoming` - Incoming payments only, first page

#### Performance:
While the current implementation loads all payments and applies pagination in-memory, the pagination structure is designed to be easily upgraded to use database-level pagination for better performance with very large payment histories.

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
```sh
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
