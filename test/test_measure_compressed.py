"""Safety regressions for the disposable Btrfs measurement helper."""
import contextlib
import importlib.util
import io
from pathlib import Path
import shutil
import json
import subprocess
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
        real_copy = shutil.copyfile

        def record_copy(source, destination):
            destinations.append(Path(destination).parent)
            return real_copy(source, destination)

        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", side_effect=[100, 125] * 3), \
                mock.patch.object(measure.shutil, "copyfile", side_effect=record_copy), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(measure.measure_file(str(self.source), "sample"), (4, 25))

        self.assertEqual(len(destinations), 3)
        self.assertEqual(len(set(destinations)), 3)
        self.assertTrue(all(not path.exists() for path in destinations))
        self.assertEqual((self.legacy / "user-data.txt").read_text(), "do not delete")

    def test_copy_failure_removes_only_its_own_temporary_directory(self):
        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", return_value=100), \
                mock.patch.object(measure.shutil, "copyfile", side_effect=OSError("copy failed")):
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


    def test_zstd_property_is_set_only_on_owned_temporary_directories(self):
        probes = []
        copies = []
        real_copy = shutil.copyfile

        def check_property(command, **kwargs):
            self.assertEqual(command[:3], ["btrfs", "property", "set"])
            self.assertEqual(command[4:], ["compression", "zstd:6"])
            self.assertTrue(Path(command[3]).is_dir())
            self.assertEqual(Path(command[3]).parent, self.mount)
            probes.append(Path(command[3]))
            return subprocess.CompletedProcess(command, 0)

        def check_copy(source, destination):
            self.assertEqual(Path(destination).parent, probes[-1])
            copies.append(destination)
            return real_copy(source, destination)

        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", side_effect=[100, 120] * 2), \
                mock.patch.object(measure.subprocess, "run", side_effect=check_property), \
                mock.patch.object(measure.shutil, "copyfile", side_effect=check_copy), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(measure.measure_file(str(self.source), "test",
                                                   level=6, repetitions=2), (4, 20))
        self.assertEqual(len(probes), 2)
        self.assertEqual(len(copies), 2)
        self.assertEqual(len(set(probes)), 2)
        self.assertTrue(all(not probe.exists() for probe in probes))
        self.assertEqual((self.legacy / "user-data.txt").read_text(), "do not delete")

    def test_failed_property_never_copies_and_cleans_its_probe(self):
        failure = subprocess.CalledProcessError(1, ["btrfs", "property", "set"])
        with mock.patch.object(measure.subprocess, "run", side_effect=failure), \
                mock.patch.object(measure.shutil, "copyfile") as copy:
            with self.assertRaises(subprocess.CalledProcessError):
                measure.measure_file(str(self.source), "test", level=9)
        copy.assert_not_called()
        self.assertEqual([entry.name for entry in self.mount.iterdir()], ["_bgc_probe"])

    def test_json_cli_reports_all_requested_levels_without_mutating_source(self):
        output = io.StringIO()
        with mock.patch.object(measure, "clear_cache_sync"), \
                mock.patch.object(measure, "df_used", side_effect=[100, 120, 100, 110]), \
                mock.patch.object(measure.subprocess, "run", return_value=subprocess.CompletedProcess(
                    ["btrfs"], 0)), contextlib.redirect_stdout(output):
            self.assertEqual(measure.main(["--scratch-dir", str(self.mount),
                                           "--zstd-level", "3", "--zstd-level", "9",
                                           "--repetitions", "1", "--json",
                                           str(self.source)]), 0)
        doc = json.loads(output.getvalue())
        self.assertEqual(doc["schema_version"], 1)
        self.assertEqual([row["compression"] for row in doc["results"]], ["zstd:3", "zstd:9"])
        self.assertEqual([row["observed_fs_used_delta_bytes"] for row in doc["results"]], [20, 10])
        self.assertEqual(self.source.read_bytes(), b"data")
        self.assertEqual([entry.name for entry in self.mount.iterdir()], ["_bgc_probe"])

    def test_rejects_symlink_scratch_and_invalid_repetitions(self):
        link = self.root / "scratch-link"
        link.symlink_to(self.mount, target_is_directory=True)
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as caught:
                measure.main(["--scratch-dir", str(link), str(self.source)])
            self.assertEqual(caught.exception.code, 1)
            with self.assertRaises(SystemExit) as caught:
                measure.main(["--scratch-dir", str(self.mount),
                              "--repetitions", "0", str(self.source)])
            self.assertEqual(caught.exception.code, 2)
        self.assertEqual((self.legacy / "user-data.txt").read_text(), "do not delete")


if __name__ == "__main__":
    unittest.main()
