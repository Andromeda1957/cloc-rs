#!/usr/bin/env python3
"""Check the project-focused preset without a Perl reference."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/cloc-rs').resolve())
checks = 0
with tempfile.TemporaryDirectory(prefix='basic-', dir=os.environ.get('TMPDIR')) as tmp:
    root = Path(tmp)

    def write(name, content):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    def run(*args, ok=True):
        global checks
        checks += 1
        p = subprocess.run([binary, '--json', '--quiet', *map(str, args)], cwd=root,
                           capture_output=True, text=True, timeout=20)
        assert (p.returncode == 0) == ok, (args, p.stderr)
        return json.loads(p.stdout) if ok else None

    write('main.py', ''.join(f'x={i}\n' for i in range(1000)))
    write('wrapper.java', 'class Wrapper {}\n')
    write('app.swift', 'let value = 1\n')
    for name, text in [('data.json', '{"x":1}\n'), ('config.yaml', 'x: 1\n'),
                       ('README.md', '# Title\n'), ('settings.toml', 'x=1\n'),
                       ('notes.txt', 'Notes\n'), ('config.xml', '<x/>\n'),
                       ('settings.ini', '[settings]\nx=1\n'),
                       ('Dockerfile', 'FROM alpine\n'), ('Makefile', 'all:\n\techo 1\n')]:
        write(name, text)
    directories = ['node_modules', 'venv', 'vendor', 'target', '.python_libs',
                   'release', 'build', 'dist', '.recovery-tools', '.venv']
    for i, directory in enumerate(directories):
        write(directory + '/dependency.rs', f'fn dependency_{i}() {{}}\n')
    result = run('--basic', '.')
    assert set(result) == {'header', 'Python', 'SUM'}, result
    assert result['SUM'] == {'nFiles': 1, 'blank': 0, 'comment': 0, 'code': 1000}

    result = run('--basic=0', '.')
    assert {'Python', 'Java', 'Swift'} <= result.keys()
    assert result['SUM']['code'] == 1002
    assert not {'JSON', 'YAML', 'Markdown', 'TOML', 'Text', 'XML', 'INI', 'Rust'} & result.keys()

    assert run('--basic', '--include-lang=Java', '.')['SUM']['code'] == 1
    assert run('--basic', '--include-lang=JSON', '.')['SUM']['code'] == 1
    result = run('--basic', '--by-file', '.')
    assert len([k for k in result if k not in ['header', 'SUM']]) == 1
    assert next(v for v in result.values() if isinstance(v, dict) and 'language' in v)['language'] == 'Python'

    java = root / 'java-project'; java.mkdir()
    write('java-project/Main.java', ''.join(f'class Class{i} {{}}\n' for i in range(1000)))
    write('java-project/tiny.py', 'x=1\n')
    assert set(run('--basic', java)) == {'header', 'Java', 'SUM'}
    write('java-project/comments.py', '# only comments\n')
    assert run('--basic', java)['SUM']['code'] == 1000

    for language, extension, line in [('Swift', 'swift', 'let value_{} = 1\n'),
                                      ('C#', 'cs', 'class Type{} {{}}\n'),
                                      ('Dart', 'dart', 'void function_{}() {{}}\n')]:
        directory = root / (extension + '-project')
        write(directory.name + '/main.' + extension, ''.join(line.format(i) for i in range(1000)))
        write(directory.name + '/tiny.py', 'x=1\n')
        assert set(run('--basic', directory)) == {'header', language, 'SUM'}

    write('boundary/main.py', ''.join(f'x={i}\n' for i in range(198)))
    write('boundary/wrapper.java', 'class A {}\nclass B {}\n')
    assert 'Java' in run('--basic', root / 'boundary')  # exactly 1%
    assert 'Java' not in run('--basic=1.01', root / 'boundary')

    comments = write('comments-only/source.py', '# only comments\n')
    assert run('--basic', comments)['SUM']['comment'] == 1
    assert run('--basic', '--only-count-files', '.')['SUM']['nFiles'] > 0

    counted = root / 'counted.log'
    run('--basic', '--counted', counted, root / 'main.py', root / 'wrapper.java')
    assert 'main.py' in counted.read_text() and 'wrapper.java' not in counted.read_text()

    write('gentle/main.py', ''.join(f'x={i}\n' for i in range(1000)))
    write('gentle/main.c', ''.join(f'int value_{i};\n' for i in range(5)))
    write('gentle/main.sh', ''.join(f'echo value_{i}\n' for i in range(5)))
    write('gentle/main.rs', ''.join(f'fn function_{i}() {{}}\n' for i in range(5)))
    result = run('--basic', root / 'gentle')
    assert {'C', 'Bourne Shell', 'Rust', 'Python'} <= result.keys(), result

    # A language removed on one side stays in the diff if it was substantial
    # across the pair; per-side percentage filtering would hide that deletion.
    write('old/main.py', ''.join(f'x={i}\n' for i in range(1000)))
    write('new/main.py', ''.join(f'x={i}\n' for i in range(1000)))
    write('old/main.java', ''.join(f'class Old{i} {{}}\n' for i in range(500)))
    write('old/wrapper.swift', 'let value = 1\n')
    write('new/wrapper.swift', 'let value = 2\n')
    result = run('--basic', '--diff', root / 'old', root / 'new')
    assert result['SUM']['removed']['code'] == 500, result
    assert 'Java' in result['removed'], result
    assert all('Swift' not in result[k] for k in ['added', 'removed', 'same', 'modified']), result

    for value in ['-1', '101', 'nan', 'no']:
        run('--basic=' + value, '.', ok=False)

print(f'{checks} basic preset checks passed')
