#!/usr/bin/env python3
"""Compare the generated portable wallet contract with its checked-in manifest."""

from __future__ import annotations

import ast
import difflib
import pathlib
import sys


OBJECTS = {
    "CashuWallet",
    "CrossMintTransferPlan",
    "MintSession",
    "PaymentPlan",
    "PaymentSession",
    "PendingPayment",
    "SendPlan",
    "Wallet",
}

# Top-level helpers that are part of opening the portable wallet. Without this
# list a rename or return-type change would bypass the object contract check.
FUNCTIONS = {
    "generate_mnemonic",
    "mnemonic_to_entropy",
}

# Records and enums used directly by the portable objects. Locking these shapes
# is as important as locking method signatures: changing a request field or an
# outcome variant changes every generated language binding.
CONTRACT_TYPES = {
    "Amount",
    "CashuWalletOpenRequest",
    "CrossMintClaimPending",
    "CrossMintTransferOutcome",
    "CrossMintTransferReceipt",
    "CrossMintTransferRequest",
    "CurrencyUnit",
    "FfiError",
    "HistoryEntry",
    "HistoryQuery",
    "MintRequest",
    "MintSessionState",
    "MintUrl",
    "MintingState",
    "PaymentConfirmation",
    "PaymentMethod",
    "PaymentQuote",
    "PaymentQuoteRequest",
    "PaymentReceipt",
    "PaymentState",
    "PaymentTarget",
    "RateLimit",
    "ReceiveReceipt",
    "ReceiveRequest",
    "Restored",
    "SendKind",
    "SendRequest",
    "SyncPolicy",
    "SyncReport",
    "TransactionDirection",
    "TransactionStatus",
    "WalletBalance",
    "WalletBalanceEntry",
    "WalletConfig",
    "WalletErrorKind",
    "WalletIdentity",
    "WalletOpenRequest",
    "WalletStore",
}

# These objects belong to the engine or to the old manually mirrored facade.
FORBIDDEN_OBJECTS = {
    "PreparedMelt",
    "PreparedSend",
    "PendingMelt",
    "WalletRepository",
}


def method_signature(class_name: str, node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    modifiers = []
    if isinstance(node, ast.AsyncFunctionDef):
        modifiers.append("async")

    decorators = {ast.unparse(decorator) for decorator in node.decorator_list}
    for decorator in ("classmethod", "staticmethod", "property"):
        if decorator in decorators:
            modifiers.append(decorator)

    modifiers.append("object")
    result = ast.unparse(node.returns) if node.returns else "None"
    return (
        f"{' '.join(modifiers)} {class_name}.{node.name}"
        f"({', '.join(parameters(node))}) -> {result}"
    )


def function_signature(node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    modifier = "async " if isinstance(node, ast.AsyncFunctionDef) else ""
    result = ast.unparse(node.returns) if node.returns else "None"
    return f"{modifier}function {node.name}({', '.join(parameters(node))}) -> {result}"


def parameters(node: ast.FunctionDef | ast.AsyncFunctionDef) -> list[str]:
    positional = list(node.args.posonlyargs) + list(node.args.args)
    defaults: list[ast.expr | None] = [None] * (len(positional) - len(node.args.defaults))
    defaults.extend(node.args.defaults)
    result = []
    for argument, default in zip(positional, defaults, strict=True):
        if argument.arg in {"self", "cls"}:
            continue
        annotation = ast.unparse(argument.annotation) if argument.annotation else "typing.Any"
        default_text = (
            "default"
            if isinstance(default, ast.Name) and default.id == "_DEFAULT"
            else ast.unparse(default) if default is not None else None
        )
        suffix = f" = {default_text}" if default_text is not None else ""
        result.append(f"{argument.arg}: {annotation}{suffix}")

    for argument, default in zip(node.args.kwonlyargs, node.args.kw_defaults, strict=True):
        annotation = ast.unparse(argument.annotation) if argument.annotation else "typing.Any"
        default_text = (
            "default"
            if isinstance(default, ast.Name) and default.id == "_DEFAULT"
            else ast.unparse(default) if default is not None else None
        )
        suffix = f" = {default_text}" if default_text is not None else ""
        result.append(f"{argument.arg}: {annotation}{suffix}")
    return result


def constructor(node: ast.ClassDef) -> ast.FunctionDef | None:
    return next(
        (
            item
            for item in node.body
            if isinstance(item, ast.FunctionDef) and item.name == "__init__"
        ),
        None,
    )


def is_simple_enum(node: ast.ClassDef) -> bool:
    return any(ast.unparse(base) == "enum.Enum" for base in node.bases)


def type_shape(name: str, node: ast.ClassDef) -> str:
    if is_simple_enum(node):
        variants = [
            target.id
            for item in node.body
            if isinstance(item, ast.Assign)
            and len(item.targets) == 1
            and isinstance((target := item.targets[0]), ast.Name)
            and target.id.isupper()
        ]
        if not variants:
            raise ValueError(f"generated enum {name} has no variants")
        return f"enum {name}({', '.join(variants)})"

    variants = []
    for item in node.body:
        if not isinstance(item, ast.ClassDef) or item.name.startswith("_"):
            continue
        init = constructor(item)
        if init is not None:
            variants.append(f"{item.name}({', '.join(parameters(init))})")
    if variants:
        return f"enum {name}({' | '.join(variants)})"

    init = constructor(node)
    if init is None:
        raise ValueError(f"generated contract type {name} has no constructor or variants")
    return f"record {name}({', '.join(parameters(init))})"


def public_contract(tree: ast.Module) -> tuple[list[str], set[str]]:
    # Some generated exception enums use a temporary base class followed by a
    # second class with variant definitions. Keeping the last definition gives
    # us the actual public shape.
    classes = {node.name: node for node in tree.body if isinstance(node, ast.ClassDef)}
    missing = OBJECTS - classes.keys()
    if missing:
        names = ", ".join(sorted(missing))
        raise ValueError(f"generated binding is missing wallet objects: {names}")
    missing_types = CONTRACT_TYPES - classes.keys()
    if missing_types:
        names = ", ".join(sorted(missing_types))
        raise ValueError(f"generated binding is missing wallet contract types: {names}")

    functions = {
        node.name: node
        for node in tree.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    missing_functions = FUNCTIONS - functions.keys()
    if missing_functions:
        names = ", ".join(sorted(missing_functions))
        raise ValueError(f"generated binding is missing wallet functions: {names}")

    methods = [
        method_signature(class_name, node)
        for class_name in OBJECTS
        for node in classes[class_name].body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and not node.name.startswith("_")
    ]
    top_level = [function_signature(functions[name]) for name in FUNCTIONS]
    shapes = [type_shape(name, classes[name]) for name in CONTRACT_TYPES]
    return sorted(methods + top_level + shapes), classes.keys() & FORBIDDEN_OBJECTS


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
    actual, forbidden = public_contract(tree)
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
