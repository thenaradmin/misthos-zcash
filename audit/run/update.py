#!/usr/bin/env python3
import json,pathlib,re,collections
root=pathlib.Path(__file__).resolve().parents[2]
r={}
for f in ['results.json','ui-results.json','suite-results.json','review-results.json']:
 p=root/'audit'/f
 if p.exists():r.update(json.loads(p.read_text()))
lines=[]
for l in (root/'AUDIT.md').read_text().splitlines():
 if l.startswith('| ') and re.search(r' \| (PENDING|PASS|FAIL|UNTESTED) \|',l):
  id=l.split(' | ',2)[0][2:]
  if id in r:
   x=r[id];detail=x['detail'].replace('|','&#124;').replace('\n',' ').replace('`','')
   l=re.sub(r' \| (PENDING|PASS|FAIL|UNTESTED) \|.*$',lambda m:f' | {x["status"]} | {detail} [evidence]({x["path"]}) | '+('pending' if x['status']=='FAIL' else '')+' |',l)
 lines.append(l)
(root/'AUDIT.md').write_text('\n'.join(lines)+'\n')
print(collections.Counter(v['status'] for k,v in r.items() if k in json.loads((root/'audit/run/row-ids.json').read_text())))
