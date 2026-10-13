"""Check metadata and detailed operations across streaming input order."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

binary = str(Path(sys.argv[1]).resolve())


def run(*args):
    return json.loads(subprocess.check_output([binary, *map(str, args), "--quiet", "--json"]))


with tempfile.TemporaryDirectory(prefix="streaming-") as tmp:
    root = Path(tmp)
    a, z = root / "a.py", root / "z.rs"
    for source in [a, z]:
        source.write_text("value = 1\n")
    assert set(run(z, a)) == {"header", "Python", "SUM"}
    archive = root / "sources.zip"
    with zipfile.ZipFile(archive, "w") as stream:
        stream.write(z, z.name)
        stream.write(a, a.name)
    assert set(run(archive)) == {"header", "Python", "SUM"}
    assert run(z, a, "--processes=2")["Python"]["nFiles"] == 1
    categorized = root / "categorized.txt"
    run(a, f"--categorized={categorized}")
    assert categorized.read_text() == f"9 Python {a}\n"
    # Invalid Unicode with binary reading explicitly requested retains old fallback behavior.
    malformed = root / "malformed.py"
    malformed.write_bytes(b"\xff\xfe\x00\x00\x01")
    assert run(malformed, "--unicode", "--read-binary-files")["SUM"]["nFiles"] == 1
print("5 streaming compatibility checks passed")
