#!/usr/bin/env python3
"""Compare fresh reads of generated sources, evicting only our own file pages."""
import argparse
import base64
import json
import os
from pathlib import Path
import resource
import random
import statistics
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", action="append", required=True)
parser.add_argument("--runs", type=int, default=3)
args = parser.parse_args()
if sys.platform != "linux":
    parser.error("This benchmark requires Linux posix_fadvise and wait4")
if args.runs < 1:
    parser.error("--runs must be positive")


def cap_memory():
    resource.setrlimit(resource.RLIMIT_AS, (1024**3, 1024**3))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


with tempfile.TemporaryDirectory(prefix="cold-sources-") as tmp:
    root = Path(tmp)
    rng = random.Random(1957)
    patterns = [
        ("rs", 'let value = "/* {payload} */"; // comment\n/* {payload} */\n'),
        ("py", 'value = "# {payload}"  # comment\n\'\'\'documentation\n{payload}\n\'\'\'\n'),
        ("json", '{{"value": "{payload}"}}\n'),
    ]
    sources = []
    for index in range(64):
        ext, pattern = patterns[index % len(patterns)]
        source = root / f"source-{index:03}.{ext}"
        with source.open("wb") as stream:
            stream.write((f"# file {index}\n" if ext == "py" else f"// file {index}\n").encode())
            while stream.tell() < 1024**2:
                payload = base64.b64encode(rng.randbytes(300)).decode()
                stream.write(pattern.format(payload=payload).encode())
            stream.flush()
            os.fsync(stream.fileno())
        sources.append(source)
    mib = sum(source.stat().st_size for source in sources) / 1024**2
    expected = None
    for name in args.binary:
        binary = str(Path(name).resolve())
        results = []
        for _ in range(args.runs):
            for source in sources:
                with source.open("rb") as stream:
                    os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
            with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
                start = time.monotonic()
                proc = subprocess.Popen(
                    [binary, str(root), "--json", "--quiet", "--skip-uniqueness"],
                    stdout=out, stderr=err, preexec_fn=cap_memory,
                )
                _, status, usage = os.wait4(proc.pid, 0)
                elapsed = time.monotonic() - start
                proc.returncode = os.waitstatus_to_exitcode(status)
                err.seek(0)
                assert proc.returncode == 0, err.read().decode()
                out.seek(0)
                report = json.load(out)
                counts = {key: value for key, value in report.items() if key != "header"}
                if expected is None:
                    expected = counts
                assert counts == expected, "Counts differ between benchmarked binaries"
                results.append((elapsed, usage.ru_maxrss / 1024, usage.ru_inblock / 2048))
        print(json.dumps({
            "binary": binary, "source_mib": round(mib, 2),
            "file_content_cache": "evicted before every run; metadata cache uncontrolled",
            "median_seconds": round(statistics.median(result[0] for result in results), 4),
            "peak_rss_mib": round(max(result[1] for result in results), 2),
            "median_disk_read_mib": round(statistics.median(result[2] for result in results), 2),
            "runs": args.runs,
        }), flush=True)
