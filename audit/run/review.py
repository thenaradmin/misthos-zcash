#!/usr/bin/env python3
"""Record source reviews honestly; inspection alone never produces a PASS."""
import json,pathlib,hashlib,subprocess
root=pathlib.Path(__file__).resolve().parents[2]; out=root/'audit/logs'; results={}
def row(id,status,detail,files,extra=None):
 data={'id':id,'status':status,'expected':'Unchanged exact expectation in AUDIT.md','actual':detail,'console':[],'network':[],'reviewed_files':[{'path':f,'sha256':hashlib.sha256((root/f).read_bytes()).hexdigest()} for f in files],'additional_evidence':extra}
 p=out/(id+'.json');p.write_text(json.dumps(data,indent=2)+'\n');results[id]={'status':status,'detail':detail,'path':str(p.relative_to(root))}
reviews={
 'WALLET-COMMAND':('src/wallet.rs','Reviewed run_in session args and single quicksend receiver JSON. Needs an authorized executable wallet-command observation; no successful send permitted. Existing failure and receiver-JSON unit checks do not observe every argv field.'),
 'WALLET-CHILD-INIT':('src/wallet.rs','Existing temp probe checks initial seed env/birthday and failed balance; it does not assert 0700, later calls removing inherited seed, or all directory semantics. Needs expanded isolated probe; no real wallet permitted.'),
 'PAY-TWOSTEP-PREFLIGHT':('src/main.rs','Reviewed label/duplicate/pending gates, child sync loop and tip before funding. Needs controlled pre-broadcast execution with wallet/chain dependencies; forbidden funded path not executed; code review alone is not PASS.'),
 'PAY-TWOSTEP-RESUME':('src/main.rs','Reviewed Fresh/Funded/Paid selection and pending-before-send ordering, ambiguous outcome refusal and known-unsent retry. Needs controlled payout/resume execution; funded send forbidden.'),
 'PAY-TWOSTEP-CLEAR':('src/main.rs','Reviewed period-specific filter, temporary-file write, sync_all, rename and fatal read/write handling. Private function has no existing runtime fixture; needs isolated executable clear-marker probe.'),
 'PAY-TWOSTEP-WAITS':('src/main.rs','Reviewed all-funding-mined aggregation, max height, saved targets, ten failed tip polls and named employee timeout. Needs controlled funded-run/lightwalletd observations; funded send forbidden.'),
 'CHAIN-CLIENT':('src/chain.rs','Reviewed reversed 32-byte hash, raw transaction bytes, 0/u32::MAX unmined mapping, checked tip conversion and TLS roots/30s endpoint timeout. Needs authorized direct lightwalletd fixture exercising every return mapping; not executed.'),
 'PROOF-EXPORT-LEDGER':('src/main.rs','Reviewed exact selector, child paid_txids for adhoc label, zero/multiple txid rejection and roster-sourced amount. Needs isolated signed export/ledger fixture; no real ledger/key access permitted.'),
 'PROOF-VERIFY-ORDER':('src/main.rs','Reviewed parse, expected key, signature, child and signed-txid checks before HTTPS fetch; fetched raw/mined height and exact amount checks. Needs signed recorded proof files from 2026-10-20 for live path; no wallet replacements allowed.'),
 'SCRIPT-TWO-EMPLOYEES':('scripts/testnet-two-employees.sh','Inspected only: requires employer seed, existing payout A/B, period roster; writes employee proofs, verifies live/cross-key then invokes ignored test. Needs PAYROLL_PAYOUT_A / PAYROLL_PAYOUT_B and period roster; script execution forbidden.'),
 'CONTAINER-IMAGE':('Dockerfile','Inspected rust:1.89-bookworm, locked release payroll build, Debian slim UID 10001, fixture CSV and no wallet/.env copies. Needs image build/runtime verification for PASS beyond source; image build/run excluded for this row.'),
 'CONTAINER-COMPOSE':('docker-compose.yml','Inspected published-host bind with allow-remote, 0.0.0.0 extra_hosts, Traefik TLS/CORS, loopback healthcheck and no chain vars. Needs deployed configuration/runtime inspection beyond the permitted public GETs; SSH/redeploy forbidden.'),
}
for id,(f,note) in reviews.items():row(id,'UNTESTED',note,[f])
tracked=subprocess.check_output(['git','ls-files'],cwd=root,text=True).splitlines();proof_files=[]
for f in tracked:
 if f.startswith(('docs/','examples/')) and pathlib.Path(f).suffix in ['.txt','.proof'] and 'zec-payroll-proof v2' in (root/f).read_text():proof_files.append(f)
row('CHAIN-VERIFY-LIVE','UNTESTED','needs the signed proof files from the 2026-10-20 run; not in the repo; wallet and testnet send are forbidden',['tests/proof_real_tx.rs'],{'tracked_signed_proof_files':proof_files})
state=json.loads((out/'EXT-LIVE-STATE.txt').read_text());rootbody=(out/'EXT-LIVE-ROOT.txt').read_text(); names=['ZCASH_NETWORK','ZCASH_LWD_URL','ZCASH_WALLET_DB','ZCASH_WALLET_CLI','ZCASH_AUDITOR_UFVK']
ok='payroll-2026-10-demo' in rootbody and [r['name'] for r in state['roster']]==['Alice','Bob'] and state['missing']==names and bool(state['config_error']) and state['runs']==[]
row('EXT-LIVE','PASS' if ok else 'FAIL','One read-only GET each: / and /api/state returned 200; recorded payslip, two fixture rows, all five missing vars, Setup required configuration, empty runs. No deploy/SSH.', ['docker-compose.yml'],['audit/logs/EXT-LIVE-ROOT.json','audit/logs/EXT-LIVE-STATE.json'])
search=subprocess.run(['rg','-n','-i','mcp|modelcontextprotocol',*['src','tests','Cargo.toml','e2e/package.json']],cwd=root,text=True,capture_output=True)
row('EXT-MCP','PASS' if search.returncode==1 else 'FAIL','Executed repository integration search: no MCP server/tool-list implementation or dependency found. No feature added.',['Cargo.toml','e2e/package.json'],{'command':search.args,'exit':search.returncode,'stdout':search.stdout,'stderr':search.stderr})
metadata=json.loads((out/'cargo-metadata.json').read_text());pkg=metadata['packages'][0];readme=(root/'README.md').read_text();phrase='repo, crate and CLI stay `zec-private-payroll` / `payroll`'
row('DOCS-CLAIMS','FAIL' if phrase in readme and pkg['name']=='zec-payroll' else 'UNTESTED','Expected current crate/bin identifiers; README identifies repo/crate together as zec-private-payroll / payroll, but executed cargo metadata reports package/crate zec-payroll and bin payroll. Historical test counts remain dated; TESTING.md edits prohibited. No forbidden feature authority inferred.',['README.md','TESTING.md','CLAUDE.md','Cargo.toml'],{'metadata_package':pkg['name'],'metadata_targets':[{'name':t['name'],'kind':t['kind']} for t in pkg['targets']],'readme_excerpt':readme.splitlines()[6]})
(root/'audit/review-results.json').write_text(json.dumps(results,indent=2)+'\n')
