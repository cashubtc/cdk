from cashu_sdk import Mint;

mint = Mint("supersecret", "0/0/0/0", {}, [], 32)

print(mint.active_keyset_pubkeys().keys().as_hashmap())
