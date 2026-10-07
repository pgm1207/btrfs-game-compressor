#!/usr/bin/env python3
"""Contract tests for the fixed-backend, read-only opportunity report."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / 'tools' / 'asset-opportunity-report.py'


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
                '    print("XNB_TEXTURE|4|1024|1024|1")\n'
                '    print("XNB_TEXTURE_STATUS|METADATA_ONLY|METADATA_ONLY")\n'
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
            markdown = (output / 'asset-opportunities.md').read_text()
            self.assertIn('Experimental XNB v5 texture opportunities', markdown)
            self.assertIn('393,216', markdown)

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
