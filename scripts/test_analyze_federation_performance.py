"""Acceptance checks for performance evidence, independent of production code."""

import copy
import unittest

from analyze_federation_performance import compare, latency_metrics, payment_window


def records():
    return [
        {
            "schema_version": 1, "pair": pair, "arm": arm,
            "order": 1 + ((pair + index) % 2),
            "revision": arm, "artifact_sha256": arm,
            "workload": {"name": "fixture", "members": 5},
            "sample_count": 100, "higher_is_better": ["operations_per_second"],
            "metrics": {"p95_ms": 100 if index == 0 else 80,
                        "operations_per_second": 10 if index == 0 else 12,
                        "p99_ms": None},
        }
        for pair in range(1, 6)
        for index, arm in enumerate(("baseline", "candidate"))
    ]


class EvidenceTests(unittest.TestCase):
    def test_late_and_queued_payments_do_not_inflate_window_throughput(self):
        def attempt(index, scheduled, started, finished, outcome):
            return dict(id=index, scheduled_ns=scheduled, started_ns=started,
                        finished_ns=finished, outcome=outcome)
        result = payment_window({"window_ns": 1_000_000_000, "attempts": [
            attempt(1, 0, 100_000_000, 200_000_000, "completed"),
            attempt(2, 1, 900_000_000, 1_000_000_000, "completed"),
            attempt(3, 2, 1_100_000_000, 1_200_000_000, "completed"),
            attempt(4, 3, None, None, "pending"),
            attempt(5, 4, None, 5, "rejected"),
        ]})
        self.assertEqual(result["completed_payments_per_second"], 1)
        self.assertEqual(result["offered_payments_per_second"], 5)
        self.assertEqual(result["late_completed"], 2)
        self.assertEqual(result["unfinished_at_window_end"], 3)
        self.assertEqual(result["client_latency"]["p50_ms"], 200)
        self.assertEqual(result["client_latency_population"], "completed_inside_window_only")
        self.assertEqual(result["late_client_latency_population"], "completed_during_drain_only")
        self.assertIsNone(result["sustainable_throughput"])

    def test_latency_tail_requires_enough_samples(self):
        self.assertIsNone(latency_metrics([1] * 999, "ms")["p99_ms"])
        self.assertEqual(latency_metrics(list(range(1, 1001)), "ms")["p99_ms"], 990)

    def test_independent_pairs_and_metric_directions(self):
        result = compare(records())[0]
        self.assertEqual(result["pair_count"], 5)
        self.assertAlmostEqual(result["metrics"]["p95_ms"]["candidate_over_baseline"], 0.8)
        self.assertEqual(result["metrics"]["operations_per_second"]["status"], "improvement")
        self.assertEqual(result["metrics"]["p99_ms"]["status"], "insufficient_samples")

    def test_rejects_unmatched_workload_or_duplicate_arm(self):
        data = records()
        data[0]["workload"]["members"] = 11
        with self.assertRaises(ValueError):
            compare(data)
        data = records()
        data.append(copy.deepcopy(data[0]))
        with self.assertRaises(ValueError):
            compare(data)

    def test_rejects_pseudoreplication_and_order_bias(self):
        with self.assertRaises(ValueError):
            compare(records()[:2])
        data = records()
        for record in data:
            record["order"] = 1 if record["arm"] == "baseline" else 2
        with self.assertRaises(ValueError):
            compare(data)

    def test_observed_fee_and_proof_shape_must_match_within_each_pair(self):
        data = records()
        for record in data:
            record["workload"]["require_matched_inputs"] = True
            record["matched_inputs"] = {"fee_sat": 200 + record["pair"],
                                        "selected_proofs": [3, 9]}
        self.assertEqual(compare(data)[0]["pair_count"], 5)
        data[1]["matched_inputs"]["fee_sat"] += 1
        with self.assertRaisesRegex(ValueError, "observed inputs differ"):
            compare(data)

    def test_required_input_observations_cannot_be_omitted(self):
        data = records()
        for record in data:
            record["workload"]["require_matched_inputs"] = True
        with self.assertRaisesRegex(ValueError, "input observations are required"):
            compare(data)
        data = records()
        data[0]["matched_inputs"] = {"fee_sat": 200}
        with self.assertRaisesRegex(ValueError, "input observations are required"):
            compare(data)

    def test_noise_is_inconclusive_and_regression_is_detected(self):
        data = records()
        for index, record in enumerate(data[1::2]):
            record["metrics"]["p95_ms"] = [70, 130, 90, 110, 100][index]
            record["metrics"]["operations_per_second"] = 8
        result = compare(data)[0]["metrics"]
        self.assertEqual(result["p95_ms"]["status"], "inconclusive")
        self.assertEqual(result["operations_per_second"]["status"], "regression_over_5_percent")

    def test_missing_metric_and_small_sample_p99_are_rejected(self):
        data = records()
        for record in data[:2]:
            del record["metrics"]["p99_ms"]
        with self.assertRaises(ValueError):
            compare(data)
        data = records()
        data[0]["metrics"]["p99_ms"] = 100
        with self.assertRaises(ValueError):
            compare(data)


if __name__ == "__main__":
    unittest.main()
