#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Export fixed seed102/two-robot truth; preserve original network arrivals."""
import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

HERE=Path(__file__).resolve().parent
ROOT=HERE.parents[4]
binary=Path(sys.argv[1]).resolve()
out=HERE/'truth'
out.mkdir(exist_ok=False)
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
paths=sorted((ROOT/'crates').rglob('*.rs'))+sorted((ROOT/'crates').rglob('Cargo.toml'))+[ROOT/'Cargo.toml',ROOT/'Cargo.lock',HERE/'run.py',HERE/'analyze.py']
source_hashes={str(p.relative_to(ROOT)):sha(p) for p in paths}
binary_hash=sha(binary)
started=time.monotonic()
command=[str(binary),'--ignored','--exact','section_topics::delivery::trials::tuning::t3::projectile_truth','--nocapture']
p=subprocess.Popen(command,cwd=ROOT,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
chunks=[];batch=[];log=[];source_hash=None;count=0

def flush():
    name=f'poses-{len(chunks):03}.jsonl.gz'
    (out/name).write_bytes(gzip.compress(''.join(batch).encode(),mtime=0))
    chunks.append({'name':name,'sha256':sha(out/name)})
    batch.clear()

for line in p.stdout:
    if line.startswith('RM_PROJECTILE_TRUTH '):
        raw=line.removeprefix('RM_PROJECTILE_TRUTH ')
        row=json.loads(raw);count+=1
        assert row[0]==count and row[1]==count*16000000
        batch.append(raw)
        if len(batch)==128:flush()
    elif line.startswith('RM_PROJECTILE_SOURCE_HASH '):source_hash=line.split()[1]
    else:log.append(line)
if batch:flush()
code=p.wait()
assert code==0 and count==4376 and 'test result: ok.' in ''.join(log)
assert sha(binary)==binary_hash and all(sha(ROOT/n)==h for n,h in source_hashes.items())
for v in ['drr-128-211','completion-128-211']:
    old=HERE.parent/'t3-replication/screen'/f'102-2-limited-{v}.json.gz'
    assert json.loads(gzip.decompress(old.read_bytes()))['captured_stream_sha256']==source_hash
(out/'output.txt').write_text(''.join(log).strip()+'\n')
(out/'metadata.json').write_text(json.dumps({'complete':True,'scope':'Truth export only; no network rerun','seed_id':102,'players':2,'captures':count,'captured_stream_sha256':source_hash,'binary_sha256':binary_hash,'source_sha256':source_hashes,'chunks':chunks,'wall_seconds':time.monotonic()-started,'command':command},indent=2)+'\n')
print('Exported and hash-matched',count,'captures in',time.monotonic()-started,'seconds')
