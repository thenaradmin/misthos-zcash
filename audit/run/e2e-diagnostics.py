#!/usr/bin/env python3
"""Preserve only console/network evidence; keep Playwright traces/videos untracked."""
import pathlib,json,zipfile
root=pathlib.Path(__file__).resolve().parents[2]
rows=[]
for trace in sorted((root/'e2e/test-results').glob('**/trace.zip')):
 row={'test':str(trace.parent.relative_to(root/'e2e/test-results')),'console':[],'console_errors':[],'page_errors':[],'network':[]}
 with zipfile.ZipFile(trace) as z:
  for name in z.namelist():
   if not name.endswith(('.trace','.network')):continue
   for line in z.read(name).decode().splitlines():
    v=json.loads(line)
    if v.get('type')=='console':
     row['console'].append(v)
     if v.get('messageType')=='error':row['console_errors'].append(v)
    if v.get('type')=='event' and v.get('method')=='pageError':row['page_errors'].append(v)
    if v.get('type')=='resource-snapshot':
     q=v['snapshot'];row['network'].append({'url':q['request']['url'],'method':q['request']['method'],'status':q['response']['status'],'failure':q.get('_failureText'),'request_headers':q['request'].get('headers',[])})
 rows.append(row)
(root/'audit/logs/e2e-diagnostics.json').write_text(json.dumps(rows,indent=2)+'\n')
print('Extracted diagnostics for',len(rows),'tests')
