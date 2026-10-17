"""Guard the POC receipt's failure handling and executable architecture labels."""

import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('currency_poc', Path(__file__).with_name('poc.py'))
poc = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(poc)


class PocReceiptTests(unittest.TestCase):
    def test_windows_arm64_host_name_matches_pe_arm64(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'arm64.exe'
            header = bytearray(128)
            header[:2] = b'MZ'
            struct.pack_into('<I', header, 60, 64)
            header[64:68] = b'PE\0\0'
            struct.pack_into('<H', header, 68, 0xAA64)
            path.write_bytes(header)
            with patch.object(poc.platform, 'machine', return_value='ARM64'):
                self.assertEqual(poc.binary_identity(path)['architecture_relation'], 'same')

    def test_checker_observes_exact_artifact_bytes_and_failed_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'artifacts').mkdir()
            (root / 'verify.py').write_text(poc.CHECKER, encoding='utf-8')
            for content, expected in [(b'A delivered\n', 0), (b'A delivered\r\n', 1)]:
                (root / 'artifacts/A.txt').write_bytes(content)
                result = subprocess.run([sys.executable, 'verify.py', '--work', 'A', '--sha',
                                         'a' * 64, '--output', 'report.json'], cwd=root,
                                        capture_output=True, timeout=10)
                self.assertEqual(result.returncode, expected)
                report = json.loads((root / 'report.json').read_text(encoding='utf-8'))
                self.assertEqual(report['checks'][0]['passed'], expected == 0)

    def test_executable_headers_identify_foreign_architectures(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'binary'
            cases = []
            for machine, arch in [(0x1000007, 'x86_64'), (0x100000C, 'arm64')]:
                header = bytearray(64)
                header[:4] = b'\xcf\xfa\xed\xfe'
                struct.pack_into('<I', header, 4, machine)
                cases.append((header, 'Mach-O', arch))
            for machine, arch in [(62, 'x86_64'), (183, 'arm64')]:
                header = bytearray(64)
                header[:6] = b'\x7fELF\x02\x01'
                struct.pack_into('<H', header, 18, machine)
                cases.append((header, 'ELF', arch))
            header = bytearray(128)
            header[:2] = b'MZ'
            struct.pack_into('<I', header, 60, 64)
            header[64:68] = b'PE\0\0'
            struct.pack_into('<H', header, 68, 0x8664)
            cases.append((header, 'PE', 'x86_64'))
            for header, kind, arch in cases:
                with self.subTest(kind=kind, arch=arch):
                    path.write_bytes(header)
                    result = poc.binary_identity(path)
                    self.assertEqual((result['format'], result['architecture']), (kind, arch))
                    self.assertNotIn('native', result['architecture_relation'])

    def test_failed_real_subprocess_retains_exit_and_log_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            run = poc.Run(Path(sys.executable), output)
            with self.assertRaises(AssertionError):
                run.command([sys.executable, '-c', 'print("observed failure"); raise SystemExit(3)'], output)
            self.assertEqual(run.steps[0]['exit_code'], 3)
            self.assertEqual(run.steps[0]['expected_exit'], 0)
            stdout = output / run.steps[0]['stdout']
            self.assertEqual(run.steps[0]['stdout_sha256'], hashlib.sha256(stdout.read_bytes()).hexdigest())
            self.assertIn('observed failure', stdout.read_text())

    def test_failed_scenario_is_recorded_without_stopping_independent_scenarios(self):
        with tempfile.TemporaryDirectory() as temporary:
            run = poc.Run(Path(sys.executable), Path(temporary))
            run.scenario('failure', lambda: poc.require(False, 'failed acceptance'))
            run.scenario('independent', lambda: {'observed': True})
            self.assertFalse(run.scenarios[0]['passed'])
            self.assertTrue(run.scenarios[1]['passed'])
            self.assertIn('failed acceptance', (Path(temporary) / 'failure.failure.txt').read_text())

    def test_public_repository_output_is_rejected_before_creation(self):
        output = poc.REPO / 'evidence-currency-public-output-must-not-exist'
        self.assertFalse(output.exists())
        with patch.object(sys, 'argv', ['poc.py', '--awr', sys.executable, '--output', str(output)]):
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as raised:
                    poc.main()
        self.assertEqual(raised.exception.code, 2)
        self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
