#!/usr/bin/env python3
import json,pathlib,collections
root=pathlib.Path(__file__).resolve().parents[2]
r={}
for f in ['results.json','ui-results.json','suite-results.json','review-results.json']:
 p=root/'audit'/f
 if p.exists():r.update(json.loads(p.read_text()))
fixes={
 'UI-DESK-SPENDKEY':'aec5e70: validate auditor material in state and precheck before fetching the ledger.',
 'UI-DESK-UNAUTH-COPY':'02e7814: both desk ledes explicitly state It has no login.',
 'UI-CONSOLE':'aec5e70: invalid auditor keys cause no known-failing fetch or console error.',
 'API-STATE-SETUP':'02e7814: config_error names every missing variable.',
 'API-431':'3a3ce71: assemble the full HTTP response before write_all; regression checks require complete 431 JSON.',
 'TEST-E2E':'aec5e70: remove spending-key 400 exemption and assert no audit request on refusal.',
 'CLI-PLAN-BAD':'da2c852: use a valid mainnet address in the bad CSV example.',
 'EDGE-BAD-CSV':'da2c852: use a valid mainnet address in the bad CSV example.',
 'DOCS-CLAIMS':'81c193b: correct Cargo package/library names; final pass corrects proof-test count 5 to 7.'}
lines=[]
for line in (root/'AUDIT.md').read_text().splitlines():
 if line.startswith('| ') and line.endswith(' |'):
  cells=line[2:-2].split(' | ')
  if cells[0] in r:
   assert len(cells)==6,(cells[0],len(cells))
   x=r[cells[0]];detail=x['detail'].replace('|','&#124;').replace('\n',' ').replace('`','')
   cells[3]=x['status'];cells[4]=detail+' [evidence]('+x['path']+')';cells[5]=fixes.get(cells[0],'')
   line='| '+' | '.join(cells)+' |'
 lines.append(line)
(root/'AUDIT.md').write_text('\n'.join(lines)+'\n')
print(collections.Counter(r[k]['status'] for k in json.loads((root/'audit/run/row-ids.json').read_text()) if k in r))
