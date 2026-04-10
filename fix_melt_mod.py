import re
with open('crates/cdk/src/mint/melt/mod.rs', 'r') as f:
    content = f.read()

# Just replace `payment_quote.extra_json,` followed by `);` with `payment_quote.extra_json, payment_quote.estimated_blocks,`
content = re.sub(r'payment_quote\.extra_json,\n(\s*\);)', r'payment_quote.extra_json,\n            payment_quote.estimated_blocks,\n\1', content)

with open('crates/cdk/src/mint/melt/mod.rs', 'w') as f:
    f.write(content)
