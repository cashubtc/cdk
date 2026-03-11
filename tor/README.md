# Running CDK mint over tor

If you wish to run CDK mint over tor, follow this guide.

## TL;DR

1. navigate to the cdk project root directory
1. `docker compose -f docker-compose.tor.yaml`
1. open new terminal session in same directory
1. `sudo cat tor-data/hostname` will show the onion host name
1. make sure to add `http://` and NOT `https://` in front of the url when connecting to the mint

It can sometimes take some time for the tor service to be bootstrapped and be ready to receive connections. Even after complete bootstrapping, i noticed that sometimes the service is not immediately reachable. Give it a few minutes, and try 

## Using a tor mint

To connect to a mint via tor from a wallet, the wallet has to support tor. This means, for mobile wallets you either have to use something like orbot to proxy all traffic through tor, or the wallet has to natively support tor connections.

For browser wallets, you should be able to connect to the mint by opening the wallet in a tor browser. When using tor browsers, BE CAREFUL. The tor browser usually wipes all local storage after closing a session. THIS MEANS IT WILL DELETE THE WALLET AND ALL FUNDS!

Make sure to have your seed phrase secured & create a backup file EACH TIME before closing the session. Backup files have to be created after each session, or the wallet state will be invalid. There may be some plugins or configurations in certain tor browsers that let you keep storage after sessions end, but you have been warned. 

## Configuration

All of the configuration should happen automatically. If something is not working, maybe the port or host name of the cdk mint deployment was changed. In which case they have to be edited in `tor/torrc` to match the actual hostname and port where the cdk mint container is running.
 