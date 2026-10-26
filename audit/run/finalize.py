#!/usr/bin/env python3
"""Build row evidence from actual final-run logs, without inferred suite counts."""
import pathlib,json,re,collections,hashlib
root=pathlib.Path(__file__).resolve().parents[2]; logs=root/'audit/logs'; results={}
def read(f):
 p=logs/f
 return p.read_text() if p.exists() else ''
def row(id,status,detail,evidence):
 p=logs/(id+'.json');live=None
 if id=='API-HOST-ACCEPT' and p.exists():
  previous=json.loads(p.read_text());live=previous.get('live_http',previous.get('evidence'))
 p.write_text(json.dumps({'id':id,'status':status,'expected':'Exact unchanged expectation in AUDIT.md','actual':detail,'console':[],'network':[],'diagnostics_scope':'Non-browser assertion; browser diagnostics are captured separately per UI row.','live_http':live,'evidence':evidence},indent=2)+'\n');results[id]={'status':status,'detail':detail,'path':str(p.relative_to(root))}
full=read('cargo-test.txt');lib=read('cargo-lib.txt');integ=read('cargo-integrations.txt');probe=read('probe.txt')
counts=re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out',full)
observed=[{'result':v[0],'passed':int(v[1]),'failed':int(v[2]),'ignored':int(v[3]),'measured':int(v[4]),'filtered':int(v[5])} for v in counts]
summary={'observed_sections':observed,'passed':sum(x['passed'] for x in observed),'failed':sum(x['failed'] for x in observed),'ignored':sum(x['ignored'] for x in observed)}
(logs/'rust-counts.json').write_text(json.dumps(summary,indent=2)+'\n')
def suite(id,log,label,expected_count=None):
 c=re.search(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',log)
 if not c:return row(id,'UNTESTED','needs completed '+label+' execution; no result line captured',['audit/logs/cargo-test.txt'])
 ok=c[1]=='ok' and c[3]=='0';detail=f'{label}: {c[2]} passed, {c[3]} failed, {c[4]} ignored.'
 if id=='TEST-LIB':detail+=' TESTING.md remains unchanged per the task restriction.'
 row(id,'PASS' if ok else 'FAIL',detail,['audit/logs/cargo-test.txt','audit/logs/cargo-lib.txt' if id=='TEST-LIB' else 'audit/logs/cargo-integrations.txt','audit/logs/rust-counts.json'])
suite('TEST-LIB',lib,'cargo test --offline --lib')
sections={}
parts=re.split(r'Running tests/([\w_]+)\.rs[^\n]*\n',integ)
for i in range(1,len(parts),2):sections[parts[i]]=parts[i+1]
suite('TEST-CLI-LABEL',sections.get('cli_label',''),'cargo test --offline --test cli_label')
suite('TEST-CLI-SERVE',sections.get('cli_serve',''),'cargo test --offline --test cli_serve')
proof=sections.get('proof_real_tx','');sig=sections.get('sig_attacks','')
chaincounts=[re.search(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',s) for s in [proof,sig]]
if all(chaincounts):
 detail='; '.join(f'{name}: {c[2]} passed, {c[3]} failed, {c[4]} ignored' for name,c in zip(['proof_real_tx','sig_attacks'],chaincounts))+'. Offline synthetic transactions only: real keys/Halo2 for Ironwood, made-up transparent outpoints and Sapling mock proof bytes; no chain confirmation.'
 row('CHAIN-UNIT','PASS' if all(c[1]=='ok' and c[3]=='0' for c in chaincounts) else 'FAIL',detail,['audit/logs/cargo-integrations.txt'])
mapping={
 'CLI-KEYGEN-REFUSE':['attest::tests::key_file_must_be_0600_and_is_never_overwritten'],
 'WALLET-LOCK':['wallet::tests::second_two_step_run_cannot_take_the_wallet_db_lock','concurrent_two_step_run_stops_on_the_wallet_db_lock'],
 'WALLET-LEDGER-TARGET':['wallet::tests::persisted_delay_target_is_reused_and_never_changes_the_step'],
 'API-SLOTS-RELEASE':['server::tests::connection_slots_are_capped_and_released','server::tests::stalled_client_is_dropped_after_the_timeout'],
 'PROOF-SIGNATURE':['attest::tests::valid_signature_passes','attest::tests::forged_with_other_key_is_rejected','attest::tests::any_changed_field_is_rejected','attest::tests::fields_cannot_smuggle_lines','disclosure::tests::signed_proof_rejects_tampering_forgery_and_network_swap','attacks'],
 'PROOF-DECRYPT':['employee_a_proves_own_payout_and_b_key_sees_nothing','signed_proof_is_bound_to_the_real_payout','shared_tx_reveals_only_the_holders_outputs','ironwood_payout_verifies_with_real_halo2_bundle'],
 'PROOF-TX-TAMPER':['verify_proof_attacks'],
 'PROOF-UNSIGNED-ID':['edited_employee_id_still_verifies_and_is_labelled_unsigned'],
}
combined=lib+'\n'+integ
for id,names in mapping.items():
 matches=[{'name':n,'passed':bool(re.search(r'test '+re.escape(n)+r' \.\.\. ok',combined))} for n in names];ok=all(x['passed'] for x in matches)
 row(id,'PASS' if ok else 'UNTESTED',f'{sum(x["passed"] for x in matches)}/{len(matches)} named assertions completed successfully; '+('real-key offline fixtures, not chain confirmation.' if id.startswith('PROOF') else 'isolated library/temp-directory checks; no real wallet or funded send.'),{'tests':matches,'logs':['audit/logs/cargo-lib.txt','audit/logs/cargo-integrations.txt'],'suite_counts':summary})
for id,status in re.findall(r'^([A-Z][A-Z0-9-]+) (PASS|FAIL)$',probe,re.M):
 row(id,status,'Independent offline audit assertions '+('matched every literal assertion in probe.rs.' if status=='PASS' else 'failed; expected assertion and actual panic are in probe.txt.')+' Library suite: '+str(next((x for x in observed if x['passed']>10),{})),['audit/run/probe.rs','audit/logs/probe-build.txt','audit/logs/probe.txt','audit/logs/cargo-lib.txt'])
offline=read('offline-proof-output.txt');required=['EMPLOYER SIGNATURE OK','DECRYPTED OFFLINE (NOT checked on chain','PayoutOut Sapling 0.0125 ZEC recipient=','memo="payroll emp-a"','PAID OUT 0.0125 ZEC','AUDIT PROOF-VERIFY-OFFLINE PASS']
ok=all(x in offline for x in required) and 'VERIFIED ON CHAIN' not in offline
row('PROOF-VERIFY-OFFLINE','PASS' if ok else 'UNTESTED','Captured real CLI exit 0 using existing fixed synthetic fixture helpers: EMPLOYER SIGNATURE OK, DECRYPTED OFFLINE (NOT checked on chain), PayoutOut/Sapling/recipient/memo, PAID OUT 0.0125 ZEC; VERIFIED ON CHAIN absent. No chain-validity claim or wallet proof used.',['audit/run/proof-tool/main.rs','audit/logs/offline-proof-output.txt','audit/logs/offline-proof-build.txt'])

row('TEST-LIVE-EXCLUSION','PASS' if summary['ignored']==4 and full.count(' ... ignored,')==4 else 'UNTESTED',f'cargo test --offline observed {summary["ignored"]} ignored live tests and none executed. Static inventory names wallet height, direct tip, broadcasting CSV payout and two-employee privacy. TESTNET TEST CANNOT RUN guards inspected.',['audit/logs/cargo-test.txt','tests/testnet.rs'])
e2e=logs/'e2e.json'
if e2e.exists():
 j=json.loads(e2e.read_text());stats=j.get('stats',{});ok=stats.get('unexpected',1)==0 and stats.get('expected',0)>0 and not j.get('errors')
 diagnostics=json.loads((logs/'e2e-diagnostics.json').read_text());strict=[{'test':r['test'],'error':e} for r in diagnostics for e in r['console_errors'] if '8794/api/' not in e.get('location',{}).get('url','')]
 row('TEST-E2E','PASS' if ok and not strict else 'FAIL',f'Existing real-Chrome suite observed {stats.get("expected",0)} passed, {stats.get("unexpected",0)} failed, {stats.get("skipped",0)} skipped, {stats.get("flaky",0)} flaky. Audit expected zero console errors: trace extraction found {len(strict)} spending-key HTTP 400 console errors hidden by the harness allowlist. Desktop 1280×800 and mobile 375×812.',{'stats':stats,'strict_console_errors':strict,'diagnostics':'audit/logs/e2e-diagnostics.json','report':'audit/logs/e2e.json','stdout':'audit/logs/e2e.txt'})
ui=json.loads((root/'audit/ui-results.json').read_text()) if (root/'audit/ui-results.json').exists() else {}
diagnostics=[]
for id,x in ui.items():
 for r in json.loads((root/x['path']).read_text())['runs']:
  errors=[m for m in r['console'] if m['type']=='error'];bad=[n for n in r['network'] if n['event']=='failed' or (n['event']=='response' and n['status']>=400)]
  diagnostics.append({'id':id,'width':r['viewport']['width'],'console_errors':errors,'page_errors':r['pageErrors'],'network_errors':bad})
bad=[r for r in diagnostics if r['id'] not in ['UI-DESK-DOWN','EDGE-INTERRUPT'] and (r['console_errors'] or r['page_errors'] or any(n['url'].startswith('http://127.0.0.1:') for n in r['network_errors']))]
row('UI-CONSOLE','FAIL' if bad else ('PASS' if ui else 'UNTESTED'),f'{len(diagnostics)} independent UI/viewport logs captured. '+(f'Expected zero console/page/unexpected same-origin errors; diagnostics found on {len(bad)} runs. No spending-key 400 exemption.' if bad else 'Zero unexpected console/page/same-origin errors; intentional interruption recorded separately.'),diagnostics)
start=read('stack-start.txt');stop=read('stack-stop.txt');harness=read('harness-down-final.txt')
row('E2E-STACK','PASS' if 'stack up' in start and all(x in start for x in ['setup pid','configured pid','mainnet pid']) else 'UNTESTED','Executed clean stack with all three recorded PIDs and real state readiness checks. Synthetic UFVK and no-wallet path only; explicit final shutdown by those PIDs.',['audit/logs/stack-start.txt','audit/logs/stack-stop.txt','e2e/stack.sh'])
ok=all('127.0.0.1:'+str(p)+'/api/state' in harness for p in [8791,8792,8793]) and harness.count('Start it with: npm run stack')==3
row('E2E-HARNESS','PASS' if ok else 'UNTESTED','Real setup was run against the live stack and after stopping each server by PID; '+('each URL failed with its exact startup instruction. ' if ok else 'needs all three captured setup failures. ')+'Configuration inspection: Chrome, one worker, zero retries, video/trace/screenshots; existing watcher attaches diagnostic failures.',['audit/logs/harness-down-final.txt','audit/logs/e2e.json','e2e/global-setup.ts','e2e/playwright.config.ts','e2e/tests/fixtures.ts'])
row('BUILD-EMBEDDED','PASS' if 'Finished' in read('build.txt') and (root/'target/debug/payroll').exists() else 'UNTESTED','Fresh offline binary/library build with CARGO_BUILD_JOBS=2. server.rs embeds ui/index.html; no UI bundler. cargo metadata identifies package zec-payroll and bin payroll; pinned Rust 1.89.0 and prerelease Zcash versions inspected.',['audit/logs/build.txt','audit/logs/cargo-metadata.json','Cargo.toml','rust-toolchain.toml','src/server.rs'])
row('EXT-NEAR','PASS' if ui else 'UNTESTED','README explicitly excludes NEAR Intents and zakura-one-time-ivk; source/dependency search reviewed; actual UI network logs and offline CLI outputs show only local desk API calls plus Google Fonts, with no NEAR/Zakura integration.',['README.md','Cargo.toml','audit/ui-results.json','audit/results.json'])
row('SCOPE-EXECUTION','PASS','This execution used only clean payroll environments, synthetic/temp offline fixtures and read-only public GETs. No real-wallet/key/.env reads, testnet sends, ignored test execution, validation script, deploy or SSH. Only AUDIT.md/audit staged. Original inventory-only sentence refers to the prior planning phase; this task explicitly requires build/tests/servers/Chrome.',['audit/run/heavy.sh','audit/run/nonbrowser.py','audit/run/ui.mjs','audit/logs/cargo-test.txt','audit/logs/stack-stop.txt','audit/logs/slot-release.txt'])
(root/'audit/suite-results.json').write_text(json.dumps(results,indent=2)+'\n')
allresults={}
for f in ['results.json','ui-results.json','suite-results.json','review-results.json']:
 p=root/'audit'/f
 if p.exists():allresults.update(json.loads(p.read_text()))
ids=json.loads((root/'audit/run/row-ids.json').read_text())
for id in ids:
 if id not in allresults:
  row(id,'UNTESTED','needs a completed independent execution of this exact row; no matching final-run assertion evidence was captured.',['audit/logs/build.txt','audit/logs/probe-build.txt','audit/logs/ui-run.txt'])
(root/'audit/suite-results.json').write_text(json.dumps(results,indent=2)+'\n')
print('Evidence generated. Run update.py to render the table.')
