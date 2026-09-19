"""Synthetic regression checks; never read the checkout's private environment."""
import os
import sys
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class LocalBuildTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        for source in Path(__file__).parent.iterdir():
            if source.suffix in ('.sh', '.py', '.ps1'):
                shutil.copy(source, self.root / 'scripts')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        cargo = self.bin / 'cargo'
        cargo.write_text('''#!/bin/bash
set -eu
test "$*" = "${TEST_CARGO_ACTION:-build} --release -p teleark-gui --bin teleark --locked"
test "$TELEARK_DISTRIBUTION_TELEGRAM_API_ID" = 12345
test "$TELEARK_DISTRIBUTION_TELEGRAM_API_HASH" = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
printf invoked > cargo-called
exit "${TEST_CARGO_EXIT:-0}"
''')
        cargo.chmod(0o755)
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'])
        for key in ('TELEARK_DISTRIBUTION_TELEGRAM_API_ID', 'TELEARK_DISTRIBUTION_TELEGRAM_API_HASH'):
            self.env.pop(key, None)

    def config(self, api_id='12345', api_hash='a' * 32):
        (self.root / '.env.local').write_text(
            f'TELEARK_DISTRIBUTION_TELEGRAM_API_ID={api_id}\n'
            f'TELEARK_DISTRIBUTION_TELEGRAM_API_HASH={api_hash}\n')

    def run_build(self):
        result = subprocess.run(['bash', str(self.root / 'scripts/build-local.sh')],
                                cwd=self.bin, env=self.env, capture_output=True, text=True)
        self.assertNotIn('a' * 32, result.stdout + result.stderr)
        return result

    def test_loads_local_from_repository_root(self):
        self.config()
        self.assertEqual(self.run_build().returncode, 0)
        self.assertTrue((self.root / 'cargo-called').exists())

    def test_missing_local_does_not_fall_back_to_example(self):
        self.config()
        (self.root / '.env.local').rename(self.root / '.env.example')
        self.assertNotEqual(self.run_build().returncode, 0)
        self.assertFalse((self.root / 'cargo-called').exists())

    def test_invalid_or_public_values_prevent_build(self):
        for api_id, api_hash in [('17349', 'a' * 32), ('0', 'a' * 32),
                                 ('2147483648', 'a' * 32), ('12345', 'invalid')]:
            with self.subTest(api_id=api_id):
                self.config(api_id, api_hash)
                self.assertNotEqual(self.run_build().returncode, 0)
                self.assertFalse((self.root / 'cargo-called').exists())

    def test_inherited_values_cannot_fill_missing_local_values(self):
        (self.root / '.env.local').write_text('TELEARK_DISTRIBUTION_TELEGRAM_API_ID=12345\n')
        self.env['TELEARK_DISTRIBUTION_TELEGRAM_API_HASH'] = 'a' * 32
        self.assertNotEqual(self.run_build().returncode, 0)
        self.assertFalse((self.root / 'cargo-called').exists())

    def test_build_failure_is_propagated(self):
        self.config()
        self.env['TEST_CARGO_EXIT'] = '7'
        self.assertEqual(self.run_build().returncode, 7)


    def test_run_uses_cargo_run_and_propagates_failure(self):
        self.config()
        self.env['TEST_CARGO_ACTION'] = 'run'
        for exit_code in ('0', '7'):
            self.env['TEST_CARGO_EXIT'] = exit_code
            result = subprocess.run([str(self.root / 'scripts/run.sh')],
                                    cwd=self.bin, env=self.env, capture_output=True)
            self.assertEqual(result.returncode, int(exit_code))
        self.assertTrue((self.root / 'cargo-called').exists())

    def test_every_script_dry_run_has_no_side_effects(self):
        # Sourcing this synthetic config would leave a visible marker and fail.
        (self.root / '.env.local').write_text('touch config-sourced; exit 91\n')
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*'))
        for script in sorted((self.root / 'scripts').iterdir()):
            with self.subTest(script=script.name):
                if script.suffix == '.py':
                    prefix = [sys.executable]
                elif script.suffix == '.ps1':
                    prefix = ['powershell', '-ExecutionPolicy', 'Bypass', '-File']
                else:
                    prefix = ['bash']
                result = subprocess.run(prefix + [str(script), '--dry-run'],
                                        cwd=self.root, env=self.env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('Would', result.stdout)
                self.assertEqual(before, sorted(str(p.relative_to(self.root))
                                               for p in self.root.rglob('*')))

    def test_packaging_dry_run_preserves_custom_paths(self):
        result = subprocess.run(['bash', str(self.root / 'scripts/package-macos.sh'),
                                 'missing binary', 'output app', '--dry-run'],
                                cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('missing binary -> output app', result.stdout)
        self.assertFalse((self.root / 'output app').exists())

    def test_unknown_options_fail_before_loading_config(self):
        for script in ('build-local.sh', 'run.sh', 'package-macos.sh'):
            result = subprocess.run(['bash', str(self.root / 'scripts' / script),
                                     '--unknown'], cwd=self.root, capture_output=True)
            self.assertEqual(result.returncode, 2)
        if shutil.which('powershell'):
            for script in ('build-local.ps1', 'run.ps1'):
                result = subprocess.run(['powershell', '-ExecutionPolicy', 'Bypass',
                                         '-File', str(self.root / 'scripts' / script),
                                         '--unknown'], cwd=self.root, capture_output=True)
                self.assertEqual(result.returncode, 2)
        self.assertFalse((self.root / 'cargo-called').exists())


if __name__ == '__main__':
    if '--dry-run' in sys.argv:
        if sys.argv[1:] != ['--dry-run']:
            raise SystemExit('Usage: python3 scripts/test-build-local.py [--dry-run]')
        print('Would run synthetic script regression tests in temporary directories; no real credentials, build or app launch.')
    else:
        unittest.main()
