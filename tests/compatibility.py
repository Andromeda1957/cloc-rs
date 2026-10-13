#!/usr/bin/env python3
"""Black-box conformance checks using independently authored source snippets.

No upstream source or tests are copied. The reference executable is supplied
externally and must be cloc 2.10. All temporary files live below TMPDIR.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile
import tarfile
import shutil

PARSER = argparse.ArgumentParser()
PARSER.add_argument('--binary', default='target/debug/cloc-rs')
PARSER.add_argument('--reference', required=True)
PARSER.add_argument('--catalog', action='store_true', help='exercise each reference language')
ARGS = PARSER.parse_args()
BINARY = str(Path(ARGS.binary).resolve())
REFERENCE = ['perl',str(Path(ARGS.reference).resolve())] if ARGS.reference.endswith('.pl') else [ARGS.reference]
CHECKS = 0
FAILURES = []

def run(command, args, cwd, stdin=None):
    p = subprocess.run(command + ['--json', '--quiet', '--hide-rate'] + list(args),
                       cwd=cwd, input=stdin, text=True, capture_output=True, timeout=30)
    if p.returncode:
        raise AssertionError(f'{command}: {p.stderr}\n{p.stdout}')
    data = json.loads(p.stdout or '{}')
    data.pop('header', None)
    return data

def check(label, args, cwd, stdin=None):
    global CHECKS
    CHECKS += 1
    actual = run([BINARY], args, cwd, stdin)
    expected = run(REFERENCE, args, cwd, stdin)
    # Empty input has no language entries; only header presentation differs.
    if not expected and actual.get('SUM', {}).get('nFiles') == 0:
        actual = {}
    if actual != expected:
        FAILURES.append((label, expected, actual))
        changed={k:{'expected':expected.get(k),'actual':actual.get(k)} for k in expected.keys()|actual.keys() if expected.get(k)!=actual.get(k)}
        print(f'FAIL {label}: {changed}')

SNIPPETS = {
 'c.c': '// heading\n\nint main(void) { /* inline */\n  /* multiple\n     lines */\n  return 0;\n}\n',
 'cpp.cpp': '#include <iostream>\n// heading\n/* block */\nint value = 1;\n',
 'header.h': '#pragma once\n// declaration\nint value;\n',
 'rust.rs': '// heading\n/* outer\n /* nested */\n end */\nfn main() {\n let url = "https://example.org";\n}\n',
 'python.py': '# heading\n\n"""documentation\nmore\n"""\nx = "# string"\nprint(x) # inline\n',
 'shell.sh': '#!/bin/sh\n# heading\necho "# value"\n\n',
 'js.js': '// heading\nconst x = 1; /* inline */\n/* multi\nline */\n',
 'typescript.ts': '// heading\ninterface Value {\n value: string;\n}\n',
 'java.java': '// heading\n/* docs */\nclass Example {\n String s = "/* string */";\n}\n',
 'ruby.rb': '# heading\n=begin\ndocumentation\n=end\nputs "hello"\n',
 'perl.pl': '# heading\n=pod\nDocumentation\n=cut\nprint "hello";\n',
 'go.go': 'package example\n// heading\n/* block */\nfunc value() int { return 1 }\n',
 'swift.swift': '// heading\n/* block */\nlet value = 1\n',
 'kotlin.kt': '// heading\n/* block */\nval value = 1\n',
 'csharp.cs': '// heading\n/* block */\nclass Example {}\n',
 'html.html': '<!-- heading -->\n<html>\n\n<body>Hello</body>\n</html>\n',
 'xml.xml': '<?xml version="1.0"?>\n<!-- heading -->\n<value>1</value>\n',
 'css.css': '/* heading */\n.example {\n color: red; /* inline */\n}\n',
 'sql.sql': '-- heading\n/* multi\nline */\nSELECT 1;\n',
 'yaml.yml': '# heading\nvalue: 1\n\n',
 'json.json': '{\n "value": 1\n}\n',
 'toml.toml': '# heading\nvalue = 1\n',
 'make.mk': '# heading\nall:\n\techo hello\n',
 'cmake.cmake': '# heading\nset(VALUE 1)\n',
 'lua.lua': '-- heading\n--[[ multi\nline ]]\nlocal value = 1\n',
 'haskell.hs': '-- heading\n{- multi\nline -}\nvalue = 1\n',
 'r.R': '# heading\nvalue <- 1\n',
 'scala.scala': '// heading\n/* block */\nval value = 1\n',
 'erlang.erl': '% heading\nvalue() -> 1.\n',
 'elixir.ex': '# heading\nvalue = 1\n',
 'pascal.pas': '// heading\n{ block }\nbegin\nend.\n',
 'fortran.f90': '! heading\nprogram example\nend program example\n',
 'legacy.f': 'C Heading\n      PROGRAM EXAMPLE\n      END\n',
 'ocaml.ml': '(* heading *)\nlet value = 1\n',
 'dart.dart': '// heading\n/* block */\nvoid main() {}\n',
 'zig.zig': '// heading\nconst value = 1;\n',
 'nix.nix': '# heading\n{ value = 1; }\n',
 'lisp.lisp': '; heading\n(+ 1 2)\n',
 'scheme.scm': '; heading\n(+ 1 2)\n',
 'vb.vb': "' heading\nDim value As Integer\n",
 'batch.bat': '@echo off\nREM heading\necho hello\n',
 'php.php': '<?php\n// heading\n/* block */\necho "hello";\n',
 'markdown.md': '# Title\n\n<!-- comment -->\nSome prose.\n',
 'stata.do': '// heading\n/* block */\nscalar value = 1\n',
 'message.mc': '/* heading */\nMessageId=1\n',
 'literate.Rmd': 'Prose.\n\n```{r}\n# heading\nx <- 1\n```\n',
 'literate.lidr': 'Prose.\n\n> value = 1\n',
 'docker/Dockerfile': '# heading\nFROM alpine\nRUN echo hello\n',
 'build/Makefile': '# heading\nall:\n\techo hello\n',
 'build/CMakeLists.txt': '# heading\nproject(example)\n',
}

with tempfile.TemporaryDirectory(prefix='compat-', dir=os.environ.get('TMPDIR')) as directory:
    root = Path(directory)
    fixture = root/'fixture'
    fixture.mkdir()
    for name, text in SNIPPETS.items():
        p = fixture/name
        p.parent.mkdir(parents=True, exist_ok=True)
        # Use distinct identifiers to keep unrelated dialect samples unique.
        import re
        text=text.replace('heading','heading_'+name.replace('/','_').replace('.','_'))
        unique=re.sub(r'\bvalue\b', 'value_'+p.suffix.lstrip('.').replace('-','_'), text, flags=re.IGNORECASE)
        p.write_text(unique)
        check('language '+name, [str(p)], root)
    notebook=root/'notebook.ipynb'
    notebook.write_text(json.dumps({'cells':[{'cell_type':'markdown','source':['# Title\n','Some prose.\n'],'metadata':{}},{'cell_type':'code','source':['# heading\n','x=1\n','\n','print(x)\n'],'metadata':{},'outputs':[],'execution_count':None}],'metadata':{'kernelspec':{'language':'python','name':'python3'}},'nbformat':4,'nbformat_minor':5},indent=2))
    check('notebook',[str(notebook)],root)
    check('aggregate', [str(fixture)], root)
    for opts in [
        ['--by-file'], ['--include-lang=Python,Rust,C'],
        ['--exclude-lang=Python'], ['--include-ext=py,rs'], ['--exclude-ext=py'],
        ['--exclude-dir=build,docker'], ['--match-f=^python'], ['--not-match-f=^python'],
        ['--not-match-d=build'], ['--match-d=build'], ['--no-recurse'],
        ['--docstring-as-code'], ['--processes=2'], ['--force-lang=Python,rb'],
        ['--ignore-regex=Python|^print'], ['--skip-leading=1,py'], ['--only-count-files'],
        ['--include-content=heading'], ['--exclude-content=heading'], ['--fullpath','--match-f=build/'],
    ]:
        check('option '+' '.join(opts),opts+[str(fixture)],root)
    duplicate = root/'duplicates'
    duplicate.mkdir()
    for name in ['one.py','two.py']:
        (duplicate/name).write_text('print(1)\n')
    check('deduplicate',[str(duplicate)],root)
    check('keep duplicates',['--skip-uniqueness',str(duplicate)],root)
    for name,contents in [('empty.py',b''),('binary.py',b'\x00\x01\x02'),('crlf.py',b'# heading\r\nx=1\r\n'),('no-eol.py',b'x=1'),('utf16.py','x=1\n# heading\n'.encode('utf-16'))]:
        p=root/name;p.write_bytes(contents)
        check(name,[str(p)],root)
        if name=='utf16.py':check('unicode',['--unicode',str(p)],root)
    check('stdin',['--stdin-name=source.py','-'],root,stdin='# heading\nx=1\n')
    script=root/'script';script.write_text('#!/usr/bin/env python3\nprint(1)\n')
    check('shebang',[str(script)],root)
    check('lang-no-ext',['--lang-no-ext=Python',str(script)],root)
    for ext in ['jpg','png','pdf','JPG','unknown']:
        media=root/('script.'+ext);media.write_text('#!/usr/bin/env python3\nprint(1)\n')
        check('shebang extension '+ext,[str(media)],root)
        check('forced extension '+ext,['--force-lang=Python,'+ext,str(media)],root)
        check('binary option extension '+ext,['--read-binary-files',str(media)],root)
    for suffix in ['.zip','.tar','.tar.gz','.tar.bz2','.tar.xz']:
        p=root/('fixture'+suffix)
        if suffix=='.zip':
            with zipfile.ZipFile(p,'w')as z:
                for f in fixture.rglob('*'):
                    if f.is_file():z.write(f,f.relative_to(fixture))
        else:
            mode={'.tar':'w','.tar.gz':'w:gz','.tar.bz2':'w:bz2','.tar.xz':'w:xz'}[suffix]
            with tarfile.open(p,mode)as t:t.add(fixture,arcname='fixture')
        check('archive '+suffix,[str(p)],root)
        scan=root/('scan-archive'+suffix.replace('.','-'));scan.mkdir()
        shutil.copyfile(p,scan/p.name)
        (scan/'main.py').write_text('print(19)\n')
        check('directory containing archive '+suffix,[str(scan)],root)
    listing=root/'list.txt';listing.write_text(str(fixture/'python.py')+'\n'+str(fixture/'rust.rs')+'\n')
    check('list',['--list-file='+str(listing)],root)
    exclude=root/'exclude.txt';exclude.write_text(str(fixture/'python.py')+'\n')
    check('exclude-list',['--exclude-list-file='+str(exclude),str(fixture)],root)
    old=root/'old';new=root/'new';old.mkdir();new.mkdir()
    for dirname in [old,new]:
        (dirname/'same.py').write_text('# heading\nprint(1)\n\n')
    (old/'change.py').write_text('# old\nx = 1\n\nprint(x)\n')
    (new/'change.py').write_text('# new\nx = 2\n\nprint(x)\nprint(3)\n')
    (old/'removed.rs').write_text('// removed\nfn old() {}\n')
    (new/'added.c').write_text('// added\nint value;\n')
    check('diff',['--diff',str(old),str(new)],root)
    check('diff files',['--diff',str(old/'change.py'),str(new/'change.py')],root)
    check('diff by file',['--diff','--by-file',str(old/'change.py'),str(new/'change.py')],root)
    (old/'inline.py').write_text('x=1 # old\n\n')
    (new/'inline.py').write_text('x=1 # new\n\n\n')
    check('diff inline and blanks',['--diff',str(old/'inline.py'),str(new/'inline.py')],root)
    (old/'change.py').write_text('VALUE = 1\n')
    (new/'change.py').write_text('value=1\n')
    check('diff normalized',['--diff','--ignore-whitespace','--ignore-case',str(old/'change.py'),str(new/'change.py')],root)
    gitroot=root/'git';gitroot.mkdir()
    subprocess.run(['git','init','-q',str(gitroot)],check=True)
    subprocess.run(['git','-C',str(gitroot),'config','user.name','Conformance'],check=True)
    subprocess.run(['git','-C',str(gitroot),'config','user.email','conformance@example.invalid'],check=True)
    (gitroot/'main.py').write_text('# heading\nx=1\n')
    (gitroot/'same.rs').write_text('// heading\nfn main() {}\n')
    subprocess.run(['git','-C',str(gitroot),'add','.'],check=True)
    subprocess.run(['git','-C',str(gitroot),'commit','-qm','first'],check=True)
    first=subprocess.check_output(['git','-C',str(gitroot),'rev-parse','HEAD'],text=True).strip()
    (gitroot/'main.py').write_text('# heading\nx=2\n')
    subprocess.run(['git','-C',str(gitroot),'commit','-qam','second'],check=True)
    (gitroot/'untracked.py').write_text('print(9)\n')
    check('vcs',['--vcs=git'],gitroot)
    check('revision',['--git',first],gitroot)
    check('git diff',['--git','--diff',first,'HEAD'],gitroot)
    check('git diff all',['--git-diff-all',first,'HEAD'],gitroot)
    subprocess.run(['git','-C',str(gitroot),'mv','main.py','renamed.py'],check=True)
    (gitroot/'renamed.py').write_text('# heading\nx=2\nprint(x)\n')
    subprocess.run(['git','-C',str(gitroot),'commit','-qam','rename'],check=True)
    check('git edited rename',['--git','--diff','HEAD~1','HEAD'],gitroot)
    if ARGS.catalog:
        definitions=json.loads(Path(__file__).resolve().parents[1].joinpath('data/catalog.json').read_text())
        langs=subprocess.check_output(REFERENCE+['--show-lang'],text=True)
        import re
        names={m[1].strip() for line in langs.splitlines() if (m:=re.match(r'^(.+?)\s+\((.*)\)$',line))}
        for d in definitions:
            if d['name'] not in names or d['name'] in ['Pascal/Pawn','Unknown/BitBake','XML-Qt-GTK']:continue
            # One code line, a blank, and independent canonical comment syntax.
            # Forced language isolates the lexer from filename ambiguity.
            syntax=d.get('line',[])
            comment=(syntax[0]+' heading\n') if syntax else ''
            block=d.get('block',[])
            if block:comment+=block[0][0]+' block '+block[0][1]+'\n'
            text='value = 1\n\n'+comment
            p=root/'catalog.src';p.write_text(text)
            check('catalog '+d['name'],['--force-lang='+d['name'],str(p)],root)

print(f'{CHECKS-len(FAILURES)}/{CHECKS} conformance checks passed')
if FAILURES:
    raise SystemExit(1)
