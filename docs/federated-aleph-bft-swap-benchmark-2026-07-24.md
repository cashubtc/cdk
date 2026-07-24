# AlephBFT serial-swap parameter benchmark

Date: 2026-07-24

## Scope and method

This benchmark measures the steady-state finality latency of an ordinary
federated swap while varying only:

1. the active AlephBFT unit-creation interval; and
2. the maximum number of fresh outbound messages drained by one federation
   network-pump invocation.

Production defaults were not changed. Every run created a fresh, healthy
five-member in-process federation, completed one warm-up swap, and then
submitted ten valid swaps strictly serially. The next swap was submitted only
after all five members had applied the previous operation. The benchmark also
asserted journal/state convergence and that no member entered lagging or
catch-up.

The upstream round-zero delay was preserved, as were all pump tick, recovery,
request, rebroadcast, retry, channel-capacity, and delivery settings. The
requested duplicate `100 ms / 1` matrix entry was deduplicated; each of the
eight unique configurations ran three times.

`startup_ms` includes federation startup and the excluded warm-up swap.
Operation latency is measured from `SubmitQueued` to `WaiterReturned`.

## Results

All values are milliseconds. `Median` is the median of the three run medians.
With ten samples per run, nearest-rank p95 is that run's maximum; `worst p95`
is therefore the slowest operation across the three runs. `range` spans all
30 measured operations for the configuration.

| Unit / pump | Run 1 median | Run 2 median | Run 3 median | Median | Worst p95 | Range | Startup median |
|---|---:|---:|---:|---:|---:|---:|---:|
| 500 / 1 | 2500.4 | 2514.7 | 2506.8 | 2506.8 | 3005.7 | 2006.1–3005.7 | 7178.9 |
| 250 / 1 | 1255.2 | 1254.2 | 1248.2 | 1254.2 | 1489.2 | 997.2–1489.2 | 6183.7 |
| 100 / 1 | 503.1 | 504.1 | 498.4 | 503.1 | 582.1 | 382.8–582.1 | 5570.6 |
| 250 / 2 | 1243.4 | 1247.0 | 1256.2 | 1247.0 | 1484.6 | 990.4–1484.6 | 6189.9 |
| 100 / 2 | 504.2 | 498.9 | 498.2 | 498.9 | 605.0 | 392.4–605.0 | 5582.5 |
| 100 / 8 | 493.4 | 504.2 | 505.5 | 504.2 | 605.9 | 398.2–605.9 | 5586.0 |
| 50 / 1 | 243.8 | 254.8 | 248.8 | 248.8 | 279.9 | 184.2–279.9 | 5372.0 |
| 50 / 8 | 246.2 | 242.9 | 247.4 | **246.2** | **278.5** | 192.0–278.5 | 5386.9 |

The median traffic values below cover ten measured swaps. A logical outbound
message is one message drained from a member's AlephBFT bridge. A delivery is
one signed private-API request to one peer, so these five-member broadcast
results contain four deliveries per logical outbound message. Application and
helper classification is based on whether the decoded AlephBFT message carries
included application data.

| Unit / pump | Logical outbounds | Deliveries | App | Helper | Delivery bytes | Deliveries vs baseline | Bytes vs baseline |
|---|---:|---:|---:|---:|---:|---:|---:|
| 500 / 1 | 261 | 1044 | 40 | 1004 | 184032 | baseline | baseline |
| 250 / 1 | 255 | 1020 | 40 | 980 | 180840 | -2.3% | -1.7% |
| 100 / 1 | 251 | 1004 | 40 | 964 | 178712 | -3.8% | -2.9% |
| 250 / 2 | 239 | 956 | 40 | 916 | 172328 | -8.4% | -6.4% |
| 100 / 2 | 251 | 1004 | 40 | 964 | 178712 | -3.8% | -2.9% |
| 100 / 8 | 260 | 1040 | 40 | 1000 | 183500 | -0.4% | -0.3% |
| 50 / 1 | 254 | 1016 | 40 | 976 | 180308 | -2.7% | -2.0% |
| 50 / 8 | 249 | 996 | 40 | 956 | 177648 | -4.6% | -3.5% |

Traffic did not increase relative to `500 / 1`. The modest decreases and
configuration-to-configuration variation are helper-message timing variance;
the application-bearing count remained exactly 40 deliveries, or one logical
application-bearing broadcast per swap, in every run.

## Localhost 25 ms extension

The new request-body metric serializes the complete signed JSON request sent to
each peer. It is a better application-layer traffic estimate than the raw SCALE
payload, but remains a lower bound because the in-process transport has no
HTTP/TCP/TLS headers and response bodies are not counted. Rates are aggregate
across all five members and use decimal kbit/s.

| Unit / pump | Run 1 median | Run 2 median | Run 3 median | Median | Worst p95 | Signed request bytes / 10 swaps | kbit/s | Additional vs `500 / 1` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 500 / 1 | 2491.8 | 2502.1 | 2496.5 | 2496.5 | 3006.1 | 829688 | 254.4 | baseline |
| 25 / 1 | 139.7 | 139.5 | 141.4 | 139.7 | 168.8 | 887448 | 4520.6 | +4266.2 kbit/s |
| 25 / 8 | 148.3 | 139.1 | 137.4 | **139.1** | **166.3** | 884560 | 4512.6 | +4258.2 kbit/s |

Both 25 ms configurations completed all 30 swaps with queue depth one and no
backlog, retries, or drops. They were effectively tied. Request bytes per ten
swaps rose only about 7%, but aggregate bandwidth rose about 17.8 times because
the same consensus work was compressed into a much shorter window.

## RTT and jitter extension

### Network model

The transport applies a deterministic uniformly sampled RTT within ±20% of
the configured nominal 50, 100, or 200 ms value. Half of each sampled RTT is
applied before request delivery and half before the response returns. Each
repetition uses a distinct seed; matching configurations use matching seeds.
The delay starts after warm-up, so startup remains separate and unchanged.

This is more representative than a fixed pre-delivery sleep: it delays both
consensus dissemination and the network pump's ability to complete a request.
It does not model loss, bandwidth caps, geographic asymmetry, or correlated
congestion.

Matched `500 / 1` baselines were also measured at every RTT:

| RTT | Run medians | Median | Worst p95 | Request bytes / 10 swaps | kbit/s |
|---:|---|---:|---:|---:|---:|
| 50 ms | 2736.6, 2525.9, 2967.8 | 2736.6 | 3013.1 | 878784 | 255.8 |
| 100 ms | 2966.7, 2981.4, 2954.3 | 2966.7 | 3008.0 | 901166 | 256.4 |
| 200 ms | 2974.4, 2973.0, 2942.8 | 2973.0 | 3015.9 | 945396 | 269.1 |

### Latency

All values are milliseconds. Each run median covers ten serial swaps and
`worst p95` is the slowest operation across the three runs.

| RTT | Unit / pump | Run 1 | Run 2 | Run 3 | Median | Worst p95 |
|---:|---:|---:|---:|---:|---:|---:|
| 50 | 100 / 1 | 575.7 | 571.7 | 537.9 | 571.7 | 616.5 |
| 50 | 100 / 8 | 577.2 | 570.9 | 535.4 | 570.9 | 606.5 |
| 50 | 50 / 1 | 345.0 | 346.1 | 352.7 | **346.1** | 423.7 |
| 50 | 50 / 8 | 341.1 | 359.2 | 355.8 | 355.8 | **399.2** |
| 50 | 25 / 1 | 349.6 | 357.5 | 356.2 | 356.2 | 426.9 |
| 50 | 25 / 8 | 346.8 | 359.6 | 327.6 | 346.8 | 426.3 |
| 100 | 100 / 1 | 736.1 | 681.7 | 682.8 | 682.8 | 913.9 |
| 100 | 100 / 8 | 650.2 | 682.7 | 677.0 | 677.0 | 855.8 |
| 100 | 50 / 1 | 640.1 | 687.9 | 681.9 | 681.9 | 911.4 |
| 100 | 50 / 8 | 675.6 | 704.5 | 693.1 | 693.1 | 833.8 |
| 100 | 25 / 1 | 673.0 | 683.2 | 665.6 | **673.0** | **831.7** |
| 100 | 25 / 8 | 674.5 | 696.1 | 685.7 | 685.7 | 850.2 |
| 200 | 100 / 1 | 1383.9 | 1349.4 | 1346.9 | 1349.4 | 1789.1 |
| 200 | 100 / 8 | 1339.9 | 1467.7 | 1457.5 | 1457.5 | 3140.9 |
| 200 | 50 / 1 | 1334.8 | 1371.3 | 1339.4 | 1339.4 | 1887.3 |
| 200 | 50 / 8 | 1442.3 | 1386.7 | 1352.6 | 1386.7 | 2876.7 |
| 200 | 25 / 1 | 1257.9 | 1353.3 | 1124.8 | **1257.9** | **1570.5** |
| 200 | 25 / 8 | 1399.8 | 1432.9 | 1323.2 | 1399.8 | 4068.4 |

### Traffic and backlog

`Additional kbit/s` compares each row with `500 / 1` under the same RTT.
Backlog and retry counts are totals across the three runs; queue depth is the
maximum. No run dropped an outbound or retry delivery.

| RTT | Unit / pump | Request kB / 10 swaps | kbit/s | Additional kbit/s | Max queue | Backlogged ticks | Retry messages |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 100 / 1 | 899.0 | 1253.9 | +998.1 | 1 | 0 | 0 |
| 50 | 100 / 8 | 899.0 | 1262.5 | +1006.7 | 1 | 1 | 1 |
| 50 | 50 / 1 | 933.7 | 2102.0 | +1846.2 | 2 | 838 | 1 |
| 50 | 50 / 8 | 956.8 | 2094.7 | +1838.9 | 2 | 92 | 0 |
| 50 | 25 / 1 | 948.1 | 2105.6 | +1849.8 | 2 | 860 | 0 |
| 50 | 25 / 8 | 916.3 | 2133.2 | +1877.4 | 3 | 97 | 0 |
| 100 | 100 / 1 | 966.1 | 1091.7 | +835.4 | 2 | 860 | 2 |
| 100 | 100 / 8 | 938.0 | 1089.2 | +832.8 | 2 | 95 | 8 |
| 100 | 50 / 1 | 933.7 | 1100.7 | +844.3 | 2 | 842 | 0 |
| 100 | 50 / 8 | 960.4 | 1091.2 | +834.9 | 2 | 93 | 12 |
| 100 | 25 / 1 | 931.5 | 1107.1 | +850.7 | 2 | 810 | 5 |
| 100 | 25 / 8 | 962.5 | 1098.1 | +841.7 | 2 | 95 | 3 |
| 200 | 100 / 1 | 964.0 | 556.4 | +287.2 | 2 | 883 | 26 |
| 200 | 100 / 8 | 996.7 | 495.5 | +226.4 | 4 | 126 | 169 |
| 200 | 50 / 1 | 961.1 | 563.0 | +293.9 | 2 | 872 | 26 |
| 200 | 50 / 8 | 968.5 | 527.9 | +258.8 | 4 | 114 | 110 |
| 200 | 25 / 1 | 909.1 | 565.3 | +296.2 | 2 | 818 | 18 |
| 200 | 25 / 8 | 1031.1 | 523.6 | +254.5 | 6 | 114 | 123 |

Increasing the pump limit reduces the count of pump invocations that end with
work still queued because one invocation consumes several messages. It does
not increase actual throughput: messages inside an invocation are still
delivered sequentially. At 200 ms RTT this larger batch creates head-of-line
delay, larger maximum queues, substantially more retry work, and much worse
tail latency.

## Updated conclusions

- All 72 extension and matched-baseline runs succeeded: 720 measured swaps,
  no outbound drops, no retry-queue drops, no lagging/catch-up transitions, and
  no convergence failures.
- `25 / 1` is the fastest localhost result at 139.7 ms within measurement
  noise of `25 / 8` at 139.1 ms. There remains no reason to raise the pump
  limit.
- At 50 ms RTT, `50 / 1` is the best median result. Reducing the unit interval
  to 25 ms provides no further gain because dissemination is now the limit.
- At 100 ms RTT, all six settings cluster within about 3%; unit interval is no
  longer the dominant delay.
- At 200 ms RTT, `25 / 1` has the best median and tail result, but the gain over
  `50 / 1` is modest compared with the localhost gain.
- Pump limit eight is actively harmful at 200 ms RTT. Its worst p95 ranges from
  2.88 to 4.07 seconds versus 1.57 to 1.89 seconds for pump one.
- Faster cadence mostly increases bandwidth rate rather than bytes per swap.
  Relative to matched `500 / 1`, request-body rate increases by about
  1.0–1.88 Mbit/s at 50 ms RTT, 0.83–0.85 Mbit/s at 100 ms RTT, and
  0.23–0.30 Mbit/s at 200 ms RTT.

The best candidate from this experiment is therefore a **pump limit of one**
and a **25–50 ms active unit interval**. `25 / 1` gives the lowest latency
across the full tested range; `50 / 1` is the more conservative bandwidth/CPU
candidate because 25 ms stops helping once RTT reaches roughly 50 ms.

Before changing production defaults, test `25 / 1` and `50 / 1` with at least
30 swaps per repetition under CPU constraint, asymmetric RTT, packet loss, and
real HTTP/TLS transport. That follow-up should include CPU-seconds and actual
interface byte counters.

## Isolated benchmark additions

- `scripts/benchmark_local_aleph_bft_swaps.sh` runs an arbitrary
  `UNIT_INTERVAL_MS:PUMP_LIMIT` matrix and exposes member count, signature
  threshold, swap count, repetitions, nominal RTT values, jitter percentage,
  and output directory as parameters.
- The ignored `benchmark_aleph_bft_serial_swaps` test builds the fresh
  federation, performs warm-up and serial swaps, verifies convergence, and
  emits machine-readable JSON.
- Test-only counters record outbound bridge depth, messages and encoded bytes
  drained, backlogged pump ticks, retries, and drops. They are compiled only
  under `cfg(test)` and do not alter production behavior.
- A test-only deterministic latency model applies sampled request/response RTT
  after warm-up. It is confined to the in-process router transport.
- Serialized signed request-body bytes and aggregate kbit/s are emitted in
  addition to raw SCALE payload bytes.

Raw per-operation and per-run results are written beneath
`target/aleph-bft-swap-benchmarks/`.
