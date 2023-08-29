from cashu_protocol import Amount

amount = Amount().from_sat(10)

print(amount.to_sat())
