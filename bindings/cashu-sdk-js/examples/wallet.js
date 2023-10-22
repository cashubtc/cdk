const {Amount, loadWasmAsync, Wallet, Client } = require("..");


async  function main() {
  await loadWasmAsync();

  let client = new Client("https://mutinynet-cashu.thesimpekid.space");

  let keys = await client.getKeys();

  let wallet = new Wallet(client, keys);

  let amount = Amount.fromSat(BigInt(10));
  let pr = await wallet.requestMint(amount);

  console.log(pr);
}

main();
