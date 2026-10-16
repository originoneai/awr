import copy
import json
from pathlib import Path
import sys
import time
import unittest
from unittest.mock import patch

from host import CommandFailed, Result
from execution_reports import ExecutionReports
import test_workflow


class ExecutionReportTest(unittest.TestCase):
    setUp = test_workflow.WorkflowTest.setUp
    open_workflow = test_workflow.WorkflowTest.open_workflow
    revision = test_workflow.WorkflowTest.revision
    begin = test_workflow.WorkflowTest.begin

    def start(self, body=None):
        self.wf.acknowledge(self.begin())
        script = self.root / 'verify guide.py'
        script.write_text(body or "from pathlib import Path\nPath('guide result.txt').write_text('Reviewed guide\\n')\nprint('verified guide')\n")
        return dict(key='verify-guide', purpose='Verify the guide and retain its output',
                    command=[sys.executable, str(script)], source_paths=[script.name],
                    artifact_paths=['guide result.txt'])

    def collect(self):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            value = self.wf.collect_run('verify-guide')
            if value['state'] in ('succeeded', 'failed'):
                return value
            time.sleep(.05)
        self.fail('Managed execution did not reach a terminal state')

    def review(self, **kwargs):
        return self.wf.prepare_report('verify-guide',
            kwargs.get('checks', [dict(name='Guide review', passed=True, details='Read the actual guide result and checked its content', criteria=['Reviewed guide'])]),
            kwargs.get('reviewer', 'fixture reviewer'), 'GUIDE-EXEC')

    def test_actual_execution_collect_review_preflight_and_finish(self):
        request = self.start()
        created = self.wf.run(**request)
        self.assertTrue(created['created'])
        collected = self.collect()
        self.assertTrue(collected['eligible'])
        self.assertEqual(collected['execution']['intent']['command'], request['command'])
        self.assertEqual(collected['observation']['exit_code'], 0)
        self.assertEqual((self.root/'guide result.txt').read_text(), 'Reviewed guide\n')
        # Calling the same key only queries; it never dispatches again.
        with patch.object(self.wf.host, 'call', wraps=self.wf.host.call) as calls:
            self.wf.run(**request)
        self.assertFalse(any(c.args[:2] == ('execution', 'run') for c in calls.call_args_list))
        with self.assertRaises(ValueError): self.review(checks=[])
        with self.assertRaises(ValueError): self.review(reviewer='')
        with self.assertRaises(ValueError): self.review(checks=[dict(passed=False)])
        with self.assertRaises(CommandFailed):
            self.review(checks=[dict(name='Other', passed=True, details='Unrelated', criteria=['Other criterion'])])
        self.assertEqual(self.wf.host.ok('work', 'show', 'W')['work']['status'], 'in_progress')
        reviewed = self.review()
        self.assertFalse(reviewed['completion_claimed'])
        self.assertFalse(reviewed['preflight']['verification_executed'])
        self.assertEqual(reviewed['preflight']['evidence']['work_item_key'], 'W')
        self.assertIn('branch_id', reviewed['preflight']['evidence'])
        self.assertNotIn('work', reviewed['preflight']['evidence'])
        self.assertNotIn('branch', reviewed['preflight']['evidence'])
        body = json.loads(Path(reviewed['report']['path']).read_text())
        self.assertEqual(body['source_sha'], collected['source_sha'])
        self.assertEqual(body['execution_receipt']['file_hashes'], collected['file_hashes'])
        done = self.wf.finish_report(reviewed['report']['id'], 'Reviewed the guide and its true execution receipt')
        self.assertEqual(done['session']['status'], 'ended')
        self.assertEqual(self.wf.host.ok('work', 'show', 'W')['work']['status'], 'completed')

    def test_lost_dispatch_is_inspected_without_reexecution(self):
        request = self.start()
        original = self.wf.host.call
        def lose(*args, **kwargs):
            result = original(*args, **kwargs)
            if args[:2] == ('execution', 'run'):
                result.require()
                return Result(None, b'', b'', result.receipt, True)
            return result
        with patch.object(self.wf.host, 'call', side_effect=lose):
            with self.assertRaises(CommandFailed): self.wf.run(**request)
        self.wf = self.open_workflow('workflow/state.json')
        with self.assertRaises(ValueError): self.wf.run(**request)
        value = self.collect()  # A read can discover the original saved key while a write is pending.
        self.assertTrue(value['eligible'])
        with self.assertRaises(ValueError): self.review()
        view = self.wf.inspect()
        self.assertEqual(view['executions']['verify-guide']['execution']['id'], value['execution']['id'])
        self.wf.reconcile(view['inspection']['sha256'], 'Observed the original managed result; no redispatch')
        self.wf.run(**request)
        self.assertEqual(len(self.wf.host.ok('execution', 'list', '--work', 'W')['executions']), 1)
        self.wf.finish_report(self.review()['report']['id'], 'Reviewed recovered real guide evidence')

    def test_failed_run_and_missing_output_cannot_produce_completion(self):
        request = self.start("import sys\nprint('failed guide review')\nsys.exit(3)\n")
        self.wf.run(**request)
        observed = self.collect()
        self.assertEqual(observed['state'], 'failed')
        self.assertFalse(observed['eligible'])
        with self.assertRaises(ValueError): self.review()
        self.assertEqual(self.wf.host.ok('work', 'show', 'W')['work']['status'], 'in_progress')

    def test_success_without_requested_artifact_is_ineligible(self):
        request = self.start("print('no guide produced')\n")
        self.wf.run(**request)
        with self.assertRaises(FileNotFoundError): self.collect()
        with self.assertRaises(FileNotFoundError): self.review()

    def test_sources_and_outputs_are_rechecked_without_replacing_collection(self):
        request = self.start()
        self.wf.run(**request)
        collected = self.collect()
        record = self.wf.state['runs']['verify-guide']['collection'].copy()
        reviewed = self.review()
        for target in [request['source_paths'][0], 'guide result.txt', collected['observation']['stdout'],
                       collected['observation']['receipt'], record['path'], reviewed['report']['path']]:
            with self.subTest(target=target):
                path = self.root / target
                original = path.read_bytes()
                path.write_bytes(original + b' changed')
                try:
                    with self.assertRaises(ValueError): self.wf.finish_report(reviewed['report']['id'], 'Attempt after tampering')
                    self.assertEqual(self.wf.state['runs']['verify-guide']['collection'], record)
                finally:
                    path.write_bytes(original)
        self.wf.finish_report(reviewed['report']['id'], 'Reviewed intact restored original bytes')

    def test_unknown_live_run_and_source_change_during_execution(self):
        request = self.start("from pathlib import Path\nimport time\nwhile not Path('continue').exists(): time.sleep(.05)\nPath('guide result.txt').write_text('Guide')\n")
        self.wf.run(**request)
        try:
            observed = self.wf.collect_run('verify-guide')
            self.assertFalse(observed['eligible'])
            with self.assertRaises(ValueError): self.review()
            script = self.root / request['source_paths'][0]
            script.write_text(script.read_text() + '# changed after dispatch\n')
        finally:
            (self.root/'continue').touch()
        with self.assertRaises(ValueError): self.collect()
        with self.assertRaises(ValueError): self.wf.run(**request)

    def test_lost_evidence_registration_reuses_exact_record(self):
        request = self.start()
        self.wf.run(**request); self.collect()
        reviewed = self.review(checks=[dict(name='Detailed guide review', passed=True,
            details='Synthetic detailed review of guide examples. ' * 1800,
            criteria=['Reviewed guide'])])
        self.assertGreater(Path(reviewed['report']['path']).stat().st_size, 65536)
        original = self.wf.host.call
        def lose(*args, **kwargs):
            result = original(*args, **kwargs)
            if args[:2] == ('evidence', 'add'):
                result.require()
                return Result(None, b'', b'', result.receipt, True)
            return result
        with patch.object(self.wf.host, 'call', side_effect=lose):
            with self.assertRaises(CommandFailed): self.wf.finish_report(reviewed['report']['id'], 'Review')
        self.wf = self.open_workflow('workflow/state.json')
        view = self.wf.inspect()
        self.wf.reconcile(view['inspection']['sha256'], 'Inspected saved evidence registration')
        with patch.object(self.wf.host, 'call', wraps=self.wf.host.call) as calls:
            try:
                self.wf.finish_report(reviewed['report']['id'], 'Reviewed saved evidence')
            except CommandFailed as error:
                self.fail(str(error.result.error))
        self.assertFalse(any(c.args[:2] == ('evidence', 'add') for c in calls.call_args_list))

    def test_scope_key_and_legacy_capability_guards(self):
        request = self.start()
        self.wf.capabilities.discard('execution.managed')
        with self.assertRaises(ValueError): self.wf.run(**request)
        self.wf.capabilities.add('execution.managed')
        with self.assertRaises(ValueError): self.wf.run(**dict(request, source_paths=[]))
        with self.assertRaises(ValueError): self.wf.run(**dict(request, artifact_paths=['../escaped.txt']))
        (self.root/'guide result.txt').write_text('stale guide')
        with self.assertRaises(ValueError): self.wf.run(**request)
        self.assertEqual(self.wf.host.ok('execution', 'list', '--work', 'W')['executions'], [])

    def test_continuous_run_survives_host_reopen_without_context_reuse(self):
        request = self.start("from pathlib import Path\nimport time\nwhile not Path('continue').exists(): time.sleep(.05)\nPath('guide result.txt').write_text('Guide')\n")
        prepared = self.wf.prepare(test_workflow.WorkflowTest.observation(self, no_deferred_wait=False))
        self.assertEqual(prepared['management']['decision']['mode'], 'continuous')
        self.assertIn('Reviewed guide', prepared['context']['work_context']['rendered_context'])
        self.wf.acknowledge(prepared['context']['work_context']['context_hash'])
        self.wf.run(**request)
        try:
            self.wf = self.open_workflow('workflow/state.json')
            self.assertFalse(self.wf.collect_run('verify-guide')['eligible'])
        finally:
            (self.root/'continue').touch()
        self.assertTrue(self.collect()['eligible'])
        self.wf.finish_report(self.review()['report']['id'], 'Reviewed continuous work after host reopen')

    def test_lost_completion_only_ends_session_after_explicit_reconciliation(self):
        request = self.start()
        request['source_paths'].append('work.yaml')
        self.wf.run(**request); self.collect()
        reviewed = self.review()
        original = self.wf.host.call
        def lose(*args, **kwargs):
            result = original(*args, **kwargs)
            if args[:2] == ('work', 'complete'):
                result.require()
                return Result(None, b'', b'', result.receipt, True)
            return result
        with patch.object(self.wf.host, 'call', side_effect=lose):
            with self.assertRaises(CommandFailed): self.wf.finish_report(reviewed['report']['id'], 'Review')
        self.wf = self.open_workflow('workflow/state.json')
        view = self.wf.inspect()
        self.assertEqual(view['work']['work']['status'], 'completed')
        self.wf.reconcile(view['inspection']['sha256'], 'Confirmed completed work; end the existing session only')
        with patch.object(self.wf.host, 'call', wraps=self.wf.host.call) as calls:
            self.wf.finish_report(reviewed['report']['id'], 'End reviewed work')
        self.assertEqual([c.args[:2] for c in calls.call_args_list], [('session', 'end')])

    def test_receipt_must_match_native_completion_before_first_collection(self):
        request = self.start()
        execution = self.wf.run(**request)['execution']['id']
        deadline = time.monotonic() + 20
        observed = {}
        while time.monotonic() < deadline:
            observed = self.wf.host.ok('execution', 'inspect', execution)['observation']
            if observed['state'] == 'succeeded':
                break
            time.sleep(.05)
        self.assertEqual(observed['state'], 'succeeded')
        path = self.root / observed['receipt']
        value = json.loads(path.read_text())
        value['finished_at'] += 1
        path.write_text(json.dumps(value))
        with self.assertRaises(ValueError): self.wf.collect_run('verify-guide')
        self.assertNotIn('collection', self.wf.state['runs']['verify-guide'])

    def test_directory_identity_accepts_equivalent_native_path_but_rejects_other_root(self):
        request = self.start()
        execution = self.wf.run(**request)['execution']
        self.collect()
        run = self.wf.state['runs']['verify-guide']
        alias = copy.deepcopy(execution)
        alias['intent']['cwd'] = str(self.root) + '/.'
        ExecutionReports(self.wf).identity(run, alias)
        alias['intent']['cwd'] = str(self.root.parent)
        with self.assertRaises(ValueError): ExecutionReports(self.wf).identity(run, alias)
        alias = copy.deepcopy(execution)
        alias['intent']['command'].append('--changed')
        with self.assertRaises(ValueError): ExecutionReports(self.wf).identity(run, alias)
