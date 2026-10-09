"""Safety regressions for the disposable Btrfs measurement helper."""
import contextlib
import importlib.util
import io
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock


spec = importlib.util.spec_from_file_location(
    "bgc_measure_compressed", Path(__file__).with_name("measure-compressed.py"))
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


class MeasurementProbeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bgc-measure-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.mount = self.root / "mount"
        self.mount.mkdir()
        self.source = self.root / "input.bin"
        self.source.write_bytes(b"data")
        self.legacy = self.mount / "_bgc_probe"
        self.legacy.mkdir()
        (self.legacy / "user-data.txt").write_text("do not delete")
        self.mount_patch = mock.patch.object(measure, "MOUNT", str(self.mount))
        self.mount_patch.start()
        self.addCleanup(self.mount_patch.stop)

    def test_existing_probe_survives_and_each_trial_uses_unique_directory(self):
        destinations = []
        real_copy = shutil.copy2

        def record_copy(source, destination):
            destinations.append(Path(destination).parent)
            return real_copy(source, destination)

        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", side_effect=[100, 125] * 3), \
                mock.patch.object(measure.shutil, "copy2", side_effect=record_copy), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(measure.measure_file(str(self.source), "sample"), (4, 25))

        self.assertEqual(len(destinations), 3)
        self.assertEqual(len(set(destinations)), 3)
        self.assertTrue(all(not path.exists() for path in destinations))
        self.assertEqual((self.legacy / "user-data.txt").read_text(), "do not delete")

    def test_copy_failure_removes_only_its_own_temporary_directory(self):
        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", return_value=100), \
                mock.patch.object(measure.shutil, "copy2", side_effect=OSError("copy failed")):
            with self.assertRaisesRegex(OSError, "copy failed"):
                measure.measure_file(str(self.source), "sample")

        self.assertEqual(sorted(path.name for path in self.mount.iterdir()), ["_bgc_probe"])
        self.assertEqual((self.legacy / "user-data.txt").read_text(), "do not delete")

    def test_empty_input_does_not_divide_by_zero(self):
        self.source.write_bytes(b"")
        output = io.StringIO()
        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", return_value=100), \
                contextlib.redirect_stdout(output):
            self.assertEqual(measure.measure_file(str(self.source), "empty"), (0, 0))
        self.assertIn("n/a", output.getvalue())
        self.assertEqual((self.legacy / "user-data.txt").read_text(), "do not delete")


if __name__ == "__main__":
    unittest.main()
