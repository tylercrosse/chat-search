#!/usr/bin/env python3
"""Benchmark every runtime variant against the same index.

Splits each run into two numbers:
  total   — wall clock for the whole process (what a subprocess-seam client pays)
  query   — the impl's own reported ms for db-open + query
  startup — total - query, i.e. runtime boot cost

`startup` decides whether a subprocess CLI can back type-ahead search, or whether a
long-lived daemon is required. `query` is the floor a daemon would reach.
"""
import json
import os
import statistics
import subprocess
import sys
import time

S = sys.argv[1]
NODE = open(f"{S}/nodepath").read().strip()
BUN = os.path.expanduser("~/.bun/bin/bun")
RS = "/Users/tylercrosse/dev/projects/chat-search/poc/rust/target/release/csp"
TS_SRC = "/Users/tylercrosse/dev/projects/chat-search/poc/ts/src/main.ts"

VARIANTS = [
    ("node + .ts (strip)", [NODE, "--disable-warning=ExperimentalWarning", TS_SRC], f"{S}/ts.db"),
    ("node + bundle.mjs",  [NODE, "--disable-warning=ExperimentalWarning", f"{S}/main.bundle.mjs"], f"{S}/ts.db"),
    ("bun + bundle.js",    [BUN, f"{S}/main.bun.js"], f"{S}/ts.db"),
    ("bun --compile",      [f"{S}/csp-bun"], f"{S}/ts.db"),
    ("rust --release",     [RS], f"{S}/rs.db"),
]

QUERIES = [
    "sqlite full text search", "rust borrow checker", "tailwind config",
    "docker compose postgres", "react useEffect dependency", "bm25 ranking",
    "authentication token refresh", "kubernetes ingress",
]
RUNS = 20
WARMUP = 3


def run(cmd):
    t0 = time.perf_counter()
    p = subprocess.run(cmd, capture_output=True, text=True)
    total = (time.perf_counter() - t0) * 1000
    if p.returncode != 0:
        raise SystemExit(f"FAILED {' '.join(cmd)}\n{p.stderr[:400]}")
    return total, json.loads(p.stdout)


def bench(label, base, db):
    for i in range(WARMUP):
        run(base + ["search", QUERIES[i % len(QUERIES)], "--db", db, "--limit", "10"])
    totals, queries, hits = [], [], []
    for i in range(RUNS):
        q = QUERIES[i % len(QUERIES)]
        total, out = run(base + ["search", q, "--db", db, "--limit", "10"])
        totals.append(total)
        queries.append(out["ms"])
        hits.append(len(out["results"]))
    pct = lambda xs, p: sorted(xs)[min(len(xs) - 1, int(len(xs) * p / 100))]
    return {
        "variant": label,
        "total_p50": round(statistics.median(totals), 1),
        "total_p95": round(pct(totals, 95), 1),
        "query_p50": round(statistics.median(queries), 2),
        "startup_p50": round(statistics.median(totals) - statistics.median(queries), 1),
        "avg_hits": round(statistics.mean(hits), 1),
    }


results = [bench(*v) for v in VARIANTS]
print(json.dumps(results, indent=2))
print()
hdr = f"{'variant':22} {'total p50':>10} {'total p95':>10} {'query p50':>10} {'startup':>9}  type-ahead?"
print(hdr)
print("-" * len(hdr))
for r in results:
    ok = "yes" if r["total_p95"] < 50 else ("marginal" if r["total_p95"] < 100 else "NO")
    print(f"{r['variant']:22} {r['total_p50']:>9.1f}m {r['total_p95']:>9.1f}m "
          f"{r['query_p50']:>9.2f}m {r['startup_p50']:>8.1f}m  {ok}")
