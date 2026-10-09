"""Fixture-only checks for the durable recovery runner; no real ioctls."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'resume_compaction', Path(__file__).resolve().parents[1] / 'tools' / 'resume-library-compaction.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='bgc-recovery-test-')
        self.root = Path(self.temporary.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.game = self.root / 'game'
        self.game.mkdir()
        (self.game / 'asset.bin').write_bytes(b'original asset bytes')
        self.state_dir = self.root / 'state'
        self.calls = self.root / 'calls'
        discovery = {'games': [{'name': 'Example', 'path': str(self.game)}]}
        cli = self.repo / 'btrfs-game-compressor'
        cli.write_text("#!/bin/sh\ncat <<'JSON'\n" + json.dumps(discovery) + '\nJSON\n')
        cli.chmod(0o755)
        backend = self.repo / 'bgc-native'
        backend.write_text(f'#!/bin/sh\nprintf "%s\\n" "$1" >> "{self.calls}"\nexit 0\n')
        backend.chmod(0o755)

    def tearDown(self):
        self.temporary.cleanup()

    def run_job(self, assets=False):
        arguments = ['runner', '--repo', str(self.repo), '--state-dir', str(self.state_dir)]
        if assets:
            arguments.append('--apply-assets-no-backup')
        with patch('sys.argv', arguments), \
                patch.object(runner, 'running', return_value=False):
            runner.main()
        return json.loads((self.state_dir / 'results.json').read_text())

    def test_atomic_json_ignores_predictable_foreign_temporary_file(self):
        self.state_dir.mkdir()
        journal = self.state_dir / 'results.json'
        foreign = journal.with_suffix('.tmp')
        foreign.write_text('foreign data')

        runner.atomic_json(journal, {'safe': True})

        self.assertEqual(json.loads(journal.read_text()), {'safe': True})
        self.assertEqual(foreign.read_text(), 'foreign data')

    def test_atomic_json_preserves_previous_checkpoint_and_cleans_failed_temp(self):
        self.state_dir.mkdir()
        journal = self.state_dir / 'results.json'
        journal.write_text('{"previous": true}')

        with patch.object(runner.os, 'replace', side_effect=OSError('injected failure')):
            with self.assertRaisesRegex(OSError, 'injected failure'):
                runner.atomic_json(journal, {'replacement': True})

        self.assertEqual(json.loads(journal.read_text()), {'previous': True})
        self.assertEqual(list(self.state_dir.glob('.results.json.*.tmp')), [])

    def test_asset_pipeline_checkpoints_and_never_repeats_completed_assets(self):
        self.run_job(assets=True)
        self.run_job(assets=True)
        self.assertEqual(self.calls.read_text().splitlines(), ['assets', 'compress', 'dedupe'])

    def test_uncertain_asset_stage_is_not_reapplied(self):
        state = self.run_job(assets=True)
        state['games'][0].pop('assets')
        state['games'][0]['status'] = 'assets_in_progress'
        runner.atomic_json(self.state_dir / 'results.json', state)
        result = self.run_job(assets=True)
        self.assertEqual(result['games'][0]['status'], 'asset_state_uncertain_manual_recovery_required')
        self.assertEqual(self.calls.read_text().splitlines(), ['assets', 'compress', 'dedupe'])

    def test_completed_job_resumes_without_repeating_operations_or_assets(self):
        state = self.run_job()
        self.assertEqual(state['games'][0]['status'], 'lossless_operations_completed_assets_unverified')
        self.run_job()
        self.assertEqual(self.calls.read_text().splitlines(), ['compress', 'dedupe'])
        self.assertEqual((self.game / 'asset.bin').read_bytes(), b'original asset bytes')
        self.assertFalse((self.game / '.bgc-assets-backup').exists())
        self.assertIn('not net space freed', (self.state_dir / 'stats.md').read_text())

    def test_interrupted_dedupe_does_not_repeat_successful_compression(self):
        state = self.run_job()
        state['games'][0].pop('dedupe')
        state['games'][0]['status'] = 'dedupe_in_progress'
        runner.atomic_json(self.state_dir / 'results.json', state)
        self.run_job()
        self.assertEqual(self.calls.read_text().splitlines(), ['compress', 'dedupe', 'dedupe'])

    def test_existing_recovery_data_is_preserved(self):
        recovery = self.game / '.bgc-assets-backup'
        recovery.mkdir()
        (recovery / 'original').write_bytes(b'keep this')
        state = self.run_job()
        self.assertEqual(state['games'][0]['status'], 'skipped_existing_recovery_data')
        self.assertFalse(self.calls.exists())
        self.assertEqual((recovery / 'original').read_bytes(), b'keep this')


if __name__ == '__main__':
    unittest.main()
