#!/usr/bin/env python3
"""Exercise ledger evidence currency through real, isolated CLI processes."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import sys
import time
import traceback

REPO = Path(__file__).resolve().parents[2]
CHECKER = '''import argparse, json, pathlib, subprocess, sys, time
p = argparse.ArgumentParser()
p.add_argument('--work', required=True)
p.add_argument('--sha', required=True)
p.add_argument('--output', required=True)
a = p.parse_args()
artifact = pathlib.Path('artifacts') / (a.work + '.txt')
actual = artifact.read_bytes()
passed = actual == (a.work + ' delivered\\n').encode('utf-8')
report = dict(version=1, work_item=a.work, source_sha=a.sha,
 command=subprocess.list2cmdline([sys.executable, *sys.argv]),
 scope=[artifact.as_posix()], verified_at=time.time_ns() // 1000000,
 checks=[dict(name='artifact bytes', passed=passed,
 details='Observed artifact bytes match expected output' if passed else 'Artifact bytes differ',
 criteria=[a.work + ' artifact bytes match expected output'])])
pathlib.Path(a.output).write_text(json.dumps(report, indent=2), encoding='utf-8')
raise SystemExit(0 if passed else 1)
'''


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def binary_identity(path):
    with Path(path).open('rb') as stream:
        head = stream.read(64)
        if head[:4] == b'\xcf\xfa\xed\xfe':
            machine = struct.unpack_from('<I', head, 4)[0]
            kind, arch = 'Mach-O', {0x1000007: 'x86_64', 0x100000C: 'arm64'}.get(machine, 'unknown')
        elif head[:4] == b'\x7fELF':
            machine = struct.unpack_from('<H' if head[5] == 1 else '>H', head, 18)[0]
            kind, arch = 'ELF', {62: 'x86_64', 183: 'arm64'}.get(machine, 'unknown')
        elif head[:2] == b'MZ':
            stream.seek(struct.unpack_from('<I', head, 60)[0])
            pe = stream.read(6)
            machine = struct.unpack_from('<H', pe, 4)[0] if pe[:4] == b'PE\0\0' else 0
            kind, arch = 'PE', {0x8664: 'x86_64', 0xAA64: 'arm64'}.get(machine, 'unknown')
        else:
            kind, arch = 'unknown', 'unknown'
    host = {'aarch64': 'arm64', 'ARM64': 'arm64', 'AMD64': 'x86_64'}.get(platform.machine(), platform.machine())
    return dict(format=kind, architecture=arch, host_architecture=host,
                architecture_relation='same' if host == arch else 'different',
                note='Architecture matching alone does not establish native hardware execution.')


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def inputs():
    names = subprocess.check_output(
        ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=REPO
    ).split(b'\0')
    return {os.fsdecode(name): digest(REPO / os.fsdecode(name))
            for name in sorted(set(names)) if name and (REPO / os.fsdecode(name)).is_file()}


class Run:
    def __init__(self, awr, output):
        self.awr, self.output = awr, output
        self.steps = []
        self.scenarios = []

    def command(self, argv, cwd, expected=0):
        index = len(self.steps)
        started = time.monotonic()
        result = subprocess.run(list(map(str, argv)), cwd=cwd, capture_output=True, timeout=60)
        stdout = self.output / f'{index:03d}.stdout'
        stderr = self.output / f'{index:03d}.stderr'
        stdout.write_bytes(result.stdout)
        stderr.write_bytes(result.stderr)
        self.steps.append(dict(command=list(map(str, argv)), cwd=str(cwd),
                               exit_code=result.returncode, expected_exit=expected,
                               elapsed_seconds=time.monotonic() - started,
                               stdout=stdout.name, stdout_sha256=digest(stdout),
                               stderr=stderr.name, stderr_sha256=digest(stderr)))
        require(result.returncode == expected,
                f'{argv}: expected {expected}, got {result.returncode}: '
                f'{result.stdout.decode("utf-8", "replace")} {result.stderr.decode("utf-8", "replace")}')
        return result

    def scenario(self, name, action):
        start = len(self.steps)
        try:
            details = action() or {}
            self.scenarios.append(dict(id=name, passed=True, step_start=start,
                                       step_end=len(self.steps), details=details))
        except Exception as error:
            self.scenarios.append(dict(id=name, passed=False, step_start=start,
                                       step_end=len(self.steps), error=str(error)))
            (self.output / f'{name}.failure.txt').write_text(traceback.format_exc(), encoding='utf-8')


class Project:
    def __init__(self, run, name):
        self.run = run
        self.root = run.output / name
        self.root.mkdir()
        (self.root / 'artifacts').mkdir()
        (self.root / 'reports').mkdir()
        self.ledger = self.root / 'work.yaml'
        self.ledger.write_text('goals:\n- id: G\n  title: Deliver both artifacts\n'
                              '  status: active\n  summary: Synthetic local verification request\n'
                              '  success_criteria: [Both artifacts are delivered]\nwork_items:\n' + ''.join(
            f'- id: {key}\n  title: Deliver {key}\n  goal: G\n  status: in_progress\n'
            f'  next_action: Verify {key}\n  paths: [artifacts/{key}.txt]\n'
            f'  acceptance: [{key} artifact bytes match expected output]\n'
            for key in ['A', 'B']), encoding='utf-8')
        (self.root / 'sources.toml').write_text(
            "[project]\nname='Evidence currency POC'\ncontext_profile='minimal'\n"
            "[[sources]]\ndomain='ledger'\nrole='primary'\npath='work.yaml'\nadapter='yaml-ledger-v1'\n",
            encoding='utf-8')
        (self.root / 'verify.py').write_text(CHECKER, encoding='utf-8')
        for key in ['A', 'B']:
            (self.root / 'artifacts' / f'{key}.txt').write_bytes(f'{key} delivered\n'.encode('utf-8'))
        self.cli('init', '--manifest', 'sources.toml', '--accept')

    def cli(self, *args, error=None):
        result = self.run.command([self.run.awr, '--project', self.root, '--json', *args],
                                  self.root, expected=1 if error else 0)
        value = json.loads(result.stderr if error else result.stdout)
        if error:
            require(value['code'] == error, f'Expected {error}, got {value}')
        return value

    def revision(self):
        return str(self.cli('proposal', 'list')['project_revision'])

    def start(self, work, claim=True):
        args = ['session', 'start', '--work', work, '--agent', f'worker-{work}',
                '--provider', 'synthetic-poc', '--model', 'none',
                '--expected-revision', self.revision()]
        return self.cli(*args, *(['--claim'] if claim else []))['session']

    def prepare(self, work, key, sha=None):
        sha = sha or digest(self.ledger)
        relative = f'reports/{key}.json'
        self.run.command([sys.executable, 'verify.py', '--work', work, '--sha', sha,
                          '--output', relative], self.root)
        report = json.loads((self.root / relative).read_text(encoding='utf-8'))
        require(report['checks'][0]['passed'], 'The real checker did not pass')
        prepared = self.cli('work', 'prepare-completion', work, '--report', relative,
                            '--evidence-key', key, '--source-sha', sha, '--level', 'locally_verified')
        return prepared

    def add(self, prepared, expected=None, error=None):
        key = prepared['evidence']['external_key']
        path = self.root / f'{key}.evidence.json'
        path.write_text(json.dumps(prepared['evidence']), encoding='utf-8')
        return self.cli('evidence', 'add', '--input', str(path), '--expected-revision',
                        expected or self.revision(), error=error)

    def complete(self, work, session, prepared):
        path = self.root / f'{work}.completion.json'
        path.write_text(json.dumps(prepared['completion']), encoding='utf-8')
        return self.cli('work', 'complete', work, '--session', session['id'], '--reason',
                        'Observed the actual synthetic checker result', '--input', str(path),
                        '--expected-revision', self.revision())

    def evidence(self, work, key, sha=None):
        view = self.cli('work', 'show', work, '--source-sha', sha or digest(self.ledger))
        return next(e for e in view['evidence'] if e['external_key'] == key)

    def verified(self):
        return self.cli('intake', 'inspect', '--source-sha', digest(self.ledger))['organization']['verified_completed']


def same_project(run):
    project = Project(run, 'same project 中文')
    sessions = {key: project.start(key) for key in ['A', 'B']}
    prepared = {key: project.prepare(key, f'{key}-proof') for key in ['A', 'B']}
    original = digest(project.ledger)
    reports = {key: digest(project.root / prepared[key]['evidence']['locator']) for key in prepared}
    for value in prepared.values():
        recorded = project.add(value)
        require(bool(recorded['evidence'].get('section_fingerprint')), 'Fresh ledger evidence was not stamped')
    for key in ['A', 'B']:
        project.complete(key, sessions[key], prepared[key])
    require(digest(project.ledger) != original, 'Completion did not change the ledger')
    require(project.verified() == 2, 'Both completed artifacts must remain verified')
    for key in ['A', 'B']:
        require(project.evidence(key, f'{key}-proof')['currency'] == 'current', 'Work-aware currency lost isolation')
        require(digest(project.root / prepared[key]['evidence']['locator']) == reports[key], 'Report bytes changed')
        generic = project.cli('evidence', 'show', f'{key}-proof', '--source-sha', digest(project.ledger))
        require(generic['currency'] == 'historical', 'Generic evidence reads must remain SHA-strict')
    path = project.root / prepared['A']['evidence']['locator']
    original_report = path.read_bytes()
    # Preserve valid, passing JSON so only byte-integrity validation rejects it.
    changed_report = json.loads(original_report)
    changed_report['checks'][0]['details'] = 'A different passing observation'
    path.write_text(json.dumps(changed_report, indent=2), encoding='utf-8')
    require(project.verified() == 1, 'Changed selected report bytes must invalidate one completion')
    path.write_bytes(original_report)
    require(project.verified() == 2, 'Restoring exact report bytes must restore verification')
    project.ledger.write_text(project.ledger.read_text(encoding='utf-8').replace(
        'A artifact bytes match expected output', 'A artifact needs a new review'), encoding='utf-8')
    require(project.verified() == 1, 'Changed acceptance must invalidate old proof')
    return dict(independent_sessions=True, report_bytes_unchanged=True, actual_checker_executed=True)


def separate_projects(run):
    left, right = Project(run, 'node A'), Project(run, 'node B')
    sa, sb = left.start('A'), right.start('B')
    require(sa['project_id'] != sb['project_id'], 'Nodes must have independent runtime identities')
    require(digest(left.ledger) == digest(right.ledger), 'Initial source bytes must match')
    pa, pb = left.prepare('A', 'node-A-proof'), right.prepare('B', 'node-B-proof')
    left.add(pa)
    right.add(pb)
    left.complete('A', sa, pa)
    transfers = []

    def transfer(producer, consumer, relative):
        parts = Path(relative).parts
        require(relative == 'work.yaml' or parts[0] == 'reports', 'Only source/report bytes may be transported')
        shutil.copyfile(producer.root / relative, consumer.root / relative)
        require(digest(producer.root / relative) == digest(consumer.root / relative), 'Transported bytes differ')
        transfers.append(dict(path=relative, sha256=digest(consumer.root / relative)))

    transfer(left, right, 'work.yaml')
    right.cli('source', 'reindex')
    require(right.evidence('B', 'node-B-proof')['currency'] == 'current', 'Other node ledger update invalidated B')
    right.complete('B', sb, pb)
    transfer(right, left, 'work.yaml')
    left.cli('source', 'reindex')
    require(left.evidence('A', 'node-A-proof')['currency'] == 'current', 'Returned ledger invalidated local A proof')
    require(left.verified() == 1 and right.verified() == 1, 'Source completion must not fabricate missing node proof')
    # Transport only source/report bytes, never SQLite, local IDs, claims or stamps.
    locator = pa['evidence']['locator']
    transfer(left, right, locator)
    imported = right.add(pa)
    require(not imported['evidence'].get('section_fingerprint'), 'Old imported proof acquired a current stamp')
    require(right.evidence('A', 'node-A-proof')['currency'] == 'historical', 'Old imported proof was silently promoted')
    require(right.verified() == 1, 'Imported old proof must not count as a fresh local verification')
    local = right.start('A', claim=False)
    require(local['project_id'] == sb['project_id'], 'Transport replaced the consumer runtime identity')
    right.cli('work', 'reopen', 'A', '--session', local['id'], '--reason',
              'Explicitly verify imported delivery on this node', '--next-action',
              'Run the local artifact check', '--expected-revision', right.revision())
    right.cli('work', 'claim', 'A', '--session', local['id'], '--expected-revision', right.revision())
    right.cli('work', 'progress', 'A', '--session', local['id'], '--reason',
              'Begin the explicit local verification', '--next-action', 'Check the actual artifact',
              '--expected-revision', right.revision())
    verification_start = len(run.steps)
    fresh = right.prepare('A', 'node-B-reverified-A')
    right.add(fresh)
    require(right.evidence('A', 'node-B-reverified-A')['currency'] == 'current', 'Actual fresh verification did not bind')
    completed = right.complete('A', local, fresh)
    require(completed['event']['event_type'] == 'work.completed', 'Fresh proof did not produce an accepted local completion')
    verified = right.verified()
    require(verified == 2, 'Both locally verified deliveries must survive the final ledger change')
    checker_steps = [step for step in run.steps[verification_start:]
                     if len(step['command']) > 1 and step['command'][1] == 'verify.py' and step['exit_code'] == 0]
    require(checker_steps, 'Fresh verification did not execute a checker process')
    return dict(project_ids=[sa['project_id'], sb['project_id']], transported_files=transfers,
                database_copied=any('.awr' in Path(item['path']).parts for item in transfers),
                old_report_automatically_stamped=bool(imported['evidence'].get('section_fingerprint')),
                fresh_checker_executed=bool(checker_steps), fresh_completion_event=completed['event']['event_type'],
                verified_completed=verified)


def interleaved_refresh(run):
    project = Project(run, 'source race')
    session = project.start('A')
    prepared = project.prepare('A', 'before-edit')
    revision = str(prepared['project_revision'])
    project.ledger.write_text(project.ledger.read_text(encoding='utf-8').replace(
        'title: Deliver B', 'title: Deliver revised B'), encoding='utf-8')
    human_hash = digest(project.ledger)
    project.add(prepared, expected=revision, error='RevisionConflict')
    require(digest(project.ledger) == human_hash, 'Conflicting registration overwrote the user source')
    project.cli('evidence', 'show', 'before-edit', error='NotFound')
    retried = project.add(prepared)
    require(not retried['evidence'].get('section_fingerprint'), 'Reviewed retry stamped an older snapshot')
    require(project.evidence('A', 'before-edit')['currency'] == 'historical', 'Old retry became Current')
    fresh = project.prepare('A', 'after-edit')
    project.add(fresh)
    project.complete('A', session, fresh)
    require(project.verified() == 1, 'Fresh re-verification after conflict did not complete')
    return dict(interleaving='preflight -> external source edit -> record',
                rejected_write_had_no_evidence_side_effect=True, source_bytes_preserved=True)


def actual_git_revision(run):
    project = Project(run, 'git source')
    hooks = project.root / 'empty-hooks'
    hooks.mkdir()
    git = ['git', '-c', 'user.name=Evidence fixture', '-c', 'user.email=fixture@example.invalid',
           '-c', f'core.hooksPath={hooks}', '-c', 'commit.gpgSign=false', '-c', 'core.fsmonitor=false']
    run.command([*git, 'init', '-q'], project.root)
    files = ['work.yaml', 'sources.toml', 'verify.py', 'artifacts/A.txt']
    run.command([*git, 'add', *files], project.root)
    run.command([*git, 'commit', '-qm', 'Record verified artifact'], project.root)
    old = run.command([*git, 'rev-parse', 'HEAD'], project.root).stdout.decode().strip()
    prepared = project.prepare('A', 'git-proof', old)
    recorded = project.add(prepared)
    require(not recorded['evidence'].get('section_fingerprint'), 'Git revision was confused with a ledger hash')
    require(project.evidence('A', 'git-proof', old)['currency'] == 'current', 'Original Git binding is not Current')
    ledger_hash = digest(project.ledger)
    (project.root / 'artifacts/A.txt').write_text('changed output\n', encoding='utf-8')
    run.command([*git, 'add', 'artifacts/A.txt'], project.root)
    run.command([*git, 'commit', '-qm', 'Change artifact without editing task contract'], project.root)
    new = run.command([*git, 'rev-parse', 'HEAD'], project.root).stdout.decode().strip()
    require(old != new and digest(project.ledger) == ledger_hash, 'Git fixture did not isolate the code change')
    require(project.evidence('A', 'git-proof', new)['currency'] == 'historical', 'Matching contract hid a changed Git revision')
    return dict(verification_revision=old, changed_revision=new, task_ledger_unchanged=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--awr', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    awr, output = args.awr.resolve(), args.output.resolve()
    if not awr.is_file():
        parser.error('--awr must identify an existing executable')
    if output == REPO or REPO in output.parents:
        ignored = subprocess.run(['git', 'check-ignore', '-q', str(output)], cwd=REPO)
        if ignored.returncode != 0:
            parser.error('--output inside the repository must be ignored by Git')
    output.mkdir(parents=True, exist_ok=False)
    before = inputs()
    run = Run(awr, output)
    binary_before = digest(awr)
    for name, action in [('cli_full_flow', same_project), ('cross_instance_transport', separate_projects),
                         ('stale_revision_retry', interleaved_refresh), ('git_revision_strict', actual_git_revision)]:
        run.scenario(name, lambda action=action: action(run))
    after = inputs()
    report = dict(version=1, passed=all(s['passed'] for s in run.scenarios),
                  platform=dict(system=platform.system(), machine=platform.machine(), python=platform.python_version()),
                  verification_kind='synthetic_cli_process_poc',
                  source_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
                  source_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=REPO)),
                  input_hashes=before, inputs_unchanged=before == after,
                  binary_sha256=binary_before, binary_unchanged=binary_before == digest(awr),
                  binary_identity=binary_identity(awr),
                  scenarios=run.scenarios, steps=run.steps,
                  limitations=['No agent model call, automatic host hook or business acceptance.',
                               'The cross-instance transport is explicit local byte copying, not a cloud-backend test.',
                               'Interleaving is deterministic; simultaneous writer races are separate regression tests.'])
    report['passed'] &= report['inputs_unchanged'] and report['binary_unchanged']
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(passed=report['passed'], scenarios=run.scenarios, report=str(output / 'report.json'))))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
