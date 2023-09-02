Cashu Sdk Python bindings


**ALPHA** This library is in early development, the api will change.

## Supported Nuts:

Check: [https://github.com/thesimplekid/cashu-crab#implemented-nuts](https://github.com/thesimplekid/cashu-crab#implemented-nuts)

## Build the package

```shell
just python
```

## Getting Started

For now this is not published as a package as it is still in early development. So you will have to build it as above. In the future this will be pulished and pip can be used to install. 

```python
from cashu_sdk import Wallet, Client, Amount

client = Client("https://mutinynet-cashu.thesimplekid.space")

mint_keys = client.get_keys()

wallet = Wallet(client, mint_keys)

mint_request = wallet.request_mint(Amount.from_sat(10))

print(mint_request.invoice())

print(mint_request.hash())

```


## License

Code is under the [BSD 3-Clause License](LICENSE)

## Contribution

All contributions welcome.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, shall be licensed as above, without any additional terms or conditions.

## Contact

I can be contacted for comments or questions on nostr at _@thesimplekid.com (npub1qjgcmlpkeyl8mdkvp4s0xls4ytcux6my606tgfx9xttut907h0zs76lgjw) or via email tsk@thesimplekid.com.
