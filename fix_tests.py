import re

filename = 'crates/cdk-common/src/database/mint/test/mint.rs'
with open(filename, 'r') as f:
    content = f.read()

# The pattern is:
#         PaymentMethod::Known(KnownMethod::Bolt11),
#         Some(serde_json::json!({"processor": "metadata"})),
#     );
# Or:
#         None,
#     );

content = re.sub(
    r'(PaymentMethod::Known\(KnownMethod::Bolt11\),\n\s*Some\(serde_json::json!.*?\)),\n(\s*\);)',
    r'\1,\n        None\2',
    content
)

content = re.sub(
    r'(PaymentMethod::Known\(KnownMethod::Bolt11\),\n\s*None),\n(\s*\);)',
    r'\1,\n        None\2',
    content
)

with open(filename, 'w') as f:
    f.write(content)
