import re
with open('crates/cdk-integration-tests/tests/integration_tests_pure.rs', 'r') as f:
    content = f.read()

content = re.sub(
    r'(Ok\(cdk_common::payment::SettingsResponse \{.*?bolt12: None,)',
    r'\1\n            onchain: None,',
    content,
    flags=re.DOTALL
)

content = re.sub(
    r'(Ok\(PaymentQuoteResponse \{.*?state: MeltQuoteState::Unpaid,\n\s*extra_json: None,)',
    r'\1\n                    estimated_blocks: None,',
    content,
    flags=re.DOTALL
)

content = re.sub(
    r'(\.get_melt_quote\(response\.quote\(\)\))',
    r'.get_melt_quote(response.quote().unwrap())',
    content
)

with open('crates/cdk-integration-tests/tests/integration_tests_pure.rs', 'w') as f:
    f.write(content)
