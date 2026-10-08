#!/usr/bin/env python3
"""Contract tests for the fixed-backend, read-only opportunity report."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / 'tools' / 'asset-opportunity-report.py'
SPEC = importlib.util.spec_from_file_location('asset_opportunity_report', SCRIPT)
REPORT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REPORT)


def xnb_response(size=524_400):
    return (f'XNB_CONTENT_HEADER|w|5|0|{size}\n'
            'XNB_SHARED|0\nXNB_ROOT|1\nXNB_TEXTURE|4|1024|1024|1\n'
            'XNB_MIP|0|1024|1024|112|524288\n'
            'XNB_TEXTURE_STATUS|METADATA_ONLY|METADATA_ONLY\n')


class ReportTests(unittest.TestCase):
    def test_experimental_xnb_is_separate_from_planner_totals(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            backend = root / 'fake-backend'
            backend.write_text(
                '#!/usr/bin/env python3\n'
                'import sys\n'
                'if sys.argv[1] == "--development-audits":\n'
                '    print("Development reader routes: compiled in")\n'
                'elif sys.argv[1] == "xnb-texture-audit":\n'
                f'    print({xnb_response()!r})\n'
                'else:\n'
                '    print("ASSETS|plan|Balanced (1080p)|1|100|70|0|2|1|0|0|1|0|500|400|80|0")\n'
            )
            backend.chmod(0o700)
            game = root / 'game'
            game.mkdir()
            size = 524_400
            (game / 'page.xnb').write_bytes(b'XNBw\x05\x00' + size.to_bytes(4, 'little') + bytes(size - 10))
            status = root / 'status.json'
            status.write_text(json.dumps({'games': [{'name': 'Example', 'path': str(game)}]}))
            output = root / 'out'
            result = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                     '--backend', str(backend), '--xnb-backend', str(backend),
                                     '--xnb-max-edge', '512', '--output-dir', str(output)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads((output / 'asset-opportunities.json').read_text())
            row = report['games'][0]
            self.assertEqual(row['estimated_logical_savings_bytes'], 30)
            self.assertEqual(row['xnb_theoretical_logical_savings_bytes'], 393_216)
            self.assertEqual(row['xnb_audited_files'], 1)
            self.assertEqual(row['xnb_header_files'], 1)
            self.assertEqual(report['schema'], 3)
            markdown = (output / 'asset-opportunities.md').read_text()
            self.assertIn('Experimental XNB v5 texture opportunities', markdown)
            self.assertIn('393,216', markdown)
            # A previous schema must remain intact, not silently acquire new
            # coverage semantics by resuming with a newer parser contract.
            report['schema'] = 2
            report_path = output / 'asset-opportunities.json'
            report_path.write_text(json.dumps(report))
            old_report = report_path.read_bytes()
            old_markdown = (output / 'asset-opportunities.md').read_bytes()
            resumed = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                      '--backend', str(backend), '--xnb-backend', str(backend),
                                      '--xnb-max-edge', '512', '--output-dir', str(output), '--resume'],
                                     capture_output=True, text=True)
            self.assertEqual(resumed.returncode, 2)
            self.assertEqual(report_path.read_bytes(), old_report)
            self.assertEqual((output / 'asset-opportunities.md').read_bytes(), old_markdown)

    def test_xnb_parser_rejects_mismatched_source_spans_and_record_schemas(self):
        good = xnb_response()
        self.assertEqual(REPORT.parse_xnb_audit(good, 524_400, 512)['candidate_width'], 512)
        for bad in [good + 'XNB_MIP|0|1024|1024|112|524288\n',
                    good.replace('w|5|0|524400', 'w|5|0|524401'),
                    good.replace('112|524288', '113|524288'),
                    good.replace('112|524288', '112|524287'),
                    good.replace('XNB_ROOT|1', 'XNB_ROOT|0'),
                    good.replace('XNB_SHARED|0', 'XNB_SHARED|1'),
                    good.replace('1024|1024|1', '１０２４|1024|1'),
                    good.replace('METADATA_ONLY|METADATA_ONLY', 'METADATA_ONLY|UNKNOWN')]:
            with self.subTest(response=bad), self.assertRaises(ValueError):
                REPORT.parse_xnb_audit(bad, 524_400, 512)
        opaque = 'XNB_TEXTURE_STATUS|OPAQUE|CUSTOM_OR_UNSUPPORTED_ROOT_READER\n'
        self.assertIsNone(REPORT.parse_xnb_audit(opaque, 524_400, 512))

    def test_xnb_errors_and_timeouts_preserve_partial_research_results(self):
        paths = [(Path('one.xnb'), 524_400), (Path('two.xnb'), 524_400)]
        ok = subprocess.CompletedProcess([], 0, xnb_response(), '')
        malformed = subprocess.CompletedProcess([], 0, 'XNB_TEXTURE_STATUS|bad\n', '')
        with mock.patch.object(REPORT, 'plain_xnbs', return_value=iter(paths)), \
                mock.patch.object(REPORT.time, 'monotonic', return_value=0), \
                mock.patch.object(REPORT.subprocess, 'run', side_effect=[ok, malformed]):
            result = REPORT.xnb_opportunities(Path('backend'), Path('game'), 512, 10)
        self.assertEqual(result['xnb_audit_status'], 'partial error')
        self.assertEqual(result['xnb_audited_files'], 1)
        self.assertEqual(result['xnb_error_count'], 1)
        self.assertEqual(result['xnb_theoretical_logical_savings_bytes'], 393_216)
        with mock.patch.object(REPORT, 'plain_xnbs', return_value=iter(paths)), \
                mock.patch.object(REPORT.time, 'monotonic', return_value=0), \
                mock.patch.object(REPORT.subprocess, 'run', side_effect=[ok, subprocess.TimeoutExpired('audit', 10)]):
            timed_out = REPORT.xnb_opportunities(Path('backend'), Path('game'), 512, 10)
        self.assertEqual(timed_out['xnb_audit_status'], 'timeout')
        self.assertEqual(timed_out['xnb_theoretical_logical_savings_bytes'], 393_216)

    def test_optional_xnb_failure_does_not_erase_production_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            backend = root / 'fake-backend'
            backend.write_text(
                '#!/usr/bin/env python3\nimport sys\n'
                'if sys.argv[1] == "--development-audits":\n'
                '    print("Development reader routes: compiled in")\n'
                'elif sys.argv[1] == "xnb-texture-audit":\n'
                '    print("XNB_TEXTURE_STATUS|bad")\n'
                'else:\n'
                '    print("ASSETS|plan|Balanced (1080p)|1|100|70|0|2|1|0|0|1|0|500|400|80|0")\n')
            backend.chmod(0o700)
            game = root / 'game'
            game.mkdir()
            size = 524_400
            (game / 'page.xnb').write_bytes(b'XNBw\x05\x00' + size.to_bytes(4, 'little') + bytes(size - 10))
            status = root / 'status.json'
            status.write_text(json.dumps({'games': [{'name': 'Example', 'path': str(game)}]}))
            output = root / 'out'
            result = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                     '--backend', str(backend), '--xnb-backend', str(backend),
                                     '--output-dir', str(output)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            row = json.loads((output / 'asset-opportunities.json').read_text())['games'][0]
            self.assertEqual(row['status'], 'ok')
            self.assertEqual(row['estimated_logical_savings_bytes'], 30)
            self.assertEqual(row['xnb_audit_status'], 'partial error')
            self.assertEqual(row['xnb_error_count'], 1)
            markdown = (output / 'asset-opportunities.md').read_text()
            self.assertIn('| Example | 30 |', markdown)

    def test_xnb_discovery_accepts_profile_flag_and_skips_final_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'page.xnb'
            source.write_bytes(b'XNBw\x05\x01' + (10).to_bytes(4, 'little'))
            (root / 'linked.xnb').symlink_to(source)
            self.assertEqual(list(REPORT.plain_xnbs(root)), [(source, 10)])
            with self.assertRaises(subprocess.TimeoutExpired):
                list(REPORT.plain_xnbs(root, deadline=0))

    def test_ranked_report_and_partial_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            backend = root / 'fake-backend'
            backend.write_text(
                '#!/usr/bin/env python3\n'
                'import sys\n'
                'if sys.argv[1] == "godot-texture-audit":\n'
                '    print("GODOT_TEXTURE|100|30|3|1|2|70|70.0000|CANDIDATE")\n'
                '    sys.exit(0)\n'
                'if sys.argv[-1].endswith("bad"):\n'
                '    print("unsupported fixture", file=sys.stderr)\n'
                '    sys.exit(3)\n'
                'saved = 30 if sys.argv[-1].endswith("large") else 5\n'
                'print(f"ASSETS|plan|Balanced (1080p)|1|100|{100-saved}|0|2|1|0|0|1|0|500|400|80|0")\n'
            )
            backend.chmod(0o700)
            status = root / 'status.json'
            (root / 'small').mkdir()
            large = root / 'large'
            large.mkdir()
            (large / 'content.pck').write_bytes(b'GDPC' + (4).to_bytes(4, 'little') + bytes(92))
            status.write_text(json.dumps({'games': [
                {'name': 'Small', 'path': str(root / 'small')},
                {'name': 'Bad', 'path': str(root / 'bad')},
                {'name': 'Large', 'path': str(root / 'large')},
            ]}))
            output = root / 'out'
            result = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                     '--backend', str(backend), '--output-dir', str(output)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            data = json.loads((output / 'asset-opportunities.json').read_text())
            self.assertEqual([row['status'] for row in data['games']], ['ok', 'error', 'ok'])
            self.assertEqual(data['games'][2]['planner_logical_savings_bytes'], 30)
            self.assertEqual(data['games'][2]['godot4_logical_savings_bytes'], 70)
            self.assertEqual(data['games'][2]['estimated_logical_savings_bytes'], 100)
            self.assertIsNone(data['games'][2]['physical_savings_bytes'])
            markdown = (output / 'asset-opportunities.md').read_text()
            self.assertLess(markdown.index('| Large |'), markdown.index('| Small |'))
            self.assertIn('Physical Btrfs savings and game compatibility are unknown', markdown)
            snapshots = list(output.glob('bgc-native-*'))
            self.assertEqual(len(snapshots), 1)
            self.assertEqual(snapshots[0].read_bytes(), backend.read_bytes())
            again = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                    '--backend', str(backend), '--output-dir', str(output)],
                                   capture_output=True, text=True)
            self.assertEqual(again.returncode, 2)
            self.assertIn('report already exists', again.stderr)
            resumed = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                      '--backend', str(backend), '--output-dir', str(output), '--resume'],
                                     capture_output=True, text=True)
            self.assertEqual(resumed.returncode, 1)  # Previous failed row remains visible.
            self.assertEqual(len(json.loads((output / 'asset-opportunities.json').read_text())['games']), 3)

    def test_rejects_malformed_response(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            backend = root / 'fake-backend'
            backend.write_text('#!/usr/bin/env python3\nprint("ASSETS|plan|Balanced (1080p)|bad")\n')
            backend.chmod(0o700)
            status = root / 'status.json'
            game = root / 'game'
            game.mkdir()
            status.write_text(json.dumps({'games': [{'name': 'Broken', 'path': str(game)}]}))
            output = root / 'out'
            result = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                     '--backend', str(backend), '--output-dir', str(output)],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            row = json.loads((output / 'asset-opportunities.json').read_text())['games'][0]
            self.assertEqual(row['status'], 'error')
            self.assertIn('schema', row['error'])

    def test_refuses_output_inside_game(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            game = root / 'game'
            game.mkdir()
            status = root / 'status.json'
            status.write_text(json.dumps({'games': [{'name': 'Game', 'path': str(game)}]}))
            output = game / 'report'
            result = subprocess.run([sys.executable, str(SCRIPT), '--status-json', str(status),
                                     '--backend', sys.executable, '--output-dir', str(output)],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('inside scanned game', result.stderr)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
