"""Test local build entrypoints with temporary configuration and fake build tools."""

import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent
POWERSHELL = shutil.which('pwsh') or shutil.which('powershell')
API_HASH = 'a' * 32  # Synthetic; never load the checkout's .env.local.


class ScriptTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='teleark-script-tests-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / 'repository'
        self.root.mkdir()
        (self.root / 'scripts').mkdir()
        for name in ('build-local.sh', 'run.sh', 'build-local.ps1', 'run.ps1',
                     'package-macos.sh', 'package-linux.sh', 'package-windows.ps1'):
            shutil.copy2(SCRIPTS / name, self.root / 'scripts' / name)
        (self.root / 'scripts/macos').mkdir()
        shutil.copy2(SCRIPTS / 'macos/create-signing-identity.sh', self.root / 'scripts/macos')
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "1.2.3"\n')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.marker = self.root / 'cargo-called'
        self.config_path = self.root / '.env.local'
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(('TELEARK_', 'TEST_CARGO_'))
                    and key not in ('BASH_ENV', 'ENV', 'CARGO_BUILD_TARGET')}
        self.env['PATH'] = str(self.bin) + os.pathsep + os.environ['PATH']
        self.write_tool('cargo', '''#!/bin/bash
set -eu
test "$*" = "${TEST_CARGO_ACTION:-build} --release -p teleark-gui --bin teleark --locked"
test "$TELEARK_DISTRIBUTION_TELEGRAM_API_ID" = 12345
test "$TELEARK_DISTRIBUTION_TELEGRAM_API_HASH" = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
printf invoked > cargo-called
exit "${TEST_CARGO_EXIT:-0}"
''')
        # PowerShell on Windows cannot execute the Bash fixture above.
        if os.name == 'nt':
            self.write_tool('cargo.ps1', '''
$action = if ($env:TEST_CARGO_ACTION) { $env:TEST_CARGO_ACTION } else { 'build' }
if (($args -join ' ') -cne "$action --release -p teleark-gui --bin teleark --locked") { exit 97 }
if ($env:TELEARK_DISTRIBUTION_TELEGRAM_API_ID -cne '12345') { exit 98 }
if ($env:TELEARK_DISTRIBUTION_TELEGRAM_API_HASH -cne ('a' * 32)) { exit 99 }
Set-Content -LiteralPath cargo-called -Value invoked
if ($env:TEST_CARGO_EXIT) { exit ([int]$env:TEST_CARGO_EXIT) }
exit 0
''')

    def write_tool(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def config(self, api_id='12345', api_hash=API_HASH):
        self.config_path.write_text(
            f'TELEARK_DISTRIBUTION_TELEGRAM_API_ID={api_id}\n'
            f'TELEARK_DISTRIBUTION_TELEGRAM_API_HASH={api_hash}\n')

    def run_script(self, script, *arguments, env=None):
        if script.endswith('.ps1'):
            if not POWERSHELL:
                self.skipTest('PowerShell is unavailable')
            prefix = [POWERSHELL, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File']
        else:
            if not shutil.which('bash'):
                self.skipTest('Bash is unavailable')
            prefix = ['bash']
        result = subprocess.run(prefix + [str(self.root / 'scripts' / script), *arguments],
                                cwd=self.bin, env=env or self.env,
                                capture_output=True, text=True)
        self.assertNotIn(API_HASH, result.stdout + result.stderr)
        return result

    def build_scripts(self):
        scripts = ['build-local.sh'] if shutil.which('bash') else []
        if POWERSHELL:
            scripts.append('build-local.ps1')
        return scripts

    def test_builds_from_repository_and_restricts_config(self):
        for script in self.build_scripts():
            with self.subTest(script=script):
                self.config()
                self.config_path.chmod(0o644)
                result = self.run_script(script)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(self.marker.exists())
                self.marker.unlink()
                if os.name != 'nt':
                    self.assertEqual(stat.S_IMODE(self.config_path.stat().st_mode), 0o600)

    def test_missing_local_does_not_load_example(self):
        self.config()
        self.config_path.rename(self.root / '.env.example')
        for script in self.build_scripts():
            with self.subTest(script=script):
                self.assertNotEqual(self.run_script(script).returncode, 0)
                self.assertFalse(self.marker.exists())

    def test_invalid_or_public_values_prevent_build(self):
        for script in self.build_scripts():
            for api_id, api_hash in [('17349', API_HASH), ('0', API_HASH),
                                     ('2147483648', API_HASH), ('12345', 'invalid')]:
                with self.subTest(script=script, api_id=api_id):
                    self.config(api_id, api_hash)
                    self.assertNotEqual(self.run_script(script).returncode, 0)
                    self.assertFalse(self.marker.exists())

    def test_inherited_values_cannot_fill_missing_local_values(self):
        self.config_path.write_text('TELEARK_DISTRIBUTION_TELEGRAM_API_ID=12345\n')
        self.env['TELEARK_DISTRIBUTION_TELEGRAM_API_HASH'] = API_HASH
        for script in self.build_scripts():
            with self.subTest(script=script):
                self.assertNotEqual(self.run_script(script).returncode, 0)
                self.assertFalse(self.marker.exists())

    def test_build_and_run_propagate_failure(self):
        self.config()
        for build in self.build_scripts():
            for script, action in [(build, 'build'), (build.replace('build-local', 'run'), 'run')]:
                for status in ('0', '7'):
                    with self.subTest(script=script, status=status):
                        self.env.update(TEST_CARGO_ACTION=action, TEST_CARGO_EXIT=status)
                        result = self.run_script(script)
                        self.assertEqual(result.returncode, int(status), result.stderr)
                        self.assertTrue(self.marker.exists())
                        self.marker.unlink()

    def test_supported_dry_runs_do_not_read_config_or_create_outputs(self):
        self.config_path.write_text('touch config-sourced; exit 91\n')
        original_mode = stat.S_IMODE(self.config_path.stat().st_mode)
        scripts = self.build_scripts() + [name.replace('build-local', 'run')
                                         for name in self.build_scripts()]
        scripts += ['package-macos.sh', 'package-linux.sh']
        if POWERSHELL:
            scripts.append('package-windows.ps1')
        # Linux dry-run plans the supported x86_64 target, even on a macOS host.
        self.write_tool('uname', '#!/bin/bash\nprintf "x86_64\\n"\n')
        before = sorted(str(path.relative_to(self.root)) for path in self.root.rglob('*'))
        for script in scripts:
            with self.subTest(script=script):
                result = self.run_script(script, '--dry-run')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('Would', result.stdout)
                self.assertEqual(before, sorted(str(path.relative_to(self.root))
                                               for path in self.root.rglob('*')))
                self.assertEqual(stat.S_IMODE(self.config_path.stat().st_mode),
                                 original_mode)

    def test_packaging_dry_run_preserves_paths_and_version(self):
        result = self.run_script('package-macos.sh', 'missing binary', 'output app', '--dry-run')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('missing binary -> output app', result.stdout)
        self.assertIn('TeleArk-1.2.3-macos-universal.pkg', result.stdout)
        self.assertFalse((self.bin / 'output app').exists())

    @unittest.skipUnless(POWERSHELL, 'PowerShell is unavailable')
    def test_windows_packaging_dry_run_accepts_trailing_directory_separator(self):
        destination = str(self.root / 'output dir') + os.sep
        result = self.run_script('package-windows.ps1', 'missing.exe', destination, '--dry-run')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Would', result.stdout)
        self.assertFalse((self.root / 'output dir').exists())

    @unittest.skipUnless(Path('/usr/share/doc/bash/copyright').is_file(),
                         'Linux distribution notices are unavailable')
    def test_linux_packages_the_input_binary_and_requested_version(self):
        # Fake native tools isolate argument routing, payload selection and cleanup.
        # Actual ELF, cargo-deb, linuxdeploy and installation checks run on Linux CI.
        for name in ('README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY_NOTICES.md'):
            (self.root / name).write_text('Fixture notice\n')
        source = self.bin / 'custom input'
        source.write_text('selected executable\n')
        source.chmod(0o755)
        self.env['TEST_PACKAGE_BINARY'] = str(source)
        self.env['TEST_PACKAGE_ROOT'] = str(self.root)
        self.write_tool('uname', '#!/bin/bash\nprintf "x86_64\\n"\n')
        self.write_tool('cargo-deb', '#!/bin/bash\nexit 0\n')
        self.write_tool('ldd', '#!/bin/bash\nprintf "/fixture/lib.so\\n"\n')
        self.write_tool('readlink', '#!/bin/bash\nprintf "%s\\n" "$2"\n')
        self.write_tool('dpkg-query', '#!/bin/bash\nprintf "bash: /fixture/lib.so\\n"\n')
        self.write_tool('sha256sum', '''#!/bin/bash
for file in "$@"; do
  test -s "$file" || exit 1
  printf '%064d  %s\n' 0 "$file"
done
''')
        self.write_tool('linuxdeploy', '''#!/bin/bash
set -euo pipefail
if [[ -n "${LDAI_OUTPUT:-}" ]]; then
  printf 'fixture AppImage\n' > "$LDAI_OUTPUT"
  exit 0
fi
[[ "$1" == --appdir && "$3" == --executable ]]
mkdir -p "$2/usr/bin"
cp "$4" "$2/usr/bin/teleark"
cp "$4" "$2/AppRun"
''')
        self.write_tool('cargo', '''#!/bin/bash
set -euo pipefail
[[ "$*" == "deb -p teleark-gui --no-build --locked --manifest-path $TEST_PACKAGE_ROOT/crates/teleark-gui/Cargo.toml --target x86_64-unknown-linux-gnu --deb-version 2.3.4 --output "* ]]
staged="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/teleark"
cmp "$staged" "$TEST_PACKAGE_BINARY"
printf 'simulated strip change\n' >> "$staged"
printf 'fixture Debian package\n' > "${!#}"
''')
        result = self.run_script('package-linux.sh', str(source), 'output dir', '2.3.4', 'preview')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(source.read_text(), 'selected executable\n')
        output = self.bin / 'output dir'
        self.assertEqual(sorted(path.name for path in output.iterdir()), [
            'SHA256SUMS', 'teleark-preview-linux-x86_64.AppImage',
            'teleark-preview-linux-x86_64.tar.gz', 'teleark_preview_amd64.deb'])
        self.assertEqual(len((output / 'SHA256SUMS').read_text().splitlines()), 3)

    def test_unknown_options_fail_before_loading_config(self):
        self.config_path.write_text('touch config-sourced; exit 91\n')
        scripts = self.build_scripts() + [name.replace('build-local', 'run')
                                         for name in self.build_scripts()]
        for script in scripts + ['package-macos.sh', 'package-linux.sh']:
            with self.subTest(script=script):
                self.assertEqual(self.run_script(script, '--unknown').returncode, 2)
        self.assertFalse((self.root / 'config-sourced').exists())
        self.assertFalse(self.marker.exists())

    @unittest.skipIf(os.name == 'nt', 'Unix signing identity permissions')
    def test_identity_creation_preserves_unrelated_file_permissions(self):
        identity = self.root.parent / 'private identity'
        identity.mkdir()
        unrelated = identity / 'notes.txt'
        unrelated.write_text('Unrelated fixture\n')
        unrelated.chmod(0o644)
        self.write_tool('openssl', '''#!/bin/bash
set -eu
while (( $# )); do
  case "$1" in
    -out|-keyout) shift; printf 'synthetic identity fixture\n' > "$1" ;;
  esac
  shift
done
''')
        result = self.run_script('macos/create-signing-identity.sh', str(identity))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(stat.S_IMODE(unrelated.stat().st_mode), 0o644)
        self.assertEqual(stat.S_IMODE(identity.stat().st_mode), 0o700)
        for name in ('private-key.pem', 'certificate.pem', 'identity.p12'):
            self.assertEqual(stat.S_IMODE((identity / name).stat().st_mode), 0o600)

    @unittest.skipIf(os.name == 'nt', 'Unix signing identity paths')
    def test_identity_creation_rejects_checkout_alias_and_existing_symlinks(self):
        inside = self.root / 'private'
        inside.mkdir()
        inside.chmod(0o755)
        alias = self.root.parent / 'alias'
        alias.symlink_to(inside, target_is_directory=True)
        result = self.run_script('macos/create-signing-identity.sh', str(alias))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('outside the checkout', result.stderr)
        self.assertEqual(stat.S_IMODE(inside.stat().st_mode), 0o755)
        self.assertEqual(list(inside.iterdir()), [])

        outside = self.root.parent / 'private'
        outside.mkdir()
        (outside / 'identity.p12').symlink_to(outside / 'missing')
        result = self.run_script('macos/create-signing-identity.sh', str(outside))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('refusing to replace', result.stderr)
        self.assertFalse((outside / 'private-key.pem').exists())

    @unittest.skipUnless(POWERSHELL, 'PowerShell is unavailable')
    def test_powershell_accepts_quoted_export_assignments(self):
        self.config_path.write_text(
            '# Synthetic dotenv configuration\n'
            'export TELEARK_DISTRIBUTION_TELEGRAM_API_ID="12345"\n'
            f"TELEARK_DISTRIBUTION_TELEGRAM_API_HASH='{API_HASH}'\n")
        result = self.run_script('build-local.ps1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.marker.exists())

    @unittest.skipUnless(POWERSHELL, 'PowerShell is unavailable')
    def test_powershell_rejects_malformed_config_without_echoing_values(self):
        for extra_line in ('synthetic-private-value', 'EXTRA="synthetic-private-value'):
            with self.subTest(extra_line=extra_line):
                self.config()
                with self.config_path.open('a') as config:
                    config.write(extra_line + '\n')
                result = self.run_script('build-local.ps1')
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn('synthetic-private-value', result.stdout + result.stderr)
                self.assertFalse(self.marker.exists())

    @unittest.skipUnless(POWERSHELL, 'PowerShell is unavailable')
    def test_powershell_restores_callers_directory_and_environment(self):
        self.config()
        with self.config_path.open('a') as config:
            config.write('TELEARK_TEST_RESTORED=temporary\nTELEARK_TEST_REMOVED=temporary\n')
        self.env['TELEARK_TEST_RESTORED'] = 'original'
        self.env['TELEARK_DISTRIBUTION_TELEGRAM_API_ID'] = 'inherited'
        wrapper = self.bin / 'check-restoration.ps1'
        wrapper.write_text('''
$before = (Get-Location).Path
& (Join-Path $PSScriptRoot '../scripts/build-local.ps1')
$buildStatus = $LASTEXITCODE
if ((Get-Location).Path -ne $before) { exit 91 }
if ($env:TELEARK_TEST_RESTORED -ne 'original') { exit 92 }
if ($env:TELEARK_TEST_REMOVED) { exit 93 }
if ($env:TELEARK_DISTRIBUTION_TELEGRAM_API_ID -ne 'inherited') { exit 94 }
if ($env:TELEARK_DISTRIBUTION_TELEGRAM_API_HASH) { exit 95 }
exit $buildStatus
''')
        for status in ('0', '7'):
            with self.subTest(status=status):
                self.env['TEST_CARGO_EXIT'] = status
                result = subprocess.run([POWERSHELL, '-NoProfile', '-File', str(wrapper)],
                                        cwd=self.bin, env=self.env, capture_output=True, text=True)
                self.assertEqual(result.returncode, int(status), result.stderr)
                self.assertNotIn(API_HASH, result.stdout + result.stderr)


if __name__ == '__main__':
    if sys.argv[1:] == ['--dry-run']:
        print('Would run synthetic script regressions in temporary directories; no real credentials, build or app launch.')
    else:
        unittest.main()
