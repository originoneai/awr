"""Assemble reviewed reports from AWR's managed execution journal, never shell text.

These helpers run under Workflow's lock. Runtime checks remain authoritative.
"""
import hashlib
import json
from pathlib import Path
import time
import uuid

from host import digest, protected_file


def fingerprint(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False,
                                     separators=(',', ':')).encode()).hexdigest()


class ExecutionReports:
    def __init__(self, workflow):
        self.wf = workflow
        self.root = Path(workflow.binding['project'])

    def path(self, value, exists=True):
        path = self.root / value
        resolved = path.resolve(strict=exists)
        if not resolved.is_relative_to(self.root) or path.absolute() != resolved:
            raise ValueError('Receipt paths must stay in the project without symlinks or traversal')
        if exists and not resolved.is_file():
            raise ValueError('Receipt paths must identify regular files')
        return resolved

    def hashes(self, paths):
        return {str(self.path(p).relative_to(self.root)): digest(self.path(p)) for p in paths}

    def verify(self, hashes):
        if self.hashes(hashes) != hashes:
            raise ValueError('Source, output or receipt bytes changed; retain the old evidence and verify again')

    def store(self, suffix, value):
        # One trailing newline is the canonical stored form: the completion
        # preflight binds the digest to the exact stored bytes and refuses
        # reports that do not end with a single LF.
        path = protected_file(self.wf.path.parent, suffix,
                              (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode())
        return dict(path=str(path), sha256=digest(path))

    def read(self, record):
        path = self.path(record['path'])
        if digest(path) != record['sha256']:
            raise ValueError('Saved execution collection or report changed')
        return json.loads(path.read_text())

    def run(self, key, purpose, command, source_paths, artifact_paths):
        wf = self.wf
        wf.active()
        wf.require_consumed()
        if 'execution.managed' not in wf.capabilities:
            raise ValueError('Pinned program does not support managed execution')
        # Report locators must be readable by AWR inside this project.
        self.path(str(wf.path))
        if (not isinstance(key, str) or not key.strip() or not isinstance(purpose, str) or not purpose.strip()
                or not isinstance(command, list) or not command
                or any(not isinstance(s, str) or not s for s in command)):
            raise ValueError('A stable key, purpose and nonempty argv list are required')
        if (not isinstance(source_paths, list) or not source_paths or
                not isinstance(artifact_paths, list) or not artifact_paths or
                any(not isinstance(p, str) or not p for p in source_paths + artifact_paths)):
            raise ValueError('Explicit nonempty source and artifact file lists are required')
        sources = self.hashes(source_paths)
        artifacts = sorted({str(self.path(p, False).relative_to(self.root)) for p in artifact_paths})
        request = dict(purpose=purpose, command=command, sources=sources, artifacts=artifacts)
        runs = wf.state.setdefault('runs', {})
        if key in runs:
            if runs[key]['request'] != request:
                raise ValueError('Run key is bound to a different command, scope or source snapshot')
            return self.observe(runs[key])  # Query only, including after explicit reconciliation.
        if any(self.path(p, False).exists() for p in artifacts):
            raise ValueError('Use new artifact paths so a stale output cannot stand in for this run')
        session, item = wf.session_identity(wf.state['session'], wf.state['work'])
        if session['session']['status'] != 'active':
            raise ValueError('Execution requires an active session')
        run = dict(request=request, operation_key='host/' + str(uuid.uuid4()),
                   snapshot_at=time.time_ns() // 1_000_000, source_sha=fingerprint(sources),
                   source_basis='sha256 of explicit project-relative file hashes; not a Git commit',
                   session=wf.state['session'], work=wf.state['work'], work_id=item['work']['id'],
                   branch=session['session']['branch_id'], context_hash=wf.state['context']['hash'])
        runs[key] = run
        wf.save()  # Persist the source snapshot and unique runtime key before dispatch.
        value = wf.perform('run', ['execution', 'run', '--session', run['session'], '--key',
            run['operation_key'], '--purpose', purpose, '--', *command])
        self.identity(run, value['execution'])
        run['execution_id'] = value['execution']['id']
        wf.state['history'].append(wf.state['pending'])
        wf.state['pending'] = None  # This API has no project_revision; do not invent one.
        wf.save()
        return value

    def identity(self, run, execution):
        expected = dict(operation_key=run['operation_key'], purpose=run['request']['purpose'],
                        executor='managed_local', command=run['request']['command'],
                        external_reference=None)
        intent = dict(execution['intent'])
        # Rust canonical paths use the verbatim prefix on Windows; Python may not.
        # Compare filesystem identity, while retaining exact argv and execution bindings.
        cwd_matches = Path(intent.pop('cwd')).samefile(self.root)
        if (intent != expected or not cwd_matches or execution['project_id'] != self.wf.binding['project_id']
                or execution['session_id'] != run['session'] or execution['work_item_id'] != run['work_id']
                or execution['branch_id'] != run['branch'] or execution['registered_at'] < run['snapshot_at']):
            raise ValueError('Execution does not match the saved pre-dispatch source and identity')

    def observe(self, run):
        wf = self.wf
        if run.get('execution_id'):
            execution = wf.host.ok('execution', 'show', run['execution_id'])['execution']
        else:
            records = wf.host.ok('execution', 'list', '--work', run['work'])['executions']
            matches = [e for e in records if e['intent']['operation_key'] == run['operation_key']]
            if not matches:
                return dict(state='unknown', eligible=False, operation_key=run['operation_key'],
                            next_action='Inspect the original dispatch receipt; no automatic retry')
            if len(matches) != 1:
                raise ValueError('Execution key is ambiguous')
            execution = matches[0]
        self.identity(run, execution)
        observation = wf.host.ok('execution', 'inspect', execution['id'])['observation']
        if observation['state'] in ('succeeded', 'failed'):
            # The supervisor may have started between the first read and inspection.
            execution = wf.host.ok('execution', 'show', execution['id'])['execution']
            self.identity(run, execution)
        return dict(state=observation['state'], eligible=False, execution=execution, observation=observation)

    def collect(self, key):
        run = self.wf.state.get('runs', {}).get(key)
        if run is None:
            raise ValueError('No saved pre-dispatch source snapshot for this run key')
        if run.get('collection'):
            value = self.read(run['collection'])
            self.verify(value['file_hashes'])
            return dict(value, collection=run['collection'])
        value = self.observe(run)
        observation = value.get('observation', {})
        if not (observation.get('verified') and observation.get('state') == 'succeeded'
                and observation.get('exit_code') == 0 and not observation.get('error')):
            return value  # Failed/running/unknown outcomes remain queryable, never acceptable evidence.
        self.verify(run['request']['sources'])
        paths = [observation.get(k) for k in ('stdout', 'stderr', 'receipt')]
        if not all(paths):
            raise ValueError('Managed execution has no complete log and result receipt set')
        files = self.hashes(list(run['request']['sources']) + paths + run['request']['artifacts'])
        result = json.loads(self.path(observation['receipt']).read_text())
        expected = dict(execution_id=value['execution']['id'], success=True, exit_code=0,
                        error=None, signal=observation['signal'], finished_at=observation['evidence_at'],
                        nonce=value['execution']['worker']['nonce'])
        if result != expected:
            raise ValueError('Managed result receipt differs from the verified execution')
        value.update(eligible=True, file_hashes=files, source_sha=run['source_sha'],
                     source_basis=run['source_basis'], source_files=run['request']['sources'],
                     artifact_paths=run['request']['artifacts'], snapshot_at=run['snapshot_at'])
        run['execution_id'] = value['execution']['id']
        run['collection'] = self.store('.execution.json', value)
        self.wf.save()
        return dict(value, collection=run['collection'])

    def preflight(self, report):
        value = self.wf.host.ok('work', 'prepare-completion', self.wf.state['work'],
            '--report', report['path'], '--evidence-key', report['evidence_key'],
            '--source-sha', report['source_sha'])
        self.wf.state['last_revision'] = value['project_revision']
        self.wf.save()
        return value

    def prepare(self, key, checks, reviewer, evidence_key):
        wf = self.wf
        wf.active()
        wf.require_consumed()
        if 'completion.prepare' not in wf.capabilities:
            raise ValueError('Pinned program does not support completion preflight')
        if (not isinstance(reviewer, str) or not reviewer.strip() or not isinstance(checks, list)
                or not checks or any(not isinstance(c, dict) or c.get('passed') is not True for c in checks)):
            raise ValueError('An explicit reviewer and passing reviewed checks are required; exit zero is insufficient')
        collected = self.collect(key)
        if not collected['eligible']:
            raise ValueError('Execution result is failed, incomplete or unknown; inspect it without replay')
        report = dict(version=1, work_item=wf.state['work'], source_sha=collected['source_sha'],
                      command=json.dumps(collected['execution']['intent']['command'], ensure_ascii=False),
                      scope=[wf.state['work']], verified_at=time.time_ns() // 1_000_000,
                      checks=checks, reviewer=reviewer, review_basis='explicit caller review; not inferred from exit code',
                      execution_receipt=collected)
        record = dict(self.store('.report.json', report), key=key, source_sha=report['source_sha'],
                      evidence_key=evidence_key, id=str(uuid.uuid4()))
        preflight = self.preflight(record)  # AWR checks current criteria and report bindings.
        wf.state.setdefault('reports', {})[record['id']] = record
        wf.save()
        return dict(report=record, preflight=preflight, completion_claimed=False)

    def finish(self, report_id, reason, expected_revision, response_view):
        wf = self.wf
        wf.available()
        wf.response_args(response_view)
        record = wf.state.get('reports', {}).get(report_id)
        if record is None:
            raise ValueError('Select an explicitly reviewed report from this workflow')
        if wf.state['phase'] not in ('active', 'work_completed'):
            raise ValueError('Work must be active or awaiting session closure')
        if wf.state['phase'] == 'work_completed':
            # Source completion can legitimately edit a tracked ledger. Only session end remains.
            return wf._finish({}, reason, expected_revision, response_view)
        self.read(record)
        self.collect(record['key'])  # Verify the original collection and its current file hashes again.
        wf.require_consumed()
        preflight = self.preflight(record)
        revision = preflight['project_revision'] if expected_revision is None else expected_revision
        draft = dict(preflight['evidence'])
        # The CLI preflight already returns the exact evidence-add payload.
        # Keep its resolved branch binding when forwarding it unchanged.
        # Look up the exact evidence key even after a lost registration response. No duplicate write.
        # Preflight accepts reports up to 1 MiB; recovery must read that same accepted report.
        found = wf.host.call('evidence', 'show', record['evidence_key'], '--source-sha', record['source_sha'],
                             '--content', '--max-bytes', '1048576')
        if found.exit_code == 0 and not found.outcome_unknown:
            existing = found.require()
            e = existing['evidence']
            run = wf.state['runs'][record['key']]
            if (e['sha256'] != record['sha256'] or e['source_sha'] != record['source_sha']
                    or e['work_item_id'] != run['work_id'] or e['branch_id'] != run['branch']
                    or e['locator'] != record['path'] or not existing['content_hash_verified']):
                raise ValueError('Evidence key belongs to a different report, source, branch or work item')
        elif (not found.outcome_unknown and found.exit_code != 0 and
              isinstance(found.error, dict) and found.error.get('code') == 'NotFound'):
            value = wf.perform('evidence', ['evidence', 'add', '--expected-revision', str(revision)], draft)
            wf.completed(value)
            revision = value['project_revision']
        else:
            found.require()
        return wf._finish(preflight['completion'], reason, revision, response_view)
