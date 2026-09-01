#!/usr/bin/env python3
"""Compare the generated portable wallet object API with its checked-in manifest."""

from __future__ import annotations

import ast
import difflib
import pathlib
import sys


OBJECTS = {
    "CashuWallet",
    "MintSession",
    "PaymentPlan",
    "PaymentSession",
    "PendingPayment",
    "PreparedCrossMintTransfer",
    "SendPlan",
    "Wallet",
}

# These objects belong to the engine or to the old manually mirrored facade.
FORBIDDEN_OBJECTS = {
    "PreparedMelt",
    "PreparedSend",
    "PendingMelt",
    "WalletRepository",
}


def method_signature(class_name: str, node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    positional = list(node.args.posonlyargs) + list(node.args.args)
    if positional and positional[0].arg in {"self", "cls"}:
        positional = positional[1:]

    parameters = []
    for argument in positional:
        annotation = ast.unparse(argument.annotation) if argument.annotation else "typing.Any"
        parameters.append(f"{argument.arg}: {annotation}")
    for argument in node.args.kwonlyargs:
        annotation = ast.unparse(argument.annotation) if argument.annotation else "typing.Any"
        parameters.append(f"{argument.arg}: {annotation}")

    result = ast.unparse(node.returns) if node.returns else "None"
    return f"{class_name}.{node.name}({', '.join(parameters)}) -> {result}"


def public_methods(tree: ast.Module) -> tuple[list[str], set[str]]:
    classes = {
        node.name: node for node in tree.body if isinstance(node, ast.ClassDef)
    }
    missing = OBJECTS - classes.keys()
    if missing:
        names = ", ".join(sorted(missing))
        raise ValueError(f"generated binding is missing wallet objects: {names}")

    entries = [
        method_signature(class_name, node)
        for class_name in OBJECTS
        for node in classes[class_name].body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and not node.name.startswith("_")
    ]
    return sorted(entries), classes.keys() & FORBIDDEN_OBJECTS


def manifest_entries(path: pathlib.Path) -> list[str]:
    return sorted(
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    )


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: check-wallet-api.py GENERATED_PYTHON API_MANIFEST",
            file=sys.stderr,
        )
        return 2

    generated_path = pathlib.Path(sys.argv[1])
    manifest_path = pathlib.Path(sys.argv[2])
    tree = ast.parse(generated_path.read_text(encoding="utf-8"))
    actual, forbidden = public_methods(tree)
    expected = manifest_entries(manifest_path)

    if forbidden:
        names = ", ".join(sorted(forbidden))
        print(f"advanced wallet objects leaked into UniFFI: {names}", file=sys.stderr)
        return 1

    if actual == expected:
        print(f"portable wallet API matches {manifest_path}")
        return 0

    diff = difflib.unified_diff(
        [f"{entry}\n" for entry in expected],
        [f"{entry}\n" for entry in actual],
        fromfile=str(manifest_path),
        tofile="generated wallet API",
    )
    print("portable wallet API changed:", file=sys.stderr)
    sys.stderr.writelines(diff)
    print(
        "Review the change, then update the manifest deliberately if it is intended.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
