"""Large first-pass scans must fit in memory independently of repository size."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tarfile
import zipfile


def limit_memory():
    import resource
    resource.setrlimit(resource.RLIMIT_AS, (128 * 1024**2, 128 * 1024**2))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


if sys.platform != "linux":
    print("Memory limit checks require Linux")
    sys.exit(0)

binary = str(Path(sys.argv[1]).resolve())
rows = 24576
cases = [
    ("json", '"' + "x" * 509 + '"\n', "JSON", "code"),
    ("py", "#" + "x" * 510 + "\n", "Python", "comment"),
    ("rs", "//" + "x" * 509 + "\n", "Rust", "comment"),
]
failures = []
for ext, line, language, category in cases:
    with tempfile.TemporaryDirectory(prefix="bounded-source-") as tmp:
        root = Path(tmp)
        block = (line * 1024).encode()
        for index in range(12):
            with (root / f"source-{index}.{ext}").open("wb") as stream:
                for _ in range(rows // 1024):
                    stream.write(block)
                stream.write((line.replace("x", str(index % 10), 1)).encode())
                stream.flush()
                os.fsync(stream.fileno())
                os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        sources = list(root.iterdir())
        variants = [(root, []), (root, ["--processes=2"])]
        if ext == "json":
            for suffix in ["tar", "zip"]:
                archive = root / f"sources.{suffix}"
                if suffix == "tar":
                    with tarfile.open(archive, "w") as stream:
                        for source in sources:
                            stream.add(source, arcname=source.name)
                else:
                    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as stream:
                        for source in sources:
                            stream.write(source, arcname=source.name)
                variants.append((archive, []))
        for target, options in variants:
            for source in root.iterdir():
                with source.open("rb") as stream:
                    os.fsync(stream.fileno())
                    os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
            # skip-uniqueness includes all 12 files, including two identical ones.
            result = subprocess.run(
                [binary, str(target), "--json", "--quiet", "--skip-uniqueness", *options],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, preexec_fn=limit_memory,
                timeout=120,
            )
            label = f"{language} {target.suffix or 'directory'} {' '.join(options)}"
            if result.returncode != 0:
                failures.append((label, result.returncode, result.stderr.decode()))
                print(f"FAIL: {label}: {result.stderr.decode().strip()}")
                continue
            counts = json.loads(result.stdout)[language]
            assert counts[category] == 12 * (rows + 1), counts
            assert counts["nFiles"] == 12, counts
            print(f"PASS: 144 MiB {label} within 128 MiB address space")
assert not failures, failures
