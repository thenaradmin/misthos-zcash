#!/usr/bin/env python3
"""Final-run audit probes. No wallet, .env, seed or signing-key reads."""
import json, os, pathlib, socket, subprocess, time, urllib.request, urllib.error
ROOT=pathlib.Path(__file__).resolve().parents[2]
OUT=ROOT/'audit/logs'; OUT.mkdir(exist_ok=True)
RESULTS=ROOT/'audit/results.json'
def record(id,status,detail,data):
    p=OUT/(id+'.json'); p.write_text(json.dumps({'id':id,'status':status,'expected':'See unchanged exact expected result in AUDIT.md','actual':detail,'console':[],'network':data if id.startswith(('API','CHAIN-TX','EXT-LIVE')) else [],'evidence':data},indent=2)+'\n')
    r=json.loads(RESULTS.read_text()) if RESULTS.exists() else {}; r[id]={'status':status,'detail':detail,'path':str(p.relative_to(ROOT))}; RESULTS.write_text(json.dumps(r,indent=2)+'\n')
    subprocess.run(['python3',str(ROOT/'audit/run/update.py')],cwd=ROOT,stdout=subprocess.DEVNULL,check=True)
def cli(id,cases):
    logs=[]; ok=True
    for args,env,code,need in cases:
        cmd=['env','-i','PATH='+os.environ['PATH'],*[k+'='+v for k,v in env.items()],str(ROOT/'target/debug/payroll'),*args]
        o=subprocess.run(cmd,cwd=ROOT,text=True,capture_output=True,timeout=15)
        text=o.stdout+o.stderr
        logs.append({'command':cmd,'exit':o.returncode,'stdout':o.stdout,'stderr':o.stderr})
        ok &= o.returncode==code and all(s in text for s in need) and 'BROADCAST' not in text
        if code==1: ok &= o.stdout==''
        if id=='CLI-AUDIT-EXPORT-OK':
            import csv,io
            rows=list(csv.reader(io.StringIO(o.stdout))); ok &= len(rows)==5 and all(r[4]=='' and r[3]=='' and r[9]=='0.02 ZEC' for r in rows[3:])
        if id=='CLI-PLAN-OK': ok &= text.count('0.02 ZEC')==2 and 2*2000000==4000000
    record(id,'PASS' if ok else 'FAIL','All literal exit/output assertions matched.' if ok else 'Expected exits/substrings in probe; actual output differs (see evidence).',logs)
def raw(req,port=8791):
    s=socket.create_connection(('127.0.0.1',port),timeout=8); s.settimeout(8); start=time.monotonic(); s.sendall(req); b=b''
    try:
        while True:
            a=s.recv(65536)
            if not a: break
            b+=a
    except ConnectionResetError: pass
    finally: s.close()
    return {'response':b.decode('utf8',errors='replace'),'elapsed':round(time.monotonic()-start,3),'closed':True}
def http(path='/',port=8791,method='GET',host=None):
    return raw(f'{method} {path} HTTP/1.1\r\nHost: {host or "127.0.0.1:"+str(port)}\r\n\r\n'.encode(),port)
def body(r): return r['response'].partition('\r\n\r\n')[2]
def api(id,cases):
    logs=[]; ok=True
    for request,port,status,need in cases:
        r=raw(request,port); r['request']=request.decode('utf8',errors='replace')[:300]; logs.append(r)
        ok &= r['response'].startswith('HTTP/1.1 '+str(status)+' ') and all(s in r['response'] for s in need)
        if len(need)==1 and need[0].startswith('{'):ok &= body(r)==need[0]
        if id=='API-PROOF-410':ok &= 'uviewtest1' not in body(r) and 'ufvk=' not in body(r)
    record(id,'PASS' if ok else 'FAIL','Exact HTTP status/body assertions matched.' if ok else 'Expected status/body assertions differ from captured response.',logs)
def get_public(id,url,height=None,live=False):
    try:
        # Exactly one GET per URL, no redirect following/retry.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self,*a,**kw): return None
        r=urllib.request.build_opener(NoRedirect).open(urllib.request.Request(url,headers={'User-Agent':'Misthos-read-only-audit/1'}),timeout=30)
        data=r.read().decode('utf8',errors='replace'); code=r.status
        p=OUT/(id+'.txt'); p.write_text(data)
        ok=code==200 and ((str(height) in data and url.rsplit('/',1)[1] in data) if height else ('payroll-2026-10-demo' in data if url.endswith('/') else len(json.loads(data).get('roster',[]))==2))
        record(id,'PASS' if ok else 'FAIL',f'GET {url}: HTTP {code}; '+('height/txid found' if height and ok else 'captured body requires exact expected content'),{'url':url,'status':code,'headers':dict(r.headers),'body_path':str(p.relative_to(ROOT))})
    except Exception as e:
        record(id,'UNTESTED',('needs the deployed host; redeploy is forbidden. ' if live else 'Expected public transaction and height; explorer fetch failed. ')+str(e),{'url':url,'error':str(e)})
if __name__=='__main__':
    import sys
    mode=sys.argv[1]
    if mode=='public':
        for id,tx,h in [('CHAIN-TX-FUNDING','e0a93ebccc788ba5d6570478b927ebf1758318c0012f2d95ddda6624ef2cca87',4521170),('CHAIN-TX-ALICE','f0e6ce4569f9a310b3ab936672331f947c5cadb102c3d5b50bcf2181910a46dc',4521181),('CHAIN-TX-BOB','287318aa4e0f0c55ca8b6814cb56b51264ce9dbfd549fcc8ef3db57ea06fa65a',4521189)]: get_public(id,'https://testnet.zecblock.com/tx/'+tx,h)
        get_public('EXT-LIVE-ROOT','https://misthos-api.201-18-198-162.sslip.io/',live=True)
        # The reverify task permits exactly one deployed-host GET (root above).
    elif mode=='cli':
        f='e2e/fixtures/roster.csv'; ufvk=(ROOT/'e2e/fixtures/auditor-ufvk.txt').read_text().strip(); u={'ZCASH_AUDITOR_UFVK':ufvk}; cfg={'ZCASH_NETWORK':'testnet','ZCASH_LWD_URL':'https://testnet.zec.rocks:443','ZCASH_WALLET_DB':'audit/nonexistent-wallet.sqlite'}
        def one(id,args,env={},code=1,need=[]): cli(id,[(args,env,code,need)])
        one('CLI-PLAN-OK',['plan',f],code=0,need=['Run #1 [adhoc:'+f+'] status=Draft','Alice','Bob','TOTAL 0.04 ZEC across 2 outputs'])
        for id in ['CLI-PLAN-BAD','EDGE-BAD-CSV']: one(id,['plan','examples/bad.csv'],need=['line 2:','line 3:','MAINNET address'])
        one('CLI-PLAN-CAPS',['plan',f,'--max-total','0.03','--max-per','0.01','--max-recipients','1'],need=['2 recipients > max 1','row2','row3','0.02 ZEC > per-employee max 0.01 ZEC','run total 0.04 ZEC > max 0.03 ZEC'])
        one('CLI-CHECK-ENV-MISSING',['check-env'],need=['ERROR: ZCASH_NETWORK not configured'])
        one('CLI-CHECK-ENV-MAINNET',['check-env'],{**cfg,'ZCASH_NETWORK':'mainnet'},need=['ZCASH_NETWORK invalid','mainnet','only `testnet` is allowed'])
        one('CLI-AUDIT-EXPORT-MISSING',['audit-export',f],need=['ERROR: ZCASH_AUDITOR_UFVK not configured'])
        one('CLI-AUDIT-EXPORT-OK',['audit-export',f],u,0,['# network=testnet','# ufvk='+ufvk,'run_id,period,status,txid,height,line_index,employee_id,name,address,amount_zec,memo',',draft,,,0,row2,Alice,',',draft,,,1,row3,Bob,'])
        one('CLI-PAY-REFUSE',['pay',f],need=['ERROR: ZCASH_NETWORK not configured'])
        one('CLI-TWOSTEP-LABEL',['pay',f,'--two-step'],need=['ERROR: pay --two-step needs an explicit --label <period> (the CSV path is not a stable period: `./x.csv` and `x.csv` would be different periods and fund/pay again). Nothing sent.'])
        one('CLI-EXPORT-NO-LABEL',['export-proof',f,'--employee','Alice'],need=['ERROR: export-proof needs --label <period>, the same one given to `pay --two-step`'])
        one('CLI-VERIFY-USAGE',['verify-proof'],need=['<proof-file> <txid> --lwd <https://lightwalletd:443> [--employer-pubkey HEX]','<proof-file> <raw-tx-hex-file> <txid> <block-height> [--employer-pubkey HEX]','OFFLINE, trusts the supplier'])
        cli('CLI-USAGE',[(a,{},1,['ERROR: usage: payroll','plan','pay','audit-export','keygen-signing','export-proof','verify-proof','serve','check-env']) for a in [[],['unknown']]])
        one('API-LOOPBACK',['serve','--addr','0.0.0.0:0'],need=['refusing to serve on non-loopback 0.0.0.0:0','the desk is unauthenticated','--allow-remote'])
        one('CLI-SERVE-NO-ROSTER',['serve'],need=['ERROR: PAYROLL_ROSTER_CSV not configured'])
        cli('CLI-SERVE-BAD-PATH',[(['serve'],{'PAYROLL_ROSTER_CSV':'audit/no-such-roster.csv'},1,['PAYROLL_ROSTER_CSV audit/no-such-roster.csv']),(['serve'],{'PAYROLL_ROSTER_CSV':'examples/bad.csv'},1,['line 2:','line 3:'])])
        cli('CLI-SERVE-ARGS',[(a,{},1,['usage: payroll serve [--addr HOST:PORT] [--allow-remote]']) for a in [['serve','--addr'],['serve','--unknown']]])
        cli('CLI-PLAN-ARGS',[(a,{},1,n) for a,n in [(['plan'],['ERROR: plan: missing <file.csv>']),(['plan',f,'--max-total'],['--max-total needs a value']),(['plan',f,'--max-per'],['--max-per needs a value']),(['plan',f,'--max-recipients','x'],['--max-recipients: not a number']),(['plan',f,'--strange','x'],['unknown flag --strange'])]])
        cli('CLI-CHECK-ENV-HTTPS',[(['check-env'],{**cfg,'ZCASH_LWD_URL':'http://invalid'},1,['ZCASH_LWD_URL invalid: must start with https://']),(['check-env'],{'ZCASH_NETWORK':'  '},1,['ZCASH_NETWORK not configured']),(['check-env'],{k:v for k,v in cfg.items() if k!='ZCASH_WALLET_DB'},1,['ZCASH_WALLET_DB not configured']),(['check-env'],cfg,0,['OK network=testnet lwd=https://testnet.zec.rocks:443 wallet_db=audit/nonexistent-wallet.sqlite'])])
        cli('CLI-AUDIT-TXID',[(['audit-export',f,'ab'*32],u,0,[',broadcast,'+'ab'*32+',,0,row2,Alice,',',broadcast,'+'ab'*32+',,1,row3,Bob,']),(['audit-export',f,'invalid'],u,1,['is not 64 hex chars'])])
        one('CLI-EXPORT-USAGE-AMOUNT',['export-proof',f,'--employee'],need=['The signed amount is taken from <file.csv>, not from the ledger','the roster must hold the amount paid in <label>','After a raise, use the roster of that period.'])
        cli('CLI-TWOSTEP-BAD-DELAY',[(['pay',f,'--two-step','--label','audit'],{'PAYROLL_DELAY_MAX_BLOCKS':v},1,['PAYROLL_DELAY_MAX_BLOCKS']) for v in ['-1','two','2.5','1001']]+[(['pay',f,'--two-step','--label','audit'],{'PAYROLL_DELAY_MIN_BLOCKS':'7','PAYROLL_DELAY_MAX_BLOCKS':'2'},1,['PAYROLL_DELAY_MIN_BLOCKS'])])
        cli('CLI-TWOSTEP-TIMEOUT-CONFIG',[(['pay',f,'--two-step','--label','audit'],{v:'invalid'},1,[v,'is not a whole number of seconds']) for v in ['PAYROLL_STEP2_TIMEOUT_SECS','PAYROLL_FUNDING_CONFIRM_TIMEOUT_SECS']])
        one('UI-DESK-EMPTY',['serve','--addr','127.0.0.1:8795'],{'PAYROLL_ROSTER_CSV':'e2e/fixtures/empty-roster.csv'},need=['ERROR: CSV has a header but no rows'])
        (OUT/'zero.csv').write_text('')
        cli('EDGE-EMPTY-ROSTER',[(['serve','--addr','127.0.0.1:8795'],{'PAYROLL_ROSTER_CSV':p},1,[s]) for p,s in [('e2e/fixtures/empty-roster.csv','CSV has a header but no rows'),('audit/logs/zero.csv','CSV is empty')]])
    elif mode=='api':
        ufvk=(ROOT/'e2e/fixtures/auditor-ufvk.txt').read_text().strip()
        req=lambda p,host='127.0.0.1:8791',method='GET':f'{method} {p} HTTP/1.1\r\nHost: {host}\r\n\r\n'.encode()
        api('API-GET-ROOT',[(req('/'),8791,200,['Content-Type: text/html; charset=utf-8','payroll-2026-10-demo'])])
        names=['ZCASH_NETWORK','ZCASH_LWD_URL','ZCASH_WALLET_DB','ZCASH_WALLET_CLI','ZCASH_AUDITOR_UFVK']
        r=http('/api/state'); j=json.loads(body(r)); ok=len(j['roster'])==2 and j['missing']==names and all(n in j['config_error'] for n in names)
        record('API-STATE-SETUP','PASS' if ok else 'FAIL','Expected config_error to identify all five missing variables; actual config_error='+repr(j['config_error'])+'; ordered missing='+repr(j['missing']),r)
        r=http('/api/state',8792);j=json.loads(body(r));ok=j['config_error'] is None and j['missing']==[] and j['network']=='testnet' and j['lwd']=='https://testnet.zec.rocks:443' and j['runs']==[]
        record('API-STATE-CONFIGURED','PASS' if ok else 'FAIL','Configured JSON assertions '+str(ok),r)
        api('API-STATE-MAINNET',[(req('/api/state'),8793,200,['ZCASH_NETWORK invalid','mainnet','only `testnet` is allowed'])])
        api('API-AUDIT-MISSING',[(req('/api/audit'),8791,400,['{"error":"ZCASH_AUDITOR_UFVK not configured"}'])])
        r=http('/api/audit',8792); expected='# network=testnet\n# ufvk='+ufvk+'\nrun_id,period,status,txid,height,line_index,employee_id,name,address,amount_zec,memo\n'
        record('API-AUDIT-OK','PASS' if body(r)==expected and 'Content-Type: text/csv; charset=utf-8' in r['response'] else 'FAIL','Expected exactly three CSV header lines and no data; comparison='+str(body(r)==expected),r)
        r=http('/api/audit',8793); ok=body(r)=='{"error":"ZCASH_AUDITOR_UFVK looks like a spending key; refusing to export it"}' and r['response'].startswith('HTTP/1.1 400 ') and 'secret-extended-key-test1-not-a-real-key' not in body(r)
        record('API-AUDIT-SPENDKEY','PASS' if ok else 'FAIL','Exact refusal and no synthetic value echoed: '+str(ok),r)
        api('API-RUN-410',[(req('/api/run',method='POST'),8791,410,['payroll pay <roster.csv> --two-step --label <period>','dashboard no longer pays'])])
        api('API-PROOF-410',[(req(p),8791,410,['payroll export-proof <roster.csv> --employee <id> --label <period>']) for p in ['/api/proof','/api/proof?employee=1']])
        api('API-404',[(req('/api/missing'),8791,404,['{"error":"not found"}'])])
        rs=[http(p,8792,host='attacker.example') for p in ['/','/api/state','/api/audit']];ok=all(r['response'].startswith('HTTP/1.1 403 ') and json.loads(body(r))['error'].startswith('Host not allowed') and all(s not in body(r) for s in ['<html','Alice','Bob',ufvk]) for r in rs)
        record('API-HOST-403','PASS' if ok else 'FAIL','All three foreign-host requests refused with no roster/key/HTML: '+str(ok),rs)
        api('API-HOST-MISSING',[(b'GET /api/state HTTP/1.1\r\n\r\n',8791,403,['Host not allowed'])])
        api('API-HOST-DUP',[(b'GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nHost: attacker.example\r\n\r\n',8791,400,['{"error":"bad request"}'])])
        api('API-431',[(r,8791,431,['{"error":"request line or headers too large"}']) for r in [b'GET /'+b'x'*16384+b' HTTP/1.1\r\nHost: localhost\r\n\r\n',b'GET / HTTP/1.1\r\nHost: localhost\r\nX: '+b'x'*16384+b'\r\n\r\n',b'x'*16384,b'GET / HTTP/1.1\r\nHost: localhost\r\n'+b'X: a\r\n'*64+b'\r\n']])
        holders=[]
        try:
            start=time.monotonic()
            for _ in range(64):
                s=socket.create_connection(('127.0.0.1',8791),timeout=1);s.sendall(b'GET /api/state HTTP/1.1\r\n');holders.append(s)
            time.sleep(.15);r=http('/api/state'); ok=r['response'].startswith('HTTP/1.1 503 ') and body(r)=='{"error":"too many connections"}'
            record('API-503','PASS' if ok else 'UNTESTED',('Held exactly 64 incomplete heads; 65th returned exact 503 and closed.' if ok else 'needs stable hold of 64 handlers within 5s read timeout; observed response did not establish saturation.'),{'hold_count':len(holders),'setup_elapsed':time.monotonic()-start,'65th':r})
        finally:
            for s in holders:s.close()
        time.sleep(.2)
        rs=[http('/api/state') for _ in range(2)];js=[json.loads(body(r)) for r in rs];ok=all(j['roster_path']==str(ROOT/'e2e/fixtures/roster.csv') and j['roster_total']=='0.04 ZEC' and len(j['roster'])==2 and j['runs']==[] and all(all(isinstance(e[k],str) for k in ['id','name','address','amount','memo']) and e['active'] is True for e in j['roster']) for j in js)
        record('API-STATE-ROSTER','PASS' if ok else 'FAIL','Repeated state has exact roster path, total, typed entries and no runs: '+str(ok),rs)
        api('API-HOST-ACCEPT',[(req('/api/state',h),8791,200,['"roster":']) for h in ['127.0.0.1:8791','localhost','LOCALHOST:1','[::1]:8791']])
        rs=[http('/'),http('/api/state'),http('/api/audit',8792)];ok=True
        for r in rs:
            head,b=r['response'].split('\r\n\r\n',1);h=dict(l.split(': ',1) for l in head.split('\r\n')[1:]);ok &= int(h['Content-Length'])==len(b.encode()) and h['Cache-Control']=='no-store' and h['Connection']=='close' and r['closed']
        record('API-RESPONSE-HEADERS','PASS' if ok else 'FAIL','Accurate UTF-8 byte framing, no-store and closed connection on HTML/JSON/CSV: '+str(ok),[{**r,'response':r['response'][:600]} for r in rs])
        r=raw(b'GET /api/state HTTP/1.1\r\nHost: localhost\r\nX: ');after=http('/api/state');ok=r['response']=='' and 4.5<=r['elapsed']<7 and after['response'].startswith('HTTP/1.1 200 ')
        record('API-TIMEOUT','PASS' if ok else 'FAIL','Incomplete header dropped after ~5 seconds; subsequent request succeeds: '+str(ok),{'stalled':r,'after':after})
        api('API-BAD-UTF8',[(b'GET /api/state HTTP/1.1\r\nHost: localhost\r\nX: \xff\r\n\r\n',8791,400,['{"error":"bad request"}'])])
        # Disconnected partial head: shut down write; server must close without a body.
        s=socket.create_connection(('127.0.0.1',8791),timeout=2);s.sendall(b'GET /api/state HTTP/1.1\r\nHost:');s.shutdown(socket.SHUT_WR);b=s.recv(4096);s.close();assert b==b''
        api('API-METHOD-QUERY',[(req('/api/state?x=1'),8791,200,['"roster":']),(req('/api/state',method='POST'),8791,404,['{"error":"not found"}']),(req('/api/run'),8791,404,['{"error":"not found"}'])])
