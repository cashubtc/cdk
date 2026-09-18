"""Checks on the wheel layout itself.

These import the installed `cdk` package rather than the raw generated module,
so they fail if the native library stops shipping beside it.
"""

from pathlib import Path

import cdk


def test_native_library_loads():
    mnemonic = cdk.generate_mnemonic()
    assert isinstance(mnemonic, str)
    assert len(mnemonic.split()) == 12


def test_exactly_one_library_ships_inside_the_package():
    package_dir = Path(cdk.__file__).parent
    libs = [
        p.name for p in package_dir.iterdir() if p.suffix in (".so", ".dylib", ".dll")
    ]
    assert len(libs) == 1, f"expected one native library, found {libs}"


def test_generated_module_ships_inside_the_package():
    assert (Path(cdk.__file__).parent / "cdk_ffi.py").is_file()


def test_public_api_is_reexported():
    for name in (
        "Wallet",
        "WalletConfig",
        "CurrencyUnit",
        "Amount",
        "Token",
        "SendOptions",
        "ReceiveOptions",
        "generate_mnemonic",
        "sqlite_wallet_store",
        "create_wallet_db",
    ):
        assert hasattr(cdk, name), f"cdk.{name} is missing from the package"
