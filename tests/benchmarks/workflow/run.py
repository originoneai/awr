#!/usr/bin/env python3
"""Measure entire equivalent native MCP workflows, including maintenance/recovery."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import os
import queue
import statistics
import subprocess
import sys
import threading
import time

from oracle import CRITERIA, GOAL, RULE, UPGRADE_CRITERION, check_context, check_artifact, check_final, require

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
CONTRACT = json.loads((HERE/'contract.json').read_text())
SHA = 'a' * 40  # Explicit synthetic source binding; not a repository SHA assertion.


def encoded(value):
    return json.dumps(value,ensure_ascii=False,separators=(',',':')).encode()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Recorder:
    def __init__(self, directory):
        self.directory = directory
        directory.mkdir()
        self.rows = []

    def add(self, name, phase, input_bytes, output_bytes, elapsed_ms, **extra):
        index = len(self.rows)+1
        inp, out = f'{index:04d}.input', f'{index:04d}.output'
        (self.directory/inp).write_bytes(input_bytes)
        (self.directory/out).write_bytes(output_bytes)
        row = dict(name=name,phase=phase,input=inp,output=out,
                   wire_input_bytes=len(input_bytes),wire_output_bytes=len(output_bytes),
                   elapsed_ms=elapsed_ms,**extra)
        self.rows.append(row)
        (self.directory/'operations.json').write_bytes(encoded(self.rows))
        return row


class Rpc:
    def __init__(self, binary, root, recorder, summary=False):
        self.binary, self.root, self.recorder = binary, root, recorder
        self.sequence, self.revision = 0, 0
        self.process = None
        self.summary = summary
        self.start()

    def start(self):
        began = time.perf_counter_ns()
        self.errors = (self.recorder.directory/f'server-{len(self.recorder.rows)}.stderr').open('wb')
        env = dict(os.environ)
        # Keep the benchmark's historical call-count and text metrics comparable:
        # exercise the exact flat tool names instead of domain discovery.
        env['AWR_MCP_TOOL_EXPOSURE_MODE'] = 'flat'
        self.process = subprocess.Popen([str(self.binary),'--project',str(self.root)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.errors,env=env)
        lines = queue.Queue()
        stream = self.process.stdout
        def pump():
            for line in iter(stream.readline,b''):
                lines.put(line)
            lines.put(None)
        self.lines = lines
        self.thread = threading.Thread(target=pump,daemon=True)
        self.thread.start()
        self.recorder.add('native_server_start','protocol',b'',b'',(time.perf_counter_ns()-began)/1e6)
        self.rpc('initialize',{'protocolVersion':'2026-07-28','capabilities':{},'clientInfo':{'name':'synthetic-workflow-benchmark','version':'1'}},'protocol')
        notice = encoded({'jsonrpc':'2.0','method':'notifications/initialized'})+b'\n'
        self.process.stdin.write(notice); self.process.stdin.flush()
        self.recorder.add('notifications/initialized','protocol',notice,b'',0)
        catalog = self.rpc('tools/list',{},'catalog')
        names = [tool['name'] for tool in catalog['tools']]
        require(len(names)==len(set(names)),'duplicate catalog tools')
        require(all(name in names for name in ['awr_work_prepare','awr_work_manage','awr_change_apply','awr_work_graph']),'missing required capability')

    def close(self):
        if self.process:
            began = time.perf_counter_ns()
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill(); self.process.wait()
            self.thread.join(timeout=2)
            self.process.stdout.close(); self.errors.close()
            self.process = None
            self.recorder.add('native_server_stop','protocol',b'',b'',(time.perf_counter_ns()-began)/1e6)

    def rpc(self, method, params, phase):
        self.sequence += 1
        request = encoded({'jsonrpc':'2.0','id':self.sequence,'method':method,'params':params})+b'\n'
        began = time.perf_counter_ns()
        self.process.stdin.write(request); self.process.stdin.flush()
        while True:
            line = self.lines.get(timeout=30)
            if line is None:
                raise RuntimeError('native MCP process ended without a response')
            result = json.loads(line)
            if result.get('id') == self.sequence:
                break
        elapsed = (time.perf_counter_ns()-began)/1e6
        response = result.get('result',{})
        tool_text = sum(len(c.get('text','').encode()) for c in response.get('content',[]) if c.get('type')=='text')
        row = self.recorder.add(params.get('name',method),phase,request,line,elapsed,
                               rpc=True,tool_call=method=='tools/call',tool_text_bytes=tool_text,
                               error=bool(result.get('error') or response.get('isError')),expected_rejection=False)
        if result.get('error'):
            raise RuntimeError(result['error'])
        return response

    def call(self, name, arguments, phase='maintenance', reject=False, request_id=None):
        arguments = dict(arguments)
        if self.summary and name in ('awr_work_prepare', 'awr_work_transition'):
            arguments['response_view'] = 'summary'
        if request_id:
            arguments['request_id'] = request_id
        response = self.rpc('tools/call',{'name':name,'arguments':arguments},phase)
        value = response['structuredContent']
        if isinstance(value.get('project_revision'),int):
            self.revision = max(self.revision,value['project_revision'])
        row = self.recorder.rows[-1]
        row['expected_rejection'] = reject and bool(response.get('isError'))
        row['mutation_identity'] = [name,arguments['request_id']] if 'expected_revision' in arguments and 'request_id' in arguments else None
        require(bool(response.get('isError')) == reject, f'{name}: expected rejection={reject}; {value}')
        return value

    def write(self, name, arguments, phase='maintenance', reject=False, request_id=None):
        arguments = dict(arguments,expected_revision=self.revision)
        return self.call(name,arguments,phase,reject,request_id or f'write-{self.sequence+1:04d}')


def source_fixture(root, dependencies, checked=False):
    root.mkdir()
    (root/'map.toml').write_text("[project]\nname='Workflow benchmark'\ncontext_profile='minimal'\n[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n[[sources]]\ndomain='rules'\nrole='primary'\npath='rules.md'\nadapter='markdown-rules-v1'\n")
    (root/'rules.md').write_text('# Evidence {#truth severity=hard scope=project value=*}\n\n'+RULE+'\n')
    text = f'goals:\n- id: G\n  title: {GOAL}\n  status: active\n  success_criteria: [A reviewed handoff is available]\nwork_items:\n'
    keys = ['D','W'] if dependencies else ['W']
    for key in keys:
        text += f'- id: {key}\n  title: Review the {key} handoff\n  status: planned\n  goal: G\n  next_action: Review the observations\n  acceptance: {json.dumps(CRITERIA)}\n  paths: [{key.lower()}]\n'
        if key=='W' and dependencies:
            text += '  depends_on: [D]\n'
    (root/'work.yaml').write_text(text)
    if checked:
        decision = '---\n{"id":"scope","status":"accepted","affected_keys":["*"]}\n---\n# Scope\n\n## Decision\nDeliver a reviewed local handoff with explicit limitations.\n'
        (root/'decisions').mkdir()
        (root/'decisions/scope.md').write_text(decision, encoding='utf-8')
        (root/'checks').mkdir()
        (root/'checks/verify.py').write_text(
            'import json, sys\nfrom pathlib import Path\n'
            'value = json.loads(Path("W-artifact.json").read_text(encoding="utf-8"))\n'
            'assert value["work"] == "W", "Artifact identity changed"\n'
            'field = value.get(sys.argv[1])\n'
            'assert isinstance(field, str) and field.strip(), "Missing reviewed artifact field"\n',
            encoding='utf-8')
        examples = [dict(id=f'{field}-{kind}', criterion=criterion, kind=kind,
                         given='A handoff is ready for review', when=f'The {field} field is inspected',
                         then='A reviewed value is present' if kind=='positive' else 'Missing or blank values fail review',
                         check=dict(run=[sys.executable, 'checks/verify.py', field], files=['checks/verify.py'], timeout_seconds=10))
                    for criterion, field in zip(CRITERIA, ['result', 'limitations'])
                    for kind in ['positive', 'negative']]
        definition = dict(version=1, purpose='implementation', tier='required', size='small',
                          intent=[dict(role='user_decision', decision='scope', source_fingerprint='sha256:'+digest(root/'decisions/scope.md'))],
                          contributes=[dict(goal='G', outcome='A reviewed handoff is available', claim='Provide a usable handoff', by='author')],
                          granularity=dict(judgment='independent_result', reason='One handoff with stated limitations', by='author'),
                          responsibility=dict(name='handoff', kind='delivery'), deliverable='A reviewed local handoff',
                          evidence_method='Compare direct checks before and after producing the artifact',
                          failure_recovery='Keep the previous reviewed handoff', examples=examples,
                          discrimination=dict(method='fail_before_pass_after'))
        ledger = dict(goals=[dict(id='G', title=GOAL, status='active', success_criteria=['A reviewed handoff is available'])],
                      work_items=[dict(id='W', title='Review the W handoff', status='planned', goal='G',
                                       acceptance=CRITERIA, paths=['w'], next_action='Produce and review the handoff', definition=definition)])
        (root/'work.yaml').write_bytes(encoded(ledger))
        mapping = (root/'map.toml').read_text().replace("adapter='yaml-ledger-v1'\n",
                  "adapter='yaml-ledger-v1'\n[sources.options.admission_policy]\nversion=1\nmode='governed_enforce'\nscope='all'\n")
        mapping += "[[sources]]\ndomain='decisions'\nrole='supporting'\npath='decisions'\nadapter='markdown-directory-v1'\n"
        (root/'map.toml').write_text(mapping)
        (root/'W-artifact.json').write_bytes(encoded(dict(work='W', result='', limitations='')))
        (root/'.gitignore').write_text('.awr/\nW-artifact.json\nW-report.json\n')
        for args in [('init',), ('config','user.name','Workflow fixture'), ('config','user.email','workflow@example.invalid'),
                     ('config','core.autocrlf','false'), ('add','map.toml','work.yaml','rules.md','decisions/scope.md','checks/verify.py','.gitignore'),
                     ('commit','-m','Declare the checked handoff contract')]:
            subprocess.run(['git', *args], cwd=root, capture_output=True, check=True)
    return {str(p.relative_to(root)):digest(p) for p in root.rglob('*')
            if p.is_file() and '.git' not in p.relative_to(root).parts}


def cli(binary, root, rec, *args, phase='maintenance'):
    command = [str(binary),'--project',str(root),'--json',*args]
    began = time.perf_counter_ns()
    result = subprocess.run(command,capture_output=True,timeout=30)
    rec.add('cli '+' '.join(args[:2]),phase,encoded(command),result.stdout+result.stderr,(time.perf_counter_ns()-began)/1e6,tool_call=True,error=result.returncode!=0,expected_rejection=False,tool_text_bytes=len(result.stdout))
    require(result.returncode==0,result.stderr.decode(errors='replace'))
    return json.loads(result.stdout)


def prepare(rpc, strategy, key, session, criteria):
    if strategy in ('prepared', 'summary'):
        value = rpc.call('awr_work_prepare',{'work':key,'session':session,'source_sha':SHA,'goals':['G']},'context')
        context, management = value['context'],value['management']
    else:
        rpc.call('awr_work_get',{'work':key,'source_sha':SHA},'context')
        management = rpc.call('awr_work_assess',{'work':key},'context')
        context = rpc.call('awr_context_compile',{'work':key,'session':session,'source_sha':SHA,'goals':['G']},'context')
    context_hash = check_context(context,key,criteria)
    return management, context_hash


def manage(rpc, key, session, management, continuous=False):
    observation = dict(observed_at=time.time_ns()//1_000_000,note='Synthetic host observes explicit output scope and ownership.',
                       single_outcome=not continuous,bounded_scope=True,single_executor=True,no_deferred_wait=True,
                       independently_schedulable_units=2 if continuous else 1,plan_valid=True,outcome_known=True)
    return rpc.write('awr_work_manage',dict(work=key,session=session,request_key=f'observation-{rpc.sequence+1}',contract_fingerprint=management['contract_fingerprint'],observation=observation))


def source_edit(rpc, key, fields, reject=False):
    graph = rpc.call('awr_work_graph',{'roots':[key]})
    node = next(n for n in graph['nodes'] if n['key']==key)
    request = dict(request_id=f'edit-{rpc.sequence+1}',reason='Record the reviewed scope change',change=dict(kind='edit',change=dict(operation='fields',kind='work_item',target=key,source_fingerprint=node['source_ref']['source_fingerprint'],fields=fields)))
    preview = rpc.call('awr_change_preview',request,reject=reject)
    if reject:
        require(preview.get('code')=='ClaimConflict','occupied contract edit was not rejected by its claim')
        return
    request.update(expected_revision=preview['project_revision'],expected_preview=preview['preview']['fingerprint'])
    rpc.call('awr_change_apply',request)


def write_artifact(root, rec, key, criteria, executions):
    executions[key] = executions.get(key,0)+1
    value = dict(work=key,result='The observations support a concise handoff.',limitations='This is synthetic protocol evidence, with no model or enterprise acceptance.')
    began = time.perf_counter_ns()
    data = encoded(value); (root/(key+'-artifact.json')).write_bytes(data)
    if UPGRADE_CRITERION in criteria:
        more = encoded(dict(work=key,tradeoffs=['Review cost increases when scope requires a second output.']))
        (root/(key+'-tradeoffs.json')).write_bytes(more); data += more
    rec.add('produce_and_review_artifact','business',data,b'',(time.perf_counter_ns()-began)/1e6,host_operation=True)
    check_artifact(root,key,criteria)


def run_work(rpc, cli_binary, root, rec, strategy, scenario, key, executions):
    criteria = list(CRITERIA)
    started = rpc.write('awr_session_start',dict(work=key,conversation='conversation-'+key,agent='synthetic-'+key,provider='fixture',model='no-model',claim=True))
    session, identity = started['session']['id'],started['session']['work_item_id']
    claim = started['claim']['id']
    management, context_hash = prepare(rpc,strategy,key,session,criteria)
    recorded = manage(rpc,key,session,management,continuous=scenario=='dependencies')
    require(recorded['assessment']['decision']['mode']==('continuous' if scenario=='dependencies' else 'lightweight'), 'initial observed management profile is wrong')
    rpc.write('awr_work_transition',dict(work=key,session=session,action='progress',reason='Begin the consumed fixture work',next_action='Produce and review the handoff'))
    guard = rpc.write('awr_work_transition',dict(work=key,session=session,action='complete',reason='Probe missing evidence without claiming a result'),phase='guard',reject=True)
    require(guard.get('code',guard.get('error',{}).get('code'))=='EvidenceMissing','completion guard did not require evidence')
    not_done = rpc.call('awr_work_get',{'work':key},'guard')
    require(not_done['work']['status']!='completed','false completion was recorded')

    if scenario=='upgrade':
        criteria.append(UPGRADE_CRITERION)
        source_edit(rpc,key,{'acceptance':criteria},reject=True)
        rpc.write('awr_session_claim',dict(session=session,action='release',claim=claim))
        source_edit(rpc,key,{'acceptance':criteria})
        management, context_hash = prepare(rpc,strategy,key,session,criteria)
        require(management['decision']['mode']=='continuous','changed contract did not upgrade management')
        require(management['work_id']==identity,'upgrade replaced task identity')
        manage(rpc,key,session,management,continuous=True)
        rpc.write('awr_session_claim',dict(session=session,action='acquire'))

    if scenario=='wait':
        management, context_hash = prepare(rpc,strategy,key,session,criteria)
        wait = rpc.write('awr_session_wait',dict(session=session,question='Which reporting period should the handoff cover?',context_hash=context_hash,digest='The reporting period needs clarification',next_action='Continue after the period is supplied',open_loops=['reporting period']),phase='continuity')
        rpc.close(); rpc.start()
        observed = rpc.call('awr_session_get',{'session':session},'recovery')
        require('Which reporting period should the handoff cover?' in json.dumps(observed),'persistent wait question was lost')
        rpc.write('awr_work_transition',dict(work=key,session=session,action='progress',reason='Probe unresolved wait'),phase='guard',reject=True)
        reply = rpc.write('awr_session_reply',dict(wait=wait['wait']['id'],reply='Use the previous complete quarter.'),phase='continuity')
        require(reply['wait']['reply']=='Use the previous complete quarter.','reply was not retained')
        management, context_hash = prepare(rpc,strategy,key,session,criteria)
        require(management['decision']['mode']=='continuous','real wait did not upgrade management')
        require(management['work_id']==identity,'wait lost original work identity')

    management, context_hash = prepare(rpc,strategy,key,session,criteria)
    write_artifact(root,rec,key,criteria,executions)
    request = f'checkpoint-{key}'
    saved = rpc.write('awr_session_checkpoint',dict(session=session,context_hash=context_hash,digest='Actual fixture artifact reviewed with limitations',next_action='Record the completion evidence',open_loops=[]),phase='continuity',request_id=request)
    if scenario=='unknown_result':
        # The server really saved it. Simulate the host losing this response, then restart.
        # The saved value below is evaluator-only; recovery uses the original request ID.
        checkpoint_id = saved['checkpoint']['id']
        rpc.recorder.rows[-1]['response_withheld_from_host'] = True
        rpc.close(); rpc.start()
        outcome = rpc.call('awr_operation_get',{'request_id':request},'recovery')
        require(outcome['operation']['status']=='finished','saved operation result was lost')
        actual = rpc.call('awr_session_get',{'session':session},'recovery')
        require(checkpoint_id in json.dumps(actual),'restart lost the exact saved checkpoint')
        # No checkpoint or executor replay follows this query.

    began = time.perf_counter_ns()
    report = dict(version=1,work_item=key,source_sha=SHA,command='Independently inspect the synthetic handoff artifacts',scope=[key],verified_at=time.time_ns()//1_000_000,
                  checks=[dict(name=f'criterion-{i}',passed=True,details='Actual artifact fields and limitations checked',criteria=[criterion]) for i,criterion in enumerate(criteria)])
    data = encoded(report)+b'\n'; report_path = key+'-report.json'; (root/report_path).write_bytes(data)
    rec.add('assemble_actual_report','maintenance',data,b'',(time.perf_counter_ns()-began)/1e6,host_operation=True)
    preflight = rpc.call('awr_completion_prepare',dict(work=key,report=report_path,evidence_key='PROOF-'+key,source_sha=SHA,level='locally_verified'))
    require(preflight['report_sha256']==digest(root/report_path),'report hash was invented or changed')
    rpc.write('awr_evidence_record',preflight['evidence'])
    rpc.write('awr_work_transition',dict(work=key,session=session,action='complete',reason='Actual fixture report covers all criteria',completion=preflight['completion']))
    rpc.write('awr_session_end',dict(session=session,outcome='ended'))
    final = cli(cli_binary,root,rec,'work','show',key,'--source-sha',SHA,phase='verification')
    report_hash = check_final(root,key,identity,criteria,final,executions)
    return dict(key=key,criteria=criteria,report_sha256=report_hash,management=management['decision']['mode'])


def run_checked_work(rpc, cli_binary, root, rec, strategy, executions):
    """Use recorded baseline/acceptance checks as the completion evidence."""
    key = 'W'
    started = rpc.write('awr_session_start', dict(work=key, conversation='checked-author-conversation', agent='checked-author', provider='fixture', model='no-model', claim=False))
    session, identity = started['session']['id'], started['session']['work_item_id']
    management, _ = prepare(rpc, strategy, key, session, CRITERIA)
    manage(rpc, key, session, management)
    baseline = cli(cli_binary, root, rec, 'work', 'check', key, '--session', session, '--phase', 'baseline',
                   '--require-met', '--expected-revision', str(rpc.revision))
    rpc.revision = baseline['project_revision']
    require(baseline['met'] and len(baseline['results'])==4 and all(r['outcome']=='failed' for r in baseline['results']),
            'checked completion needs a failed baseline for every example')
    judgment = dict(contribution='Provide the reviewed handoff', necessity='The artifact is missing reviewed fields',
                    alternatives='Reuse the existing handoff format', deliverable='A reviewed local handoff',
                    exit='Both fields pass positive and negative checks', responsibility='handoff')
    proposal = rpc.write('awr_work_admit', dict(work=key, session=session, request_key='checked-proposal',
                         decision='proposed', contract_fingerprint=management['contract_fingerprint'], judgment=judgment))
    reviewer = rpc.write('awr_session_start', dict(work=key, conversation='checked-reviewer-conversation', agent='checked-reviewer', provider='fixture', model='no-model', claim=False))['session']['id']
    rpc.write('awr_work_admit', dict(work=key, session=reviewer, request_key='checked-approval', decision='approved',
              contract_fingerprint=management['contract_fingerprint'], proposal_event=proposal['event']['id']))
    rpc.write('awr_session_claim', dict(session=session, action='acquire'))
    rpc.write('awr_work_transition', dict(work=key, session=session, action='progress', reason='Produce the checked handoff', next_action='Produce and review the handoff artifact'))
    guard = rpc.write('awr_work_transition', dict(work=key, session=session, action='complete', reason='Probe missing check evidence'),
                      phase='guard', reject=True)
    require(guard.get('code')=='EvidenceMissing', 'unchecked completion was not refused')
    require(rpc.call('awr_work_get', {'work':key}, 'guard')['work']['status']!='completed', 'false completion changed source state')
    write_artifact(root, rec, key, CRITERIA, executions)
    began = time.perf_counter_ns()
    commands = [['git', 'add', 'work.yaml', '.gitignore'], ['git', 'commit', '-m', 'Record the started handoff work']]
    outputs = [subprocess.run(command, cwd=root, capture_output=True, check=True) for command in commands]
    rec.add('commit_checked_sources', 'business', encoded(commands),
            b''.join(p.stdout+p.stderr for p in outputs), (time.perf_counter_ns()-began)/1e6, host_operation=True)
    acceptance = cli(cli_binary, root, rec, 'work', 'check', key, '--session', session, '--phase', 'acceptance',
                     '--require-met', '--write-input', str(root/'.awr/completion.json'), '--expected-revision', str(rpc.revision))
    rpc.revision = acceptance['project_revision']
    require(acceptance['met'] and len(acceptance['results'])==4 and all(r['outcome']=='passed' for r in acceptance['results']),
            'checked completion lost acceptance coverage')
    completion = json.loads((root/'.awr/completion.json').read_text())
    require(completion==acceptance['completion_input'], 'written completion input differs from the check receipt')
    report = root/acceptance['evidence']['locator']
    require(digest(report)==acceptance['evidence']['sha256'], 'generated report hash differs from registered evidence')
    (root/'W-report.json').write_bytes(report.read_bytes())
    done = rpc.write('awr_work_transition', dict(work=key, session=session, action='complete', reason='The registered handoff checks pass',
                     completion=completion, end_session=True))
    require(done['check_gate']['would_refuse'] is False, 'completion did not satisfy the enforced check gate')
    ended = rpc.call('awr_session_get', {'session':session}, 'verification')
    require(ended['session']['status']=='ended' and all(c['status']=='released' for c in ended['claims']), 'combined close-out left a claim or active author')
    rpc.write('awr_session_end', dict(session=reviewer, outcome='ended'))
    final = cli(cli_binary, root, rec, 'work', 'show', key, '--source-sha', completion['source_sha'], phase='verification')
    report_hash = check_final(root, key, identity, CRITERIA, final, executions)
    return dict(key=key, criteria=CRITERIA, report_sha256=report_hash, management=management['decision']['mode'])


def metrics(rows, wall_ms):
    maintenance = [r for r in rows if r['phase']=='maintenance']
    identities = [tuple(r['mutation_identity']) for r in rows if r.get('mutation_identity')]
    transport = [r for r in rows if not r.get('host_operation')]
    return dict(tool_calls=sum(bool(r.get('tool_call')) for r in rows),rpc_calls=sum(bool(r.get('rpc')) for r in rows),
                wire_input_bytes=sum(r['wire_input_bytes'] for r in transport),wire_output_bytes=sum(r['wire_output_bytes'] for r in transport),
                tool_text_bytes=sum(r.get('tool_text_bytes',0) for r in rows),catalog_bytes=sum(r['wire_output_bytes'] for r in rows if r['phase']=='catalog'),
                tool_elapsed_ms=sum(r['elapsed_ms'] for r in rows if r.get('tool_call')),wall_elapsed_ms=wall_ms,
                maintenance_calls=len(maintenance),maintenance_bytes=sum(r['wire_input_bytes']+r['wire_output_bytes'] for r in maintenance),maintenance_ms=sum(r['elapsed_ms'] for r in maintenance),
                recovery_calls=sum(r['phase']=='recovery' for r in rows),expected_rejections=sum(r.get('expected_rejection',False) for r in rows),
                unexpected_errors=sum(r.get('error',False) and not r.get('expected_rejection',False) for r in rows),explicit_replays=len(identities)-len(set(identities)))


def cost_segments(rows):
    """Disjoint subtotals; the original all-inclusive metrics remain unchanged."""
    def group(phase):
        if phase == 'onboarding': return 'project_onboarding'
        if phase in ('protocol', 'catalog'): return 'connection_setup_and_teardown'
        if phase in ('guard', 'verification'): return 'independent_verification'
        return 'normal_workflow'
    result = {}
    for name in ('project_onboarding', 'connection_setup_and_teardown', 'independent_verification', 'normal_workflow'):
        selected = [r for r in rows if group(r['phase']) == name]
        result[name] = dict(tool_calls=sum(bool(r.get('tool_call')) for r in selected),
                            tool_text_bytes=sum(r.get('tool_text_bytes',0) for r in selected),
                            elapsed_ms=sum(r['elapsed_ms'] for r in selected))
    return result


def run_case(cli_binary,mcp_binary,directory,strategy,scenario):
    directory.mkdir()
    root = directory/'project'
    sources = source_fixture(root,scenario=='dependencies',checked=scenario=='checked_completion')
    rec = Recorder(directory/'receipts')
    began = time.perf_counter_ns()
    cli(cli_binary,root,rec,'init','--manifest','map.toml','--accept',phase='onboarding')
    if scenario=='checked_completion':
        policy = b"version=1\n[checks]\nmode='enforce'\nscope='all'\n"
        (root/'.awr/gates.toml').write_bytes(policy)
        rec.add('configure_checked_completion', 'onboarding', policy, b'', 0, host_operation=True)
    rpc = Rpc(mcp_binary,root,rec,summary=strategy=='summary')
    executions, works = {}, []
    try:
        rpc.call('awr_project_status',{'view':'summary'})
        if scenario=='dependencies':
            graph = rpc.call('awr_work_graph',{})
            require(not next(n for n in graph['nodes'] if n['key']=='W')['ready'],'unfinished prerequisite was ignored')
            works.append(run_work(rpc,cli_binary,root,rec,strategy,scenario,'D',executions))
            graph = rpc.call('awr_work_graph',{})
            require(next(n for n in graph['nodes'] if n['key']=='W')['ready'],'verified predecessor did not release its dependent')
        if scenario=='checked_completion':
            works.append(run_checked_work(rpc,cli_binary,root,rec,strategy,executions))
        else:
            works.append(run_work(rpc,cli_binary,root,rec,strategy,scenario,'W',executions))
        graph = rpc.call('awr_work_graph',{},'verification')
        require(all(n['status']=='completed' and not n['active_claims'] for n in graph['nodes']),'final source/claim state is incomplete')
        require(set(executions)==set(w['key'] for w in works),'unexpected or missing business execution')
        if scenario=='lightweight':
            require(works[0]['management']=='lightweight','bounded task lost lightweight classification')
        rpc.close()
        checks = {key:True for key in CONTRACT['required_checks']}
        measure = metrics(rec.rows,(time.perf_counter_ns()-began)/1e6)
        require(measure['unexpected_errors']==0,'unexpected operation error was hidden')
        (rec.directory/'operations.json').write_bytes(encoded(rec.rows))
        by_phase = {phase:dict(calls=sum(r['phase']==phase for r in rec.rows),elapsed_ms=sum(r['elapsed_ms'] for r in rec.rows if r['phase']==phase),io_bytes=sum(r['wire_input_bytes']+r['wire_output_bytes'] for r in rec.rows if r['phase']==phase)) for phase in sorted({r['phase'] for r in rec.rows})}
        by_tool = {name:[r['elapsed_ms'] for r in rec.rows if r['name']==name] for name in sorted({r['name'] for r in rec.rows if r.get('tool_call')})}
        result = dict(strategy=strategy,scenario=scenario,initial_sources=sources,completed_work=len(works),checks=checks,metrics=measure,phases=by_phase,cost_segments=cost_segments(rec.rows),tool_latency_samples_ms=by_tool,missing_metrics=CONTRACT['missing_metrics'])
        (directory/'result.json').write_bytes(encoded(result))
        return result
    except Exception as error:
        (directory/'failure.json').write_bytes(encoded(dict(error=str(error),rows=rec.rows)))
        raise
    finally:
        rpc.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--awr',type=Path,required=True)
    parser.add_argument('--mcp',type=Path,required=True)
    parser.add_argument('--runtime-source-sha',required=True)
    parser.add_argument('--output',type=Path,required=True,help='New ignored directory under .local')
    parser.add_argument('--repetitions',type=int,default=3)
    parser.add_argument('--include-summary',action='store_true',help='Also measure optional summary views; retain both original strategies')
    args = parser.parse_args()
    require(args.repetitions>=1,'repetitions must be positive')
    require(len(args.runtime_source_sha)==40 and all(c in '0123456789abcdef' for c in args.runtime_source_sha),'full runtime source SHA required')
    output = args.output.resolve(); output.relative_to(ROOT/'.local'); output.mkdir(parents=True,exist_ok=False)
    cli_binary,mcp_binary = args.awr.resolve(strict=True),args.mcp.resolve(strict=True)
    binary_hashes = dict(awr=digest(cli_binary),mcp=digest(mcp_binary))
    cases = []
    selected_strategies = CONTRACT['strategies'] + (['summary'] if args.include_summary else [])
    for repetition in range(args.repetitions):
        for scenario in CONTRACT['workloads']:
            # Alternate order; equal-length paths avoid systematic path-length wire differences.
            strategies = selected_strategies if repetition%2==0 else list(reversed(selected_strategies))
            pair = []
            for strategy in strategies:
                suffix = {'primitive':'control0','prepared':'prepare0','summary':'summary0'}[strategy]
                result = run_case(cli_binary,mcp_binary,output/f'{repetition:02d}-{scenario}-{suffix}',strategy,scenario)
                result['repetition'] = repetition; cases.append(result); pair.append(result)
                print(json.dumps(dict(scenario=scenario,strategy=strategy,repetition=repetition,passed=all(result['checks'].values()),metrics=result['metrics'])),flush=True)
            require(all(case['initial_sources']==pair[0]['initial_sources'] for case in pair),'comparison input sources differ')
    require(binary_hashes==dict(awr=digest(cli_binary),mcp=digest(mcp_binary)),'measured binary changed')
    summary = []
    for scenario in CONTRACT['workloads']:
        values = {}
        for strategy in selected_strategies:
            rows = [r['metrics'] for r in cases if r['scenario']==scenario and r['strategy']==strategy]
            values[strategy] = {key:dict(median=statistics.median(r[key] for r in rows),min=min(r[key] for r in rows),max=max(r[key] for r in rows)) for key in CONTRACT['metrics']}
        summary.append(dict(scenario=scenario,strategies=values))
    report = dict(benchmark=CONTRACT['benchmark'],measured_at=datetime.now(timezone.utc).isoformat(),runtime_source_sha=args.runtime_source_sha,
                  binary_sha256=binary_hashes,harness_sha256={p.name:digest(p) for p in [HERE/'run.py',HERE/'oracle.py',HERE/'contract.json']},
                  environment=dict(os=platform.system(),architecture=platform.machine(),python=platform.python_version()),
                  comparison=CONTRACT['comparison'],repetitions=args.repetitions,workflow_runs=len(cases),checks={key:all(c['checks'][key] for c in cases) for key in CONTRACT['required_checks']},
                  metrics=summary,missing_metrics=CONTRACT['missing_metrics'],scope=CONTRACT['scope'])
    report['tool_latency_ms'] = {}
    report['cost_segments'] = [dict(scenario=c['scenario'],strategy=c['strategy'],repetition=c['repetition'],segments=c['cost_segments']) for c in cases]
    for strategy in selected_strategies:
        names = sorted({name for case in cases if case['strategy']==strategy for name in case['tool_latency_samples_ms']})
        report['tool_latency_ms'][strategy] = {}
        for name in names:
            samples = sorted(v for case in cases if case['strategy']==strategy for v in case['tool_latency_samples_ms'].get(name,[]))
            report['tool_latency_ms'][strategy][name] = dict(samples=len(samples),median=statistics.median(samples),min=samples[0],max=samples[-1])
    (output/'aggregate.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps({'aggregate':str(output/'aggregate.json'),'workflow_runs':len(cases),'checks':report['checks']}))


if __name__=='__main__':
    main()
