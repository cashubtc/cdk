const {Amount, loadWasmAsync, loadWasmSync } = require("..");


function main() {
  loadWasmSync();

  let amount = Amount.fromSat(BigInt(10));

  console.log(amount.toSat())
}

main();
