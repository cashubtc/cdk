#!/usr/bin/env python3
"""Normalize existing benchmark results and compare independent matched runs.

This is an offline analyzer, not a workload runner. Raw inputs and output belong
in private, ignored directories; publish reviewed aggregates only.
"""

import argparse
import json
import math
import statistics
from pathlib import Path


SCHEMA_VERSION = 1
# Two-sided 95% Student t critical values for the approved pair counts.
T_CRITICAL = {5: 2.776445, 10: 2.262157}


def percentile(samples, fraction):
    """Nearest-rank percentile, with no interpolation of unobserved tails."""
    ordered = sorted(samples)
    if not ordered or any(not math.isfinite(x) or x < 0 for x in ordered):
        raise ValueError("durations must be nonempty, finite and nonnegative")
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def latency_metrics(samples, unit):
    return {
        f"p50_{unit}": percentile(samples, 0.50),
        f"p95_{unit}": percentile(samples, 0.95),
        f"p99_{unit}": percentile(samples, 0.99) if len(samples) >= 1000 else None,
    }


def payment_window(report):
    """Account for a bounded arrival window on one monotonic client clock.

    Attempts carry numeric IDs, scheduled_ns, started_ns (nullable),
    finished_ns (nullable), and outcome: completed/failed/timeout/rejected/pending.
    Keep the operation's durable identity in the private driver's recovery log.
    """
    end = report["window_ns"]
    if not isinstance(end, int) or end <= 0:
        raise ValueError("window_ns must be a positive integer")
    counts = dict.fromkeys(("offered", "completed", "failed", "timeout", "rejected",
                           "unfinished_at_window_end", "late_completed", "late_failed",
                           "late_timeout", "late_rejected", "pending_at_drain_end"), 0)
    latencies = []
    late_latencies = []
    seen = set()
    for attempt in report["attempts"]:
        if attempt["id"] in seen:
            raise ValueError("duplicate attempt ID")
        seen.add(attempt["id"])
        scheduled, started, finished = (attempt[key] for key in
                                        ("scheduled_ns", "started_ns", "finished_ns"))
        outcome = attempt["outcome"]
        if outcome not in ("completed", "failed", "timeout", "rejected", "pending"):
            raise ValueError("unknown attempt outcome")
        if not isinstance(scheduled, int) or not 0 <= scheduled < end:
            raise ValueError("only arrivals within the measurement window belong in this report")
        if started is not None and (not isinstance(started, int) or started < scheduled):
            raise ValueError("invalid operation start")
        if finished is not None and (not isinstance(finished, int) or finished < (started or scheduled)):
            raise ValueError("invalid operation finish")
        if (outcome == "pending") != (finished is None):
            raise ValueError("only pending attempts may lack a terminal timestamp")
        if outcome == "completed" and started is None:
            raise ValueError("completion requires an operation start")
        counts["offered"] += 1
        if finished is None or finished >= end:
            counts["unfinished_at_window_end"] += 1
            if outcome == "pending":
                counts["pending_at_drain_end"] += 1
            else:
                counts[f"late_{outcome}"] += 1
                if outcome == "completed":
                    late_latencies.append((finished - scheduled) / 1_000_000)
        else:
            counts[outcome] += 1
            if outcome == "completed":
                latencies.append((finished - scheduled) / 1_000_000)
    seconds = end / 1_000_000_000
    return {
        **counts,
        "window_seconds": seconds,
        "offered_payments_per_second": counts["offered"] / seconds,
        "completed_payments_per_second": counts["completed"] / seconds,
        "client_latency": latency_metrics(latencies, "ms") if latencies else None,
        "late_client_latency": latency_metrics(late_latencies, "ms") if late_latencies else None,
        "sustainable_throughput": None,  # Requires queue stability and load sweeps.
    }


def normalize(kind, raw, metadata):
    """Wrap existing DKG/Aleph reports without treating phases as payments."""
    base = {
        "schema_version": SCHEMA_VERSION,
        **metadata,
    }
    if kind == "dkg":
        workload = {
            "harness_version": raw["harness_version"],
            "members": raw["member_count"],
            "threshold": raw["signature_threshold"],
            "denominations": raw["denomination_count"],
            "build_mode": raw["build_mode"],
            "transport": raw["transport"]["mode"],
            "iterations": raw["iterations"],
        }
        return [
            {
                **base,
                "workload": {**metadata["workload"], **workload, "phase": phase["name"]},
                "sample_count": len(phase["samples_us"]),
                "metrics": latency_metrics(phase["samples_us"], "us"),
                "higher_is_better": [],
            }
            for phase in raw["phases"]
        ]
    if kind != "aleph":
        raise ValueError(f"unsupported harness: {kind}")
    if len(raw["latencies_ms"]) != raw["swaps"] or raw["steady_state_ms"] <= 0:
        raise ValueError("incomplete or invalid Aleph run")
    workload = {key: raw[key] for key in (
        "members", "signature_threshold", "unit_interval_ms",
        "pump_messages_per_tick", "network_rtt_ms", "network_jitter_percent", "swaps",
    )}
    return [{
        **base,
        "workload": {**metadata["workload"], **workload,
                     "transport": "in_process_axum", "phase": "serial_swap"},
        "sample_count": raw["swaps"],
        "metrics": {
            **latency_metrics(raw["latencies_ms"], "ms"),
            "consensus_operations_per_second": raw["swaps"] * 1000 / raw["steady_state_ms"],
            "delivery_bytes": raw["delivery_bytes"],
            "pump_ticks": raw["pump_ticks"],
            "outbound_messages_dropped": raw["outbound_messages_dropped"],
            "retry_deliveries_dropped": raw["retry_deliveries_dropped"],
        },
        "higher_is_better": ["consensus_operations_per_second"],
    }]


def compare(records):
    """Pair run-level metrics; never treat transactions as independent runs.

    Intervals use log candidate/baseline ratios and Student t over independent
    pairs. The interval describes run variability, not proof of causation. A
    statistical improvement still requires work counters and correctness gates.
    """
    groups = {}
    for record in records:
        if record["schema_version"] != SCHEMA_VERSION:
            raise ValueError("unsupported result schema")
        if record["arm"] not in ("baseline", "candidate"):
            raise ValueError("arm must be baseline or candidate")
        if not record.get("revision") or not record.get("artifact_sha256"):
            raise ValueError("revision and artifact fingerprint are required")
        if record["sample_count"] <= 0:
            raise ValueError("empty runs must be investigated, not compared")
        key = json.dumps(record["workload"], sort_keys=True)
        group = groups.setdefault(key, {})
        pair = group.setdefault(record["pair"], {})
        if record["arm"] in pair:
            raise ValueError("duplicate arm in a matched pair")
        pair[record["arm"]] = record

    comparisons = []
    for workload, pairs in groups.items():
        count = len(pairs)
        if count not in T_CRITICAL:
            raise ValueError("comparison requires five or ten independent matched pairs")
        metric_pairs = {}
        directions = None
        arm_fingerprints = {}
        previous_first = None
        for pair_id in sorted(pairs):
            pair = pairs[pair_id]
            if set(pair) != {"baseline", "candidate"}:
                raise ValueError("unmatched run or mismatched workload")
            baseline, candidate = pair["baseline"], pair["candidate"]
            # Configuration alone cannot detect changing external inputs such
            # as fee estimates, which also change wallet proof selection.
            inputs = [record.get("matched_inputs") for record in (baseline, candidate)]
            if baseline["workload"].get("require_matched_inputs") or any(
                    value is not None for value in inputs):
                if any(not isinstance(value, dict) or not value for value in inputs):
                    raise ValueError("matched input observations are required for both arms")
                if inputs[0] != inputs[1]:
                    raise ValueError("observed inputs differ within a matched pair")
            if {baseline["order"], candidate["order"]} != {1, 2}:
                raise ValueError("each pair must record execution order 1 and 2")
            first = "baseline" if baseline["order"] == 1 else "candidate"
            if first == previous_first:
                raise ValueError("pair execution order must alternate")
            previous_first = first
            if baseline["metrics"].keys() != candidate["metrics"].keys():
                raise ValueError("matched runs must contain the same metric names")
            for arm, record in pair.items():
                fingerprint = (record["revision"], record["artifact_sha256"])
                if arm_fingerprints.setdefault(arm, fingerprint) != fingerprint:
                    raise ValueError("an arm must use the same revision and binary")
                current_directions = set(record["higher_is_better"])
                if directions is None:
                    directions = current_directions
                if directions != current_directions:
                    raise ValueError("metric directions differ between runs")
                for name, value in record["metrics"].items():
                    if value is not None and (not math.isfinite(value) or value < 0):
                        raise ValueError("metrics must be finite and nonnegative")
                    if name.startswith("p99_") and value is not None and record["sample_count"] < 1000:
                        raise ValueError("p99 requires at least 1000 completed samples per run")
            for name, value in baseline["metrics"].items():
                metric_pairs.setdefault(name, []).append((value, candidate["metrics"][name]))
        results = {}
        for name, samples in metric_pairs.items():
            if len(samples) != count:
                raise ValueError("metric missing from some pairs")
            if any(a is None or b is None for a, b in samples):
                results[name] = {"status": "insufficient_samples", "pairs": samples}
                continue
            if any(a == 0 or b == 0 for a, b in samples):
                results[name] = {"status": "zero_values_require_absolute_analysis", "pairs": samples}
                continue
            ratios = [math.log(b / a) for a, b in samples]
            mean = statistics.mean(ratios)
            margin = T_CRITICAL[count] * statistics.stdev(ratios) / math.sqrt(count)
            low, high = math.exp(mean - margin), math.exp(mean + margin)
            higher = name in directions
            improved = low > 1 if higher else high < 1
            regressed = high < 0.95 if higher else low > 1.05
            results[name] = {
                "status": "regression_over_5_percent" if regressed else
                          "improvement" if improved else "inconclusive",
                "candidate_over_baseline": math.exp(mean),
                "ratio_95_percent_ci": [low, high],
                "pairs": samples,
            }
        comparisons.append({"workload": json.loads(workload), "pair_count": count, "metrics": results})
    if not comparisons:
        raise ValueError("no runs supplied")
    return comparisons


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    normalizer = sub.add_parser("normalize")
    normalizer.add_argument("kind", choices=("dkg", "aleph"))
    normalizer.add_argument("input", type=Path)
    normalizer.add_argument("metadata", type=Path)
    comparison = sub.add_parser("compare")
    comparison.add_argument("input", type=Path, help="normalized JSONL of all paired runs")
    window = sub.add_parser("payment-window")
    window.add_argument("input", type=Path, help="one-clock bounded payment arrival/finish report")
    args = parser.parse_args()
    try:
        if args.command == "normalize":
            for record in normalize(args.kind, json.loads(args.input.read_text()),
                                    json.loads(args.metadata.read_text())):
                print(json.dumps(record, allow_nan=False))
        elif args.command == "compare":
            records = [json.loads(line) for line in args.input.read_text().splitlines() if line.strip()]
            print(json.dumps(compare(records), indent=2, allow_nan=False))
        else:
            print(json.dumps(payment_window(json.loads(args.input.read_text())), indent=2))
    except (ValueError, KeyError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
