#!/usr/bin/env python3
"""Render the shared federation control script with Bark Signet support."""

from argparse import ArgumentParser
from pathlib import Path


def parse_args():
    parser = ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--server-address", required=True)
    parser.add_argument("--esplora-url", required=True)
    parser.add_argument("--ark-server-public-key", required=True)
    parser.add_argument("--application-key-epoch", required=True)
    parser.add_argument("--aleph-bft-active-unit-interval-ms", required=True)
    parser.add_argument("--aleph-bft-network-pump-max-messages-per-tick", required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    text = args.source.read_text(encoding="utf-8")

    def replace_once(old: str, new: str) -> None:
        nonlocal text
        count = text.count(old)
        if count != 1:
            raise SystemExit(
                "control-script renderer expected one match, "
                f"found {count}: {old[:80]!r}"
            )
        text = text.replace(old, new)

    if "federated-bdk" not in text:
        raise SystemExit("control-script renderer could not find the BDK payment mode")
    text = text.replace("federated-bdk", "federated-bdk-bark")
    text = text.replace("https://mutinynet.com/api", "https://esplora.signet.2nd.dev/")
    text = text.replace("mutinynet-public-esplora", "2nddev-signet-esplora")
    text = text.replace("Mutinynet federated Cashu", "Signet federated Cashu")

    replace_once(
        'PAYMENT_MODE="${PAYMENT_MODE:-federated-bdk-bark}"\n',
        f'''PAYMENT_MODE="${{PAYMENT_MODE:-federated-bdk-bark}}"
BARK_SERVER_ADDRESS="${{BARK_SERVER_ADDRESS:-{args.server_address}}}"
BARK_ESPLORA_URL="${{BARK_ESPLORA_URL:-{args.esplora_url}}}"
BARK_ARK_SERVER_PUBLIC_KEY="${{BARK_ARK_SERVER_PUBLIC_KEY:-{args.ark_server_public_key}}}"
BARK_APPLICATION_KEY_EPOCH="${{BARK_APPLICATION_KEY_EPOCH:-{args.application_key_epoch}}}"
BARK_POLICY_CONFIG="$PWD/config/bark-policy.json"
''',
    )
    replace_once(
        'ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS="${ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS:-200}"',
        "ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS="
        f'"${{ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS:-{args.aleph_bft_active_unit_interval_ms}}}"',
    )
    replace_once(
        'ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK="${ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK:-1}"',
        "ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK="
        f'"${{ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK:-{args.aleph_bft_network_pump_max_messages_per_tick}}}"',
    )
    replace_once(
        "for command in tmux curl jq awk find ps seq sed cmp; do",
        "for command in tmux curl jq awk find ps seq sed cmp openssl; do",
    )
    replace_once(
        "    local wallet_config development_policy_config\n",
        """    local wallet_config bark_config development_policy_config
    local operational_seed_hex receive_secret_key_hex
""",
    )
    replace_once(
        '''        wallet_config="enabled = false"
        development_policy_config="allow_development_payment_observation_policy = true"
''',
        '''        wallet_config="enabled = false"
        bark_config="enabled = false"
        development_policy_config="allow_development_payment_observation_policy = true"
''',
    )
    replace_once(
        '''        ln_backend="none"
        onchain_backend="federated-bdk-bark"
        development_policy_config=""
        wallet_config="$(printf '%s\\n' \\
''',
        '''        ln_backend="federated-bark"
        onchain_backend="federated-bdk"
        development_policy_config=""
        operational_seed_hex="$(openssl rand -hex 64)"
        receive_secret_key_hex="$(openssl rand -hex 32)"
        wallet_config="$(printf '%s\\n' \\
''',
    )
    replace_once(
        '''            'sync_interval_secs = 15')"
    fi
    mkdir -p "$dir/work"
''',
        '''            'sync_interval_secs = 15')"
        bark_config="$(printf '%s\\n' \\
            'enabled = true' \\
            "network = \\"$NETWORK\\"" \\
            "ark_server_public_key = \\"$BARK_ARK_SERVER_PUBLIC_KEY\\"" \\
            "server_address = \\"$BARK_SERVER_ADDRESS\\"" \\
            "esplora_url = \\"$BARK_ESPLORA_URL\\"" \\
            "data_directory = \\"$dir/work/federation-bark\\"" \\
            "receive_secret_database_path = \\"$dir/work/federation-bark/receive-secrets.redb\\"" \\
            "frost_protocol_storage_root = \\"$dir/work/frost-protocol\\"" \\
            "operational_seed_hex = \\"$operational_seed_hex\\"" \\
            "receive_secret_key_hex = \\"$receive_secret_key_hex\\"" \\
            'create_if_missing = true' \\
            'automatic_refresh = true' \\
            'automatic_emergency_exit = false')"
    fi
    mkdir -p "$dir/work/frost-protocol"
''',
    )
    replace_once(
        '''[federation.onchain_wallet]
$wallet_config
EOF
''',
        '''[federation.onchain_wallet]
$wallet_config

[federation.bark]
$bark_config
EOF
''',
    )
    replace_once(
        '''    mkdir -p "$PWD/config" "$PWD/logs"

    local member
''',
        '''    mkdir -p "$PWD/config" "$PWD/logs"

    if [[ "$PAYMENT_MODE" == "federated-bdk-bark" ]]; then
        cat > "$BARK_POLICY_CONFIG" <<'EOF'
{
  "version": 1,
  "receive_base_fee_sat": 0,
  "receive_fee_ppm": 0,
  "maximum_outgoing_base_fee_sat": 20,
  "maximum_outgoing_fee_ppm": 5000,
  "minimum_incoming_cltv_delta": 18,
  "minimum_outgoing_cltv_delta": 18,
  "refresh_before_expiry_blocks": 72,
  "emergency_exit_before_expiry_blocks": 18,
  "receive_secret_retention_days": 30,
  "receive_anti_dos": "no_proof"
}
EOF
    fi

    local member
''',
    )
    replace_once(
        '''        --frost-application-key-epoch "$APPLICATION_KEY_EPOCH" \\
''',
        '''        --frost-application-key-epoch "$APPLICATION_KEY_EPOCH" \\
        --frost-bark-ark-server-public-key "$BARK_ARK_SERVER_PUBLIC_KEY" \\
        --frost-bark-application-key-epoch "$BARK_APPLICATION_KEY_EPOCH" \\
        --frost-bark-policy-config "$BARK_POLICY_CONFIG" \\
''',
    )
    text = text.replace(
        ".frost_wallet != null'",
        ".frost_wallet != null and .frost_wallet.bark_application != null'",
    )

    args.output.write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
