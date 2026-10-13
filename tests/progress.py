#!/usr/bin/env python3
"""Verify that progress arrives while input/counting is still in progress."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/cloc-rs').resolve())


class Running:
    def __init__(self, args, cwd, stdin=False):
        self.process = subprocess.Popen(
            [binary, *args], cwd=cwd, text=True,
            stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.events = queue.Queue()
        self.lines = []

        def read():
            for line in self.process.stderr:
                self.lines.append(line)
                self.events.put(line)
            self.events.put(None)

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def expect(self, prefix, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                line = self.events.get(timeout=max(0.001, deadline - time.monotonic()))
            except queue.Empty:
                break
            if line is None:
                break
            if line.startswith(prefix):
                return line
        raise AssertionError(f'no live {prefix!r} event; stderr={"".join(self.lines)!r}')

    def finish(self):
        assert self.process.wait(timeout=20) == 0
        self.reader.join(timeout=2)
        return json.loads(self.process.stdout.read())

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait()
        self.reader.join(timeout=2)
        for stream in [self.process.stdin, self.process.stdout, self.process.stderr]:
            if stream is not None:
                stream.close()


failures = []
checks = 0
with tempfile.TemporaryDirectory(prefix='progress-', dir=os.environ.get('TMPDIR')) as tmp:
    root = Path(tmp)
    (root / 'a.py').write_text('x=1\n')
    # Keep workers occupied after the first small file finishes, without relying
    # on filesystem speed. The normal per-file timeout bounds this fixture.
    for name in ['b.py', 'z.py']:
        (root / name).write_text(f'# {name}\n' + '# padding for counting\n' * 400_000)

    def startup():
        p = Running(['--json', '--progress-rate=1', '--stdin-name=stdin.py', '-'], root, stdin=True)
        try:
            p.expect('Scanning:', timeout=2)
            assert p.process.poll() is None
            p.process.stdin.write('x=1\n')
            p.process.stdin.close()
            assert p.finish()['SUM']['code'] == 1
        finally:
            p.close()

    def counting(processes):
        p = Running(['--json', '--progress-rate=1', '--timeout=0.25',
                     f'--processes={processes}', '.'], root)
        try:
            p.expect('Classifying:')
            p.expect('Counting: 0 / 3')
            p.expect('Counting: 1 / 3')
            assert p.process.poll() is None, 'counting updates arrived after completion'
            p.finish()
        finally:
            p.close()

    for name, test in [('startup', startup),
                       ('sequential counting', lambda: counting(0)),
                       ('parallel counting', lambda: counting(2))]:
        checks += 1
        try:
            test()
        except AssertionError as e:
            failures.append(f'{name}: {e}')

    for option in ['--quiet', '--progress-rate=0']:
        checks += 1
        p = subprocess.run([binary, '--json', option, str(root / 'a.py')],
                           cwd=root, capture_output=True, text=True, timeout=20)
        assert p.returncode == 0 and p.stderr == '', (option, p.stderr)
        assert json.loads(p.stdout)['SUM']['code'] == 1

    # Real terminal output must reuse one line across every phase and file.
    if os.name != 'nt':
        import errno
        import pty
        master, slave = pty.openpty()
        try:
            p = subprocess.Popen([binary, '--json', '--progress-rate=1', str(root / 'a.py')],
                                 cwd=root, stdout=subprocess.PIPE, stderr=slave)
            # Keep a slave descriptor open until output is drained: macOS can
            # discard unread master output when the final slave closes.
            stdout, _ = p.communicate(timeout=20)
            assert p.returncode == 0
            terminal = bytearray()
            os.set_blocking(master, False)
            while True:
                try:
                    chunk = os.read(master, 4096)
                except BlockingIOError:
                    break
                except OSError as e:
                    if e.errno == errno.EIO:
                        break
                    raise
                if not chunk:
                    break
                terminal.extend(chunk)
            checks += 1
            assert b'\r' in terminal and b'Counting:' in terminal, terminal
            assert terminal.count(b'\n') == 1, terminal
            assert json.loads(stdout)['SUM']['code'] == 1
        finally:
            os.close(master)
            if slave is not None:
                os.close(slave)

assert not failures, '\n'.join(failures)
print(f'{checks} live progress checks passed')
