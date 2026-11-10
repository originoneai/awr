"""Positive and negative contracts for executable documentation."""
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import run_doc_commands as runner

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tests/platform'))
import verify

AWR = Path(os.environ.get('AWR_DOC_TEST_BIN', ROOT / 'target/debug/awr'))


class DocCommandTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='awr-doc-tests-')
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def document(self, text, name='guide.md'):
        path = self.root / name
        path.write_text(text, encoding='utf-8')
        return path

    def run_docs(self, documents, **options):
        with contextlib.redirect_stdout(io.StringIO()):
            return runner.run_documents(AWR, documents, **options)

    def test_marked_commands_succeed_in_both_guides_within_sixty_seconds(self):
        started = time.monotonic()
        result = self.run_docs(runner.DOCUMENTS)
        self.assertEqual(result['documents'], 2)
        self.assertGreaterEqual(result['blocks'], 20)
        self.assertLess(time.monotonic() - started, 60)

    def test_invalid_document_flag_reports_file_and_command_line(self):
        source = runner.DOCUMENTS[0].read_text(encoding='utf-8')
        self.assertIn('context compile --work', source)
        changed = source.replace('context compile --work', 'context compile --wrok', 1)
        path = self.document(changed, 'broken-quickstart.md')
        line = next(i for i, text in enumerate(changed.splitlines(), 1) if '--wrok' in text)
        with self.assertRaisesRegex(runner.DocCommandError, rf'{path}:{line}:'):
            self.run_docs([path])

    def test_variables_functions_and_jq_values_survive_between_blocks(self):
        path = self.document('''<!-- doc-test: capture -->
```sh
VALUE=$(printf '{"session":{"id":"saved"}}' | jq -er '.session.id')
show_value() { printf '%s\\n' "$VALUE"; }
```
<!-- doc-test: reuse -->
```bash
show_value
```
<!-- expect: saved -->
''')
        self.assertEqual(self.run_docs([path])['blocks'], 2)

    def test_expectation_mismatch_fails_at_the_document(self):
        path = self.document('<!-- doc-test: output -->\n```sh\nprintf actual\n```\n<!-- expect: expected -->\n')
        with self.assertRaisesRegex(runner.DocCommandError, r'guide.md:3:.*expected'):
            self.run_docs([path])

    def test_failed_pipeline_and_earlier_command_stop_the_block(self):
        for command in ('false | cat', 'false\nprintf masked', 'VALUE=$(false; printf masked)'):
            with self.subTest(command=command):
                path = self.document(f'<!-- doc-test: failure -->\n```sh\n{command}\n```\n')
                with self.assertRaisesRegex(runner.DocCommandError, r'guide.md:3:'):
                    self.run_docs([path])

    def test_successful_early_shell_exit_cannot_skip_later_commands(self):
        path = self.document('<!-- doc-test: early-exit -->\n```sh\nexit 0\n```\n'
                             '<!-- doc-test: skipped -->\n```sh\nprintf skipped\n```\n')
        with self.assertRaisesRegex(runner.DocCommandError, 'before every doc-test block completed'):
            self.run_docs([path])

    def test_unmarked_commands_and_markers_inside_fences_are_ignored(self):
        path = self.document('''```sh
exit 8
<!-- doc-test: example-only -->
```
<!-- doc-test: real -->
```sh
printf real
```
<!-- expect: real -->
''')
        self.assertEqual(self.run_docs([path])['blocks'], 1)

    def test_missing_empty_or_unclosed_marked_blocks_fail(self):
        for text in ('# No markers', '<!-- doc-test: missing -->',
                     '<!-- doc-test: empty -->\n```sh\n```',
                     '<!-- doc-test: unclosed -->\n```sh\nprintf incomplete'):
            with self.subTest(text=text), self.assertRaises(runner.DocCommandError):
                self.run_docs([self.document(text)])

    def test_overall_deadline_is_shared_by_documents_and_reaps_children(self):
        marker = self.root / 'escaped'
        first = self.document('<!-- doc-test: first -->\n```sh\nsleep 0.2\n```\n', 'first.md')
        second = self.document('<!-- doc-test: second -->\n```sh\n'
                               f'(sleep 0.8; touch "{marker}") &\nsleep 5\n```\n', 'second.md')
        started = time.monotonic()
        with self.assertRaisesRegex(runner.DocCommandError, r'timeout.*overall'):
            self.run_docs([first, second], timeout_seconds=0.65)
        self.assertLess(time.monotonic() - started, 1.5)
        time.sleep(0.9)
        self.assertFalse(marker.exists(), 'The timed-out documentation left a child running')

    def test_default_sixty_second_budget_rejects_an_overrun(self):
        self.assertEqual(runner.TIMEOUT_SECONDS, 60)
        path = self.document('<!-- doc-test: late -->\n```sh\nprintf late\n```\n')
        with patch.object(runner.time, 'monotonic', side_effect=[0, 61]), \
                self.assertRaisesRegex(runner.DocCommandError, 'timeout.*overall'):
            self.run_docs([path])

    def test_cli_returns_nonzero_for_a_broken_document(self):
        path = self.document('<!-- doc-test: failure -->\n```sh\nawr --bad-document-flag\n```\n')
        result = subprocess.run([sys.executable, str(runner.__file__), '--awr', str(AWR), '--doc', str(path)],
                                capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f'{path}:3:', result.stderr)


class PlatformRegistrationTests(unittest.TestCase):
    def test_both_guides_trigger_native_ci_on_push_and_pull_request(self):
        workflow = (ROOT / '.github/workflows/platform.yml').read_text()
        for path in ('docs/series/quickstart.md', 'docs/series/daily-workflow.md'):
            self.assertEqual(workflow.count(f"- '{path}'"), 2)

    def run_platform(self, platform_id, contract):
        with tempfile.TemporaryDirectory(prefix='awr-doc-platform-') as directory:
            root = Path(directory).resolve()
            contract_path = root / 'contract.json'
            contract_path.write_text(json.dumps(contract), encoding='utf-8')
            selected = next(p for p in contract['platforms'] if p['id'] == platform_id)
            metadata = {'packages': [{'name': name, 'targets': []} for name in contract['workspace_members']],
                        'target_directory': str(root / 'target')}
            observations = {('git', 'status', '--porcelain'): '', ('git', 'rev-parse', 'HEAD'): 'a' * 40,
                            ('git', 'rev-parse', 'HEAD^{tree}'): 'b' * 40, ('git', 'ls-files', '-z'): '',
                            ('rustc', '-vV'): f'release: 1.93.1\nhost: {selected["rust_host"]}',
                            ('cargo', '--version'): 'synthetic cargo',
                            ('cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'): json.dumps(metadata)}

            def execute(command, **options):
                Path(options['log']).write_text('', encoding='utf-8')
                return dict(passed=True, exit_code=0, cleanup_complete=True, log_finalized=True)

            output = root / '.local/result'
            with patch.object(verify, 'ROOT', root), patch.object(verify, 'CONTRACT', contract_path), \
                    patch.object(verify, 'read_command', side_effect=lambda *args: observations[args]), \
                    patch.object(verify.platform, 'system', return_value=selected['system']), \
                    patch.object(verify.platform, 'machine', return_value=selected['machine']), \
                    patch.object(verify, 'run_command', side_effect=execute), \
                    patch.dict(os.environ, {'GITHUB_SHA': '', 'CARGO_BUILD_TARGET': ''}), \
                    patch.object(sys, 'argv', ['verify.py', '--platform', platform_id, '--output', str(output)]), \
                    contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                code = verify.main()
            return code, json.loads((output / 'evidence/report.json').read_text())

    def test_linux_x64_executes_the_registered_documentation_gate(self):
        contract = json.loads(verify.CONTRACT.read_text())
        _, report = self.run_platform('linux-x64', contract)
        gate = next(g for g in report['gates'] if g['id'] == 'doc_commands')
        self.assertTrue(gate['passed'], gate)
        self.assertIn('tests/docs/run_doc_commands.py', gate['command'])
        self.assertIn('--self-test', gate['command'])
        self.assertIn('doc_commands', contract['platform_required_gates']['linux-x64'])

    def test_missing_contract_registration_makes_verify_fail(self):
        contract = json.loads(verify.CONTRACT.read_text())
        contract['platform_required_gates'] = {}
        code, report = self.run_platform('linux-x64', contract)
        self.assertEqual(code, 1)
        self.assertIn('doc_commands', report['gates'][0].get('error', ''))

    def test_other_native_platforms_exclude_documentation_execution(self):
        contract = json.loads(verify.CONTRACT.read_text())
        for platform_id in ('macos-arm64', 'macos-x64', 'linux-arm64', 'windows-x64'):
            with self.subTest(platform=platform_id):
                _, report = self.run_platform(platform_id, contract)
                self.assertNotIn('doc_commands', [g['id'] for g in report['gates']])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--awr', type=Path, required=True)
    args = parser.parse_args()
    AWR = args.awr.resolve(strict=True)
    unittest.main(argv=[sys.argv[0]], verbosity=2)
