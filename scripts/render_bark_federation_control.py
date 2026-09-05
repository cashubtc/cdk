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

    # The shared control script already implements Bark and database-backed
    # startup. Customize its defaults without rewriting runtime behavior.
    import re

    defaults = {
        "BARK_SERVER_ADDRESS": args.server_address,
        "BARK_ESPLORA_URL": args.esplora_url,
        "BARK_ARK_SERVER_PUBLIC_KEY": args.ark_server_public_key,
        "BARK_APPLICATION_KEY_EPOCH": args.application_key_epoch,
        "ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS": args.aleph_bft_active_unit_interval_ms,
        "ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK": args.aleph_bft_network_pump_max_messages_per_tick,
    }
    for name, value in defaults.items():
        if not re.fullmatch(r"[A-Za-z0-9._:/-]+", value):
            raise SystemExit(f"invalid shell default for {name}")
        pattern = rf'^{name}=".*"$'
        replacement = f'{name}="${{{name}:-{value}}}"'
        text, count = re.subn(pattern, lambda _: replacement, text, flags=re.MULTILINE)
        if count != 1:
            raise SystemExit(f"expected one default for {name}, found {count}")

    args.output.write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
