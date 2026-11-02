"""Keep successful collection separate from the strength and scope of its evidence."""


DIMENSIONS = ("resolved_dependencies", "compiled_packages", "packaged_files", "upstream_native_identities",
              "final_bytes", "authenticated_provenance", "linker_inputs", "consumer_execution")
SATISFIED = {"verified", "observed", "inventoried", "authenticated", "passed"}


def assessment(index):
    coverage = index.get("coverage", {})
    kinds = {document["kind"] for document in index.get("documents", [])}
    result = {
        "resolved_dependencies": {"status": "verified" if kinds.intersection({
            "rust-resolved", "rust-consumer-resolved", "dart-resolved", "swift-resolved",
            "go-resolved", "gradle-resolved"}) else "not_collected"},
        "compiled_packages": {"status": "observed" if coverage.get("compiled_packages") else "not_observed"},
        "packaged_files": {"status": "inventoried" if index.get("distributions") else "not_requested"},
        "upstream_native_identities": {"status": "partial", "unresolved": index.get("unresolved_native_identities", []) + [
            {"reason": "Independent upstream composition of toolchain runtimes/implicit native inputs is not established."}]},
        "final_bytes": {"status": "verified" if index.get("distributions") and coverage.get("packaged_native_build_hashes_checked") else "not_verified"},
        "authenticated_provenance": {"status": "unsigned"},
        "linker_inputs": {"status": "observed" if coverage.get("linker_maps") else "build_records_only"},
        "consumer_execution": {"status": "passed" if coverage.get("go_native_load_test") or coverage.get("go_final_consumer_test") else "not_tested"},
    }
    if coverage.get("external_native_assertions"):
        result["final_bytes"] = {"status": "partial", "reason": "Some packaged identities rely on unauthenticated external native BOM assertions."}
    if coverage.get("swift_final_consumer_test"):
        result["consumer_execution"] = {"status": "passed", "scope": "native-host macOS only; no iOS execution claimed"}
    return {"version": 1, "overall": "partial", "dimensions": result}


def inherited_dimensions(indexes):
    """Retaining an inventory does not upgrade it to an observed compilation."""
    result = {}
    for name, success, missing in (("resolved_dependencies", "verified", "not_collected"),
                                   ("compiled_packages", "observed", "not_observed"),
                                   ("linker_inputs", "observed", "build_records_only")):
        reports = [index.get("assurance", assessment(index))["dimensions"].get(name, {}) for index in indexes]
        result[name] = {"status": success if reports and all(report.get("status") in SATISFIED for report in reports) else missing,
                        "scope": "retained input build indexes; not independently re-collected"}
    return result


def require(assurance, dimensions):
    missing = [name for name in dimensions if assurance["dimensions"][name]["status"] not in SATISFIED]
    if missing:
        raise ValueError("Required SBOM assurances are not satisfied: " + ", ".join(
            f"{name}={assurance['dimensions'][name]['status']}" for name in missing))
