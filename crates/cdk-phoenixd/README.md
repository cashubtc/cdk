# cdk-phoenixd

## Run phoenixd

The `phoenixd` node is included in the cdk and needs to be run separately.
Get started here: [Phoenixd Server Documentation](https://phoenix.acinq.co/server/get-started)

## Start Phoenixd

By default, `phoenixd` will run with auto-liquidity enabled. While this simplifies channel management, it makes fees non-deterministic, which is not recommended for most scenarios. However, it is necessary to start with auto-liquidity enabled in order to open a channel and get started.

Start the node with auto-liquidity enabled as documented by [Phoenixd](https://phoenix.acinq.co/server/get-started):
```sh
./phoenixd
```

> **Note:** By default the `auto-liquidity` will open a channel of 2m sats depending on the size of mint you plan to run you may want to increase this by setting the `--auto-liquidity` flag to `5m` or `10m`.

## Open Channel

Once the node is running, create an invoice using the phoenixd-cli to fund your node. A portion of this deposit will go to ACINQ as a fee for the provided liquidity, and a portion will cover the mining fee. These two fees cannot be refunded or withdrawn from the node. More on fees can be found [here](https://phoenix.acinq.co/server/auto-liquidity#fees). The remainder will stay as the node balance and can be withdrawn later.
```sh
./phoenix-cli createinvoice \
    --description "Fund Node" \
    --amountSat xxxxx
```

> **Note:** The amount above should be set depending on the size of the mint you would like to run as it will determine the size of the channel and amount of liquidity.

## Check Channel state

After paying the invoice view that a channel has been opened.
```sh
./phoenix-cli listchannels
```

## Restart Phoenixd without `auto-liquidity`

Now that the node has a channel, it is recommended to stop the node and restart it without auto-liquidity. This will prevent phoenixd from opening new channels and incurring additional fees.
```sh
./phoenixd --auto-liquidity off
```

## Start cashu-mintd

Once the node is running following the [cashu-mintd](../cdk-mintd/README.md) to start the mint. by default the `api_url` will be `http://127.0.0.1:9740` and the `api_password` can be found in `~/.phoenix/phoenix.conf` these will need to be set in the `cdk-mintd` config file.
