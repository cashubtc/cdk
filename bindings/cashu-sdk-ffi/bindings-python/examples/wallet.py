from cashu_sdk import Wallet, Client, Amount

client = Client("https://mutinynet-cashu.thesimplekid.space")

mint_keys = client.get_keys()

wallet = Wallet(client, mint_keys)

mint_request = wallet.request_mint(Amount.from_sat(10))

print(mint_request.invoice())

print(mint_request.hash())
