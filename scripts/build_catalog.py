#!/usr/bin/env python3
"""Combine permissive syntax metadata with public language/extension names.

Use cloc only for its documented --show-lang interface, never source filters.
Usage: python3 scripts/build_catalog.py /path/to/cloc-reference.pl
"""
import collections
import json
import re
import subprocess
import sys

raw = json.load(open('data/languages.json'))['languages']
def clean(s):
    return s.replace('\\"', '"').replace('\\', '')

def normalize(s):
    return re.sub(r'[^a-z0-9+]', '', s.lower())

bases = {}
for key, d in raw.items():
    name = d.get('name', key)
    bases[name] = dict(name=name, extensions=d.get('extensions', []),
        filenames=d.get('filenames', []), env=d.get('env', []),
        line=[clean(s) for s in d.get('line_comment', [])],
        block=[[clean(a), clean(b)] for a,b in d.get('multi_line_comments', [])],
        quotes=[[clean(a), clean(b)] for a,b in d.get('quotes', [])],
        verbatim=[[clean(a), clean(b)] for a,b in d.get('verbatim_quotes', [])],
        doc=[[clean(a), clean(b)] for a,b in d.get('doc_quotes', [])],
        nested=d.get('nested', False))

aliases = {'C/C++ Header':'C Header', 'Bourne Again Shell':'BASH',
 'Bourne Shell':'Bourne Shell', 'make':'Makefile', 'Fortran 77':'FORTRAN Legacy',
 'Fortran 90':'FORTRAN Modern', 'Arduino Sketch':'Arduino C++',
 'ArkTs':'Ark TypeScript', 'ANTLR Grammar':'ANTLR', 'DOS Batch':'Batch',
 'Apex Class':'Apex', 'Lisp':'Lisp', 'C#':'C#', 'Visual Basic':'Visual Basic',
 'Visual Basic .NET':'Visual Basic', 'MSBuild script':'XML', 'Ant':'XML',
 'Maven':'XML', 'XSD':'XML', 'XSLT':'XML', 'XML-Qt-GTK':'XML',
 'SVG':'XML', 'WiX source':'XML', 'WiX include':'XML', 'WiX string localization':'XML',
 'Windows Module Definition':'Module-Definition', 'Jupyter Notebook':'JSON',
 'Jupyter Notebook (Python)':'Python', 'TypeScript (JSX)':'TSX',
 'JavaScript (JSX)':'JSX', 'Objective-C++':'Objective-C++'}
text = subprocess.check_output(['perl',sys.argv[1],'--show-lang'],text=True)
result = {}
for row in text.splitlines():
    m=re.match(r'^(.+?)\s+\((.*)\)$',row)
    if not m: continue
    name=m[1].strip(); extensions=list(dict.fromkeys(m[2].split(', ')))
    base=bases.get(aliases.get(name,name))
    if not base:
        base=next((d for n,d in bases.items() if normalize(n)==normalize(name)),None)
    if not base:
        scores=[(len(set(extensions)&set(d['extensions'])), n, d) for n,d in bases.items()]
        score,_,candidate=max(scores,key=lambda x:x[0])
        if score: base=candidate
    base=dict(base or dict(line=[],block=[],quotes=[],verbatim=[],doc=[],nested=False,env=[],filenames=[]))
    base.update(name=name,extensions=extensions)
    result[name]=base
# Add independently specified syntax for dialects not covered by Tokei.
sets = {
 'hash':['AMPLE','CMake','Dockerfile','Meson','Bourne Shell','Bourne Again Shell','zsh','fish','Nushell','YAML','TOML','R','Raku','GDScript','Elixir','Erlang','Julia','Nix','Starlark','Stata'],
 'c':['Apex Class','Apex Trigger','AspectJ','C','C++','C/C++ Header','C#','CUDA','OpenCL','Java','JavaScript','TypeScript','TypeScript (JSX)','Vuejs Component','Svelte','Dart','D','Haxe','HLSL','GLSL','Objective-C','Objective-C++','Protocol Buffers','Thrift','Zeek','P4','Solidity','Aria','X++','QML','Qbs','Qt Project','IDL','ANTLR Grammar','Pawn','Squirrel'],
 'xml':['Activiti Business Process','Ant','Maven','XSD','XSLT','SVG','XML','XML-Qt-GTK','Windows Resource File','WiX source','WiX include','WiX string localization','MSBuild script','NAnt script'],
 'sql':['SQL','PL/SQL','SQL Stored Procedure','TSQL'],
 'percent':['MATLAB','Octave','Erlang','TeX','Prolog'],
 'semicolon':['Assembly','AutoHotkey','AutoIt','Clojure','Lisp','Scheme','Racket'],
}
for kind,names in sets.items():
 for name in names:
  if name not in result: continue
  d=result[name]
  if kind=='hash': d['line']=['#']
  if kind=='c': d.update(line=['//'],block=[['/*','*/']],quotes=[['"','"'],["'","'"],['`','`']])
  if kind=='xml': d.update(line=[],block=[['<!--','-->']],quotes=[['"','"'],["'","'"]])
  if kind=='sql': d.update(line=['--'],block=[['/*','*/']],quotes=[["'","'"],['"','"']])
  if kind=='percent': d['line']=['%']
  if kind=='semicolon': d['line']=[';']
# Correct case-sensitive and filename-only detection.
for name in ['C/C++ Header']: result[name]['extensions']=['h','H','hh','hpp','hxx','h++','inl','ipp','tpp']
result['Python']['extensions'] += ['pyi']
result['Ruby']['filenames']=['Gemfile','Rakefile','Vagrantfile','Brewfile','Podfile','Guardfile']
result['Dockerfile']['filenames']=['Dockerfile','Containerfile']
result['make']['filenames']=['Makefile','GNUmakefile','makefile']
result['CMake']['filenames']=['CMakeLists.txt']
result['JSON']['extensions'] += ['jsonc']
result['Markdown'].update(block=[['<!--','-->']],line=[])
result['Rust']['quotes']=[['"','"']]
result['Visual Basic'].update(line=["'",'REM '],block=[],quotes=[['"','"']])
if 'Visual Basic .NET' in result: result['Visual Basic .NET'].update(line=["'",'REM '],block=[],quotes=[['"','"']])
result['DOS Batch'].update(line=['REM ', 'rem ', '::'], block=[])
result['ABAP'].update(line=['"','*'],block=[])
result['COBOL'].update(line=['*>'],block=[])
# Preserve Tokei-only language definitions too.
covered={e for d in result.values() for e in d['extensions']}
for name,d in bases.items():
 if name not in result:
  d=dict(d); d['extensions']=[e for e in d['extensions'] if e not in covered]
  if d['extensions']: result[name]=d
for name,values in json.load(open('data/overrides.json')).items():
 if name in result: result[name].update(values)
json.dump(sorted(result.values(),key=lambda d:d['name'].lower()),open('data/catalog.json','w'),indent=2,ensure_ascii=False)
print(f'{len(result)} language definitions')

extensions={}
text=subprocess.check_output(['perl',sys.argv[1],'--show-ext'],text=True)
for row in text.splitlines()[2:]:
 m=re.match(r'^(\S+)\s+(.+?)\s*$',row)
 if m: extensions[m[1]]=m[2]
json.dump(extensions,open('data/extensions.json','w'),indent=2,sort_keys=True)
