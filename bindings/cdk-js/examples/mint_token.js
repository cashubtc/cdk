const {
    loadWasmAsync,
    Wallet,
    CurrencyUnit
} = require("../");

async function main() {
    await loadWasmAsync();
    let seed = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let mint_url = "https://testnut.cashu.space";
    let currency = CurrencyUnit.Sat;

    wallet = await new Wallet(seed, []);

    await wallet.addMint(mint_url);
    await wallet.refreshMint(mint_url);



    let amount = 10;

    let quote = await wallet?.mintQuote($mint_url, BigInt(amount), currency);
    let quote_id = quote?.id;

    let invoice = quote?.request;
    if (invoice != undefined) {
        data = invoice;
    }

    let paid = false;
    while (paid == false) {
        let check_mint = await wallet?.mintQuoteStatus(mint_url, quote_id);
        if (check_mint?.paid == true) {
            paid = true;
        } else {
            await new Promise((r) => setTimeout(r, 2000));
        }

        await wallet?.mint(
            mint_url,
            quote_id,
            undefined,
            undefined,
            undefined,
        );

        let token = await wallet?.send(
            mint_url,
            currency,
            undefined,
            BigInt(amount) undefined,
            undefined,
        );

        console.log(token);


    }
}

main();
