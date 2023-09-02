Cashu Python bindings


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
from cashu_protocol import Token, Proof, PublicKey, Amount

proof_one = Proof(Amount.from_sat(2), "EhpennC9qB3iFlW8FZ_pZw", PublicKey.from_hex("02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4"), "DSAl9nvvyfva")

proof_two = Proof(Amount.from_sat(8), "TmS6Cv0YT5PU_5ATVKnukw", PublicKey.from_hex("02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7"), "DSAl9nvvyfva")

new_token = Token("https://8333.space:3338", [proof_one, proof_two], "Thank you.")

print(new_token.as_string())

```


## License

Code is under the [BSD 3-Clause License](LICENSE)

## Contribution

All contributions welcome.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, shall be licensed as above, without any additional terms or conditions.

## Contact

I can be contacted for comments or questions on nostr at _@thesimplekid.com (npub1qjgcmlpkeyl8mdkvp4s0xls4ytcux6my606tgfx9xttut907h0zs76lgjw) or via email tsk@thesimplekid.com.
