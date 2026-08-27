#!/usr/bin/env python3
"""Summarize the usage receipts under data/api-usage/.

The receipts are the only record of what the two AI stages actually cost, and comparing a
change against "it feels cheaper now" is not a comparison. This prints the same table before
and after so the difference is a number.

    python scripts/api-usage-report.py                 # totals by stage
    python scripts/api-usage-report.py --since 2026-08-27
    python scripts/api-usage-report.py --runs          # one line per run, retries included

Cache hit rate is the headline: cached input bills at a fraction of the normal rate, so a
stage sitting near 0% is one whose constant prompt prefix is not being reused.
"""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import glob
import json
import os
import sys


def load(directory: str) -> list[dict]:
    receipts = []
    for path in glob.glob(os.path.join(directory, "*.json")):
        try:
            with open(path, encoding="utf-8") as handle:
                receipts.append(json.load(handle))
        except (OSError, json.JSONDecodeError) as error:
            print(f"skipping {os.path.basename(path)}: {error}", file=sys.stderr)
    receipts.sort(key=lambda receipt: receipt.get("recorded_at_ms", 0))
    return receipts


def within(receipt: dict, since_ms: int | None) -> bool:
    return since_ms is None or receipt.get("recorded_at_ms", 0) >= since_ms


def stage_table(receipts: list[dict]) -> None:
    totals = collections.defaultdict(lambda: collections.Counter())
    for receipt in receipts:
        usage = receipt.get("usage", {})
        row = totals[receipt.get("stage", "unknown")]
        row["calls"] += 1
        row["input"] += usage.get("input_tokens", 0)
        row["output"] += usage.get("output_tokens", 0)
        row["cached"] += usage.get("cached_input_tokens") or 0
        row["reasoning"] += usage.get("reasoning_output_tokens") or 0

    header = f"{'stage':<20}{'calls':>7}{'input':>10}{'cached':>10}{'hit':>7}{'output':>10}{'reason':>9}"
    print(header)
    print("-" * len(header))
    grand = collections.Counter()
    for stage, row in sorted(totals.items()):
        hit = f"{100 * row['cached'] / row['input']:.0f}%" if row["input"] else "-"
        print(
            f"{stage:<20}{row['calls']:>7}{row['input']:>10}{row['cached']:>10}"
            f"{hit:>7}{row['output']:>10}{row['reasoning']:>9}"
        )
        grand.update(row)
    if totals:
        hit = f"{100 * grand['cached'] / grand['input']:.0f}%" if grand["input"] else "-"
        print("-" * len(header))
        print(
            f"{'total':<20}{grand['calls']:>7}{grand['input']:>10}{grand['cached']:>10}"
            f"{hit:>7}{grand['output']:>10}{grand['reasoning']:>9}"
        )


def run_table(receipts: list[dict]) -> None:
    """One line per capture, so the retry multiplier is visible.

    Receipts written before the capture id was recorded fall back to a single "unknown" row;
    they cannot be attributed to a run after the fact.
    """
    runs = collections.defaultdict(lambda: collections.Counter())
    for receipt in receipts:
        usage = receipt.get("usage", {})
        key = (receipt.get("capture_id"), receipt.get("stage"))
        row = runs[key]
        row["calls"] += 1
        row["input"] += usage.get("input_tokens", 0)
        row["output"] += usage.get("output_tokens", 0)
        row["cached"] += usage.get("cached_input_tokens") or 0
        row["last"] = max(row["last"], receipt.get("recorded_at_ms", 0))

    header = f"{'when':<17}{'capture':>15}  {'stage':<18}{'calls':>7}{'input':>10}{'cached':>10}{'output':>10}"
    print(header)
    print("-" * len(header))
    for (capture_id, stage), row in sorted(runs.items(), key=lambda item: item[1]["last"]):
        when = dt.datetime.fromtimestamp(row["last"] / 1000).strftime("%m-%d %H:%M:%S")
        label = str(capture_id) if capture_id else "unknown"
        print(
            f"{when:<17}{label:>15}  {stage:<18}{row['calls']:>7}"
            f"{row['input']:>10}{row['cached']:>10}{row['output']:>10}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--directory",
        default=os.path.join(os.path.dirname(__file__), "..", "data", "api-usage"),
    )
    parser.add_argument("--since", help="only receipts on or after this date, as YYYY-MM-DD")
    parser.add_argument(
        "--runs",
        action="store_true",
        help="group by capture instead of by stage, to show retries per run",
    )
    args = parser.parse_args()

    since_ms = None
    if args.since:
        since_ms = int(
            dt.datetime.strptime(args.since, "%Y-%m-%d").timestamp() * 1000
        )

    receipts = [r for r in load(args.directory) if within(r, since_ms)]
    if not receipts:
        print("no receipts matched")
        return 0

    if args.runs:
        run_table(receipts)
    else:
        stage_table(receipts)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
