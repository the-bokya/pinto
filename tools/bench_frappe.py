"""Benchmark Frappe's own get_chrome_pdf engine (excludes Python/frappe startup).
Usage: env/bin/python bench_frappe.py <html> <options.json> <runs>
"""

import json
import statistics
import sys
import time

import reference


def main():
    html = open(sys.argv[1], encoding="utf-8").read()
    options = json.load(open(sys.argv[2])).get("options", {})
    runs = int(sys.argv[3]) if len(sys.argv) > 3 else 8

    pdfmod = reference.configure()
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        reference.render_pdf(pdfmod, html, options)
        times.append((time.perf_counter() - start) * 1000)

    times.sort()
    print(
        f"frappe get_chrome_pdf  runs={runs}  "
        f"min={times[0]:.0f}ms  median={statistics.median(times):.0f}ms  "
        f"mean={statistics.mean(times):.0f}ms  max={times[-1]:.0f}ms"
    )


if __name__ == "__main__":
    main()
