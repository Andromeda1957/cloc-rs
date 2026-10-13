#!/usr/bin/env python3
"""Exercise end-to-end CLI outputs and filesystem behavior without a reference."""
import csv
import io
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import tarfile
import zipfile

binary=str(Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/cloc-rs').resolve())
checks=0
with tempfile.TemporaryDirectory(prefix='commands-',dir=os.environ.get('TMPDIR'))as tmp:
    root=Path(tmp)
    source=root/"a&b's.py"
    source.write_text('# heading\n\nx=1 # inline\nprint(x)\n')
    def run(*args,stdin=None,ok=True):
        global checks
        checks+=1
        p=subprocess.run([binary,*map(str,args)],cwd=root,input=stdin,text=True,capture_output=True,timeout=30)
        assert (p.returncode==0)==ok,(args,p.returncode,p.stdout,p.stderr)
        return p.stdout
    def data(*args,**kw):return json.loads(run('--json','--hide-rate',*args,**kw))
    expected={'nFiles':1,'blank':1,'comment':1,'code':2}
    assert data(source)['SUM']==expected
    assert data('--exclude-lang=Text,',source)['SUM']==expected
    assert data('--include-lang=Python,',source)['SUM']==expected
    assert data('--exclude-dir=missing,',source)['SUM']==expected
    assert data('--processes','2',source)['SUM']==expected
    assert data('--by-file-by-lang',source)[str(source)]['language']=='Python'
    for fmt in ['--csv','--csv-delimiter=;']:
        rows=list(csv.reader(io.StringIO(run(fmt,'--hide-rate',source)),delimiter=';' if ';' in fmt else ','))
        assert rows[1][0:5]==['1','Python','1','1','2'],rows
    assert ET.fromstring(run('--xml',source)).find('languages/total').attrib['code']=='2'
    run('--xsl=1',source)
    assert (root/'cloc.xsl').is_file()
    for fmt in range(1,6):
        text=run('--fmt='+str(fmt),'--hide-rate',source)
        assert 'code' in text and ('total' in text)==(fmt in [2,4,5])
    assert 'Python | 1 | 1 | 1 | 2' in run('--md','--hide-rate',source)
    report=root/'report.json'
    run('--json','--out',report,source)
    assert json.loads(report.read_text())['SUM']==expected
    run('--json','--file-encoding=UTF-16','--out',report,source)
    assert json.loads(report.read_text(encoding='utf-16'))['SUM']==expected
    sql=run('--sql=-','--sql-project=project\'s',source)
    connection=sqlite3.connect(':memory:')
    connection.executescript(sql)
    assert connection.execute('select nBlank,nComment,nCode from t').fetchone()==(1,1,2)
    connection.executescript(run('--sql=-','--sql-append',source))
    assert connection.execute('select count(*) from t').fetchone()[0]==2
    run('--strip-comments=code','--original-dir',source)
    assert source.with_name(source.name+'.code').read_text()=='x=1 # inline\nprint(x)\n'
    run('--strip-code=comments','--original-dir',source)
    assert source.with_name(source.name+'.comments').read_text()=='# heading\n'
    run('--strip-code=comments','--original-dir',source,ok=False)
    config=root/'options.txt';config.write_text('yaml\ninclude-lang=Python\n')
    assert data('--config',config,source)['SUM']==expected
    assert data('--skip-leading=1',source)['SUM']==dict(expected,comment=0)
    assert data('--only-count-files',source)['SUM']==dict(expected,code=0,blank=0,comment=0)
    assert 'Python' in run('--show-lang=Python')
    assert 'Python' in run('--show-ext=py')
    assert json.loads(run('--explain=Python'))['line']==['#']
    assert run('--show-os').strip() in ['Windows','UNIX']
    defs=root/'lang.json';run('--write-lang-def',defs)
    assert data('--force-lang-def',defs,source)['SUM']==expected
    custom=root/'custom.def';custom.write_text('Widget\n    extension wid\n    filter remove_inline !\n    filter remove_between_general /* */\n    3rd_gen_scale 1.00\n')
    f=root/'example.wid';f.write_text('! comment\n/* block */\nx=1\n')
    assert data('--read-lang-def',custom,f)['Widget']=={'nFiles':1,'blank':0,'comment':2,'code':1}
    assert data('--force-lang=Python,wid',f)['Python']['code']==3
    noext=root/'program';noext.write_text('#!/usr/bin/env python3\nprint(1)\n')
    assert data(noext)['Python']['code']==2
    assert data('--stdin-name=stdin.py','-',stdin='x=1\n')['Python']['code']==1
    listing=root/'list';listing.write_text(str(source)+'\n')
    assert data('--list-file',listing)['SUM']==expected
    plain1=root/'one.report';plain2=root/'two.report'
    run('--sum-one','--hide-rate','--out',plain1,source)
    run('--sum-one','--hide-rate','--out',plain2,source)
    assert data('--sum-reports',plain1,plain2)['SUM']=={k:v*2 for k,v in expected.items()}
    changed=root/'changed.py';changed.write_text('# heading\nx=2\n')
    assert data('--diff',source,changed)['SUM']['modified']['code']==1
    alignment=root/'alignment';run('--diff','--diff-alignment',alignment,source,changed)
    assert data('--diff-list-file',alignment)['SUM']['modified']['code']==1
    output=root/'counts';run('--count-and-diff','--json','--out',output,source,changed)
    assert all(root.joinpath('counts'+suffix).is_file() for suffix in ['.1','.2','.diff'])
    logs=[root/k for k in ['found','counted','unique','categorized','ignored']]
    args=[a for k,p in zip(['found','counted','unique','categorized','ignored'],logs) for a in ['--'+k,str(p)]]
    run(*args,source)
    assert all(p.is_file()for p in logs)
    assert source.name in logs[0].read_text()
    run('--match-f=[',source,ok=False)
    run('--unknown',source,ok=False)
    run('--max-file-size=nan',source,ok=False)
    run('--processes=-1',source,ok=False)
    run('--force-lang=not-a-language',source,ok=False)
    bad=root/'bad.json';bad.write_text('[{"name":"bad","extensions":["bad"],"block":[["",""]]}]')
    run('--read-lang-def',bad,source,ok=False)
    if os.name!='nt':
        import tarfile
        archive=root/"archive 'quoted'.tar"
        with tarfile.open(archive,'w')as t:t.add(source,arcname='source.py')
        assert data('--extract-with=tar xf >FILE<',archive)['SUM']==expected
        assert not list(root.glob('cloc-rs-*'))
    assert data('--3',source)['Python']['factor']==4.2
    assert '3rd gen. equiv' in run('--3',source)
    assert data('--timeout=0.000000001',source)['SUM']['nFiles']==0
    assert ET.fromstring(run('--diff','--xml',source,changed)).tag=='diff_results'
    assert list(csv.reader(io.StringIO(run('--diff','--csv',source,changed))))[0][1]=='== files'

    for ext in ['jpg','png','pdf']:
        media=root/('script.'+ext);media.write_text('#!/usr/bin/env python3\nprint(1)\n')
        assert data(media)['SUM']['nFiles']==0
        assert data('--force-lang=Python,'+ext,media)['SUM']['code']==2
    unknown=root/'script.unknown';unknown.write_text('#!/usr/bin/env python3\nprint(1)\n')
    assert data(unknown)['SUM']['code']==2

    for suffix in ['zip','tar','tgz']:
        directory=root/('scan-'+suffix);directory.mkdir()
        (directory/'main.py').write_text('x=1\n')
        archive=directory/('archived.'+suffix)
        if suffix=='zip':
            with zipfile.ZipFile(archive,'w')as z:z.writestr('member.py','x=2\nx=3\n')
        else:
            with tarfile.open(archive,'w:gz' if suffix=='tgz' else 'w')as t:
                info=tarfile.TarInfo('member.py');info.size=8
                t.addfile(info,io.BytesIO(b'x=2\nx=3\n'))
        assert data(directory)['SUM']=={'nFiles':1,'blank':0,'comment':0,'code':1}
        assert data(archive)['SUM']=={'nFiles':1,'blank':0,'comment':0,'code':2}

    if sys.platform=='linux':
        import resource
        artifacts=root/'artifacts';artifacts.mkdir()
        (artifacts/'main.py').write_text('x=1\n')
        for i in range(10):
            with open(artifacts/f'artifact-{i}.bin','wb')as f:f.truncate(16*1024*1024)
        def memory_limit():
            resource.setrlimit(resource.RLIMIT_AS,(128*1024*1024,128*1024*1024))
            resource.setrlimit(resource.RLIMIT_CORE,(0,0))
        p=subprocess.run([binary,'--json','--quiet',str(artifacts)],cwd=root,
                         preexec_fn=memory_limit,capture_output=True,text=True,timeout=30)
        checks+=1
        assert p.returncode==0,p.stderr
        assert json.loads(p.stdout)['SUM']=={'nFiles':1,'blank':0,'comment':0,'code':1}
        images=root/'images';images.mkdir()
        first=images/'image.jpg';first.write_bytes(b'\xff\xd8'+b'\0'*4094)
        for i in range(40_000):os.link(first,images/f'image-{i}.jpg')
        p=subprocess.run([binary,'--json','--quiet',str(images)],cwd=root,
                         preexec_fn=memory_limit,capture_output=True,text=True,timeout=30)
        checks+=1
        assert p.returncode==0,p.stderr
        assert json.loads(p.stdout)['SUM']['nFiles']==0
print(f'{checks} CLI checks passed')
