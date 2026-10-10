#!/usr/bin/env python3
"""Run a source-bound native platform gate; never infer another platform's result."""
import argparse
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time

from process_runner import run_command

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / 'tests/platform/contract.json'
PREFIX = ['rtk', 'proxy'] if shutil.which('rtk') else []


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_command(*command):
    return subprocess.check_output(PREFIX + list(command), cwd=ROOT, text=True, encoding='utf-8').strip()


def now():
    return datetime.now().astimezone().isoformat(timespec='seconds')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    contract = json.loads(CONTRACT.read_text(encoding='utf-8'))
    selected = next((p for p in contract['platforms'] if p['id'] == args.platform), None)
    require(selected is not None, 'Select an exact platform from the contract')
    output = args.output.resolve()
    output.relative_to(ROOT / '.local')
    output.mkdir(parents=True, exist_ok=False)
    evidence = output / 'evidence'
    evidence.mkdir()
    report_path = evidence / 'report.json'
    cleanup_failed = False
    report = {'contract_id': contract['contract_id'], 'contract_version': contract['version'],
              'contract_sha256': digest(CONTRACT), 'work_item': contract['work_item'],
              'platform_id': args.platform, 'started_at': now(), 'passed': False, 'gates': [],
              'e4_completed': 0, 'model_calls': 0, 'native_agent_client_invoked': False,
              'limitations': contract['limitations']}

    def save():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')

    def gate(name, command, validator=None, env=None, requires=()):
        nonlocal cleanup_failed
        unmet = [key for key in requires if not any(g['id'] == key and g['passed'] for g in report['gates'])]
        if cleanup_failed:
            unmet.append('process_cleanup')
        budget = selected.get('gate_timeout_seconds', {}).get(name, contract['command_timeout_seconds'])
        row = {'id': name, 'command': PREFIX + list(map(str, command)), 'passed': False,
               'started_at': now(), 'not_run': bool(unmet), 'timeout_seconds': budget,
               'cleanup_timeout_seconds': contract['cleanup_timeout_seconds']}
        report['gates'].append(row)
        save()
        if unmet:
            row['unmet_dependencies'] = unmet
            save()
            return
        print('Running native gate: ' + name, flush=True)
        log = evidence / (name + '.log')
        started = time.monotonic()
        process_env = dict(os.environ, PYTHONUTF8='1', CARGO_TERM_COLOR='never')
        process_env.update(env or {})
        try:
            row.update(run_command(row['command'], cwd=ROOT, log=log, env=process_env,
                                   timeout_seconds=budget, cleanup_seconds=contract['cleanup_timeout_seconds']))
            cleanup_failed = not row['cleanup_complete']
            if row['passed'] and validator:
                row.update(validator(log.read_text(encoding='utf-8', errors='replace')) or {})
        except Exception as error:
            row.update(passed=False, error=str(error))
            cleanup_failed = not row.get('cleanup_complete', False)
        row.update(finished_at=now(), duration_seconds=round(time.monotonic() - started, 3),
                   log=str(log.relative_to(ROOT)))
        if row.get('log_finalized') and log.is_file():
            row['log_sha256'] = digest(log)
        save()

    save()
    try:
        require(not read_command('git', 'status', '--porcelain'), 'Native verification needs a clean source commit')
        source = read_command('git', 'rev-parse', 'HEAD')
        require(not os.environ.get('GITHUB_SHA') or os.environ['GITHUB_SHA'] == source, 'Checkout and Actions source SHA differ')
        require(not os.environ.get('CARGO_BUILD_TARGET'), 'Do not cross-compile this native gate')
        machine = platform.machine().lower()
        machine = {'amd64': 'x86_64', 'aarch64': 'arm64'}.get(machine, machine)
        rust = read_command('rustc', '-vV')
        rust_fields = dict(line.split(': ', 1) for line in rust.splitlines() if ': ' in line)
        require(platform.system() == selected['system'] and machine == selected['machine'], 'Actual native OS/architecture differs from the requested platform')
        require(rust_fields['host'] == selected['rust_host'] and rust_fields['release'] == contract['rust_version'],
                'Rust host or pinned toolchain does not match')
        metadata = json.loads(read_command('cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'))
        require(set(p['name'] for p in metadata['packages']) == set(contract['workspace_members']), 'Workspace coverage changed')
        tracked = [name for name in read_command('git', 'ls-files', '-z').split('\0') if name]
        fingerprints = {name: digest(ROOT / name) for name in tracked}
        report.update(source_commit=source, source_tree=read_command('git', 'rev-parse', 'HEAD^{tree}'),
                      clean_source=True, tracked_input_sha256=fingerprints,
                      environment={'system': platform.system(), 'release': platform.release(), 'version': platform.version(),
                                   'machine': machine, 'rustc': rust, 'cargo': read_command('cargo', '--version'),
                                   'python': sys.version, 'sqlite': __import__('sqlite3').sqlite_version,
                                   'ci': {key: os.environ[key] for key in ['GITHUB_REPOSITORY', 'GITHUB_WORKFLOW', 'GITHUB_RUN_ID',
                                          'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA', 'RUNNER_OS', 'RUNNER_ARCH', 'RUNNER_NAME',
                                          'ImageOS', 'ImageVersion'] if key in os.environ}},
                      compiled_target_count=sum(len(p['targets']) for p in metadata['packages']))
        report['gates'].append({'id': 'environment', 'passed': True, 'native_host_verified': True})
        save()
    except Exception as error:
        report['gates'].append({'id': 'environment', 'passed': False, 'error': str(error)})
        save()
        print(str(error), file=sys.stderr)
        return 1
    target = Path(metadata['target_directory']) / 'debug'
    extension = '.exe' if platform.system() == 'Windows' else ''
    awr, mcp = target / ('awr' + extension), target / ('awr-mcp' + extension)
    gate('runner_tests', [sys.executable, 'tests/platform/test_process_runner.py'])
    gate('build', ['cargo', 'build', '--workspace', '--all-targets', '--locked'])
    if all(path.is_file() for path in [awr, mcp]):
        report['binary_sha256'] = {'awr': digest(awr), 'awr-mcp': digest(mcp)}

    def inventory(text):
        names = re.findall(r'^(.+): test$', text, re.M)
        require(sorted(names) == sorted(contract['ignored_tests']), 'Ignored entries changed; inspect their native execution routes')
        return {'ignored_tests': names, 'routes': contract['ignored_routes']}

    def rust_results(text):
        rows = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;', text)
        require(bool(rows) and sum(int(r[0]) for r in rows) > 0, 'No executed Rust tests in a successful command')
        return {'test_functions_passed': sum(int(r[0]) for r in rows),
                'test_functions_ignored': sum(int(r[2]) for r in rows), 'test_targets_reported': len(rows)}

    gate('ignored_inventory', ['cargo', 'test', '--workspace', '--all-targets', '--locked', '--', '--ignored', '--list'],
         inventory, requires=('build',))
    gate('workspace', ['cargo', 'test', '--workspace', '--all-targets', '--no-fail-fast', '--locked'],
         rust_results, requires=('build',))
    gate('doctests', ['cargo', 'test', '--workspace', '--doc', '--no-fail-fast', '--locked'],
         rust_results, requires=('build',))
    gate('crash_recovery', ['cargo', 'test', '-p', 'awr-runtime', '--lib', '--locked',
                          'mutation_apply::recovery_tests::cli_recovery_after_process_death_binds_original_attempt_and_resolves_once',
                          '--', '--ignored', '--exact', '--test-threads=1'], rust_results,
         env={'AWR_RECOVERY_CLI': str(awr.resolve())}, requires=('build', 'ignored_inventory'))

    def receipt(path, validate):
        def check(_):
            data = json.loads(path.read_text(encoding='utf-8'))
            extra = validate(data) or {}
            return {'receipt': str(path.relative_to(ROOT)), 'receipt_sha256': digest(path), **extra}
        return check

    parity = evidence / 'cli_mcp_parity.json'

    def validate_parity(data):
        require(data['passed'] and data['tests_run'] == 9 and len(data['tools_exercised']) == 9,
                'CLI/MCP parity did not execute all nine tool contracts')

    gate('cli_mcp_parity', [sys.executable, 'crates/awr-mcp/tests/cli_parity.py', '--awr', awr, '--mcp', mcp, '--report', parity],
         receipt(parity, validate_parity), requires=('build',))
    lifecycle = output / 'fixtures' / 'lifecycle'

    def validate_lifecycle(data):
        require(data['passed'] and data['source_unchanged'] and data['claim_settled'] and not data['model_invoked'],
                'Manual lifecycle did not preserve its source or settle the claim')
        copied = evidence / 'lifecycle'
        copied.mkdir()
        for path in lifecycle.glob('*.json'):
            shutil.copyfile(path, copied / path.name)
        return {'receipt': str((copied / 'summary.json').relative_to(ROOT)),
                'receipt_sha256': digest(copied / 'summary.json')}

    gate('cli_lifecycle', [sys.executable, 'examples/codex/lifecycle.py', '--awr', awr, '--output', lifecycle],
         receipt(lifecycle / 'summary.json', validate_lifecycle), requires=('build',))
    native = evidence / 'native_cli' / 'report.json'

    def validate_native(data):
        require(data['passed'] and set(data['conditions']) == set(contract['native_cli_conditions'])
                and all(data['conditions'].values()), 'Native CLI condition coverage is incomplete')

    gate('native_cli', [sys.executable, 'tests/platform/native_cli.py', '--awr', awr,
                        '--output', output / 'fixtures' / 'native', '--report', native],
         receipt(native, validate_native), requires=('build',))
    gate('workflow_oracle', [sys.executable, '-m', 'unittest', 'discover', '-s', 'tests/benchmarks/workflow', '-v'])
    workflow = output / 'workflow'

    def validate_workflow(data):
        require(data['workflow_runs'] == 10 and all(data['checks'].values())
                and data['runtime_source_sha'] == source
                and data['binary_sha256'] == {'awr': digest(awr), 'mcp': digest(mcp)},
                'Equivalent workflow cases or native binary binding are incomplete')
        require(all(value is None for value in data['missing_metrics'].values()),
                'Model usage cannot be inferred from synthetic protocol metrics')
        copied = evidence / 'workflow-aggregate.json'
        shutil.copyfile(workflow / 'aggregate.json', copied)
        return {'workflow_runs': data['workflow_runs'], 'aggregate': str(copied.relative_to(ROOT)),
                'aggregate_sha256': digest(copied)}

    gate('workflow_scenarios', [sys.executable, 'tests/benchmarks/workflow/run.py', '--awr', awr, '--mcp', mcp,
                              '--runtime-source-sha', source, '--output', workflow, '--repetitions', '1'],
         receipt(workflow / 'aggregate.json', validate_workflow), requires=('build', 'workflow_oracle'))
    gate('format', ['cargo', 'fmt', '--all', '--check'])
    unchanged = all((ROOT / name).is_file() and digest(ROOT / name) == value for name, value in fingerprints.items())
    unchanged = unchanged and read_command('git', 'rev-parse', 'HEAD') == source and not read_command('git', 'status', '--porcelain')
    report['gates'].append({'id': 'source_unchanged', 'passed': unchanged, 'tracked_files': len(fingerprints)})
    gate_ids = [row['id'] for row in report['gates']]
    report.update(passed=sorted(gate_ids) == sorted(contract['required_gates']) and all(g['passed'] for g in report['gates']),
                  gates_passed=sum(g['passed'] for g in report['gates']), target_gates=len(contract['required_gates']),
                  finished_at=now(), inputs_unchanged=unchanged)
    save()
    print(json.dumps({key: report[key] for key in ['platform_id', 'source_commit', 'passed', 'gates_passed', 'target_gates', 'e4_completed']}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
