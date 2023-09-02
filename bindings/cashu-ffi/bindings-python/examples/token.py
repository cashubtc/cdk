from cashu_protocol import Token, Proof, PublicKey, Amount

token_str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJpZCI6IkRTQWw5bnZ2eWZ2YSIsImFtb3VudCI6Miwic2VjcmV0IjoiRWhwZW5uQzlxQjNpRmxXOEZaX3BadyIsIkMiOiIwMmMwMjAwNjdkYjcyN2Q1ODZiYzMxODNhZWNmOTdmY2I4MDBjM2Y0Y2M0NzU5ZjY5YzYyNmM5ZGI1ZDhmNWI1ZDQifSx7ImlkIjoiRFNBbDludnZ5ZnZhIiwiYW1vdW50Ijo4LCJzZWNyZXQiOiJUbVM2Q3YwWVQ1UFVfNUFUVktudWt3IiwiQyI6IjAyYWM5MTBiZWYyOGNiZTVkNzMyNTQxNWQ1YzI2MzAyNmYxNWY5Yjk2N2EwNzljYTk3NzlhYjZlNWMyZGIxMzNhNyJ9XX1dLCJtZW1vIjoiVGhhbmsgeW91LiJ9"

token = Token.from_string(token_str)


print(token.memo())
for p in token.token():
    print(p.url())
    for proof in p.proofs():
        print(proof.id())
        print(proof.amount().to_sat())
        print(proof.secret())
        print(proof.c().to_hex())


proof_one = Proof(Amount.from_sat(2), "EhpennC9qB3iFlW8FZ_pZw", PublicKey.from_hex("02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4"), "DSAl9nvvyfva")

proof_two = Proof(Amount.from_sat(8), "TmS6Cv0YT5PU_5ATVKnukw", PublicKey.from_hex("02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7"), "DSAl9nvvyfva")

new_token = Token("https://8333.space:3338", [proof_one, proof_two], "Thank you.")

print(new_token.as_string())

# This is failing because of the url serialization.
# https://github.com/thesimplekid/cashu-crab/issues/13
# It is still a valid token, just does not match the reference
print(new_token.as_string == token_str)
