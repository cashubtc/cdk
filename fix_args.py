import re

for filename in ['crates/cdk-common/src/database/mint/test/mint.rs', 'crates/cdk/src/mint/melt/mod.rs']:
    with open(filename, 'r') as f:
        content = f.read()
    
    # We will search for MeltQuote::new( ... ) and ensure it has 11 arguments.
    # Actually, since I only need to add `, None` or `, payment_quote.estimated_blocks`
    # Let's just do it manually via regex replacements for the specific known lines.
    
    if 'test/mint.rs' in filename:
        content = re.sub(r'(MeltQuote::new\(.*?KnownMethod::Bolt11\), None)(\));', r'\1, None\2;', content)
        content = re.sub(r'(PaymentMethod::Known\(KnownMethod::Bolt11\),\n\s*None)(\n\s*\));', r'\1,\n            None\2;', content)
    elif 'melt/mod.rs' in filename:
        content = re.sub(r'(PaymentMethod::Known\(KnownMethod::Bolt11\),\n\s*payment_quote.extra_json)(\n\s*\));', r'\1,\n            payment_quote.estimated_blocks\2;', content)
        content = re.sub(r'(PaymentMethod::Known\(KnownMethod::Bolt12\),\n\s*payment_quote.extra_json)(\n\s*\));', r'\1,\n            payment_quote.estimated_blocks\2;', content)
        content = re.sub(r'(PaymentMethod::Known\(KnownMethod::Onchain\),\n\s*payment_quote.extra_json)(\n\s*\));', r'\1,\n            payment_quote.estimated_blocks\2;', content)
        content = re.sub(r'(PaymentMethod::Custom.*?\n\s*payment_quote.extra_json)(\n\s*\));', r'\1,\n            payment_quote.estimated_blocks\2;', content)

    with open(filename, 'w') as f:
        f.write(content)
