"""Regression tests for read-only space reporting; no Btrfs or privileges required."""
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest import mock


def load():
    path = Path(__file__).resolve().parents[1] / "tools" / "space-delta.py"
    spec = importlib.util.spec_from_file_location("bgc_space_delta", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


space = load()


def snapshot(root="/games/example", device=123, available=1000, disk=600):
    measured = disk is not None
    return {
        "schema_version": 1, "kind": "snapshot", "root": root,
        "device": device, "captured_utc": "2026-10-08T00:00:00+00:00",
        "filesystem_space": {
            "available_bytes": available,
            "free_bytes": available + 100,
            "total_bytes": 5000,
            "fragment_bytes": 4096,
        },
        "extent_measurement": {
            "status": "measured" if measured else "unavailable",
            "reason": None if measured else "permission denied",
            "disk_bytes": disk,
            "raw_extent_bytes": disk + 100 if measured else None,
            "referenced_bytes": disk + 20 if measured else None,
        },
    }


class SpaceDeltaTests(unittest.TestCase):
    def test_capture_is_read_only_and_missing_backend_is_unknown(self):
        with tempfile.TemporaryDirectory() as root:
            before = sorted(Path(root).iterdir())
            report = space.capture(root)
            self.assertEqual(report["kind"], "snapshot")
            self.assertEqual(report["extent_measurement"]["status"], "unavailable")
            self.assertIsNone(report["extent_measurement"]["disk_bytes"])
            self.assertGreaterEqual(report["filesystem_space"]["available_bytes"], 0)
            self.assertEqual(sorted(Path(root).iterdir()), before)

    def test_symlink_root_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            parent = Path(root)
            (parent / "real").mkdir()
            (parent / "link").symlink_to(parent / "real", target_is_directory=True)
            with self.assertRaises(ValueError):
                space.capture(parent / "link")

    def test_native_measurement_is_strict_and_never_invents_zero(self):
        with mock.patch.object(space.subprocess, "run", return_value=subprocess.CompletedProcess(
            ["bgc-native"], 0, "123|234|345\n", "")):
            values = space._measure(Path("/game"), Path("/bin/backend"))
            self.assertEqual(values["disk_bytes"], 123)
            self.assertEqual(values["status"], "measured")
        with mock.patch.object(space.subprocess, "run", return_value=subprocess.CompletedProcess(
            ["bgc-native"], 0, "123|not-a-number|345\n", "")):
            bad = space._measure(Path("/game"), Path("/bin/backend"))
            self.assertEqual(bad["status"], "unavailable")
            self.assertIsNone(bad["disk_bytes"])
        with mock.patch.object(space.subprocess, "run", side_effect=subprocess.TimeoutExpired("bgc-native", 120)):
            timed_out = space._measure(Path("/game"), Path("/bin/backend"))
            self.assertEqual(timed_out["status"], "unavailable")
            self.assertIsNone(timed_out["raw_extent_bytes"])

    def test_comparison_separates_extent_and_observed_free_space(self):
        report = space.compare(snapshot(available=1000, disk=600),
                               snapshot(available=1250, disk=400))
        self.assertEqual(report["filesystem_available_delta_bytes"], 250)
        self.assertEqual(report["game_extent_disk_reduction_bytes"], 200)
        self.assertEqual(report["game_referenced_extent_reduction_bytes"], 200)
        self.assertNotEqual(report["filesystem_available_delta_bytes"],
                            report["game_extent_disk_reduction_bytes"])

    def test_growth_is_negative_and_missing_is_null(self):
        growth = space.compare(snapshot(available=1000, disk=600),
                               snapshot(available=900, disk=700))
        self.assertEqual(growth["filesystem_available_delta_bytes"], -100)
        self.assertEqual(growth["game_extent_disk_reduction_bytes"], -100)
        unknown = space.compare(snapshot(disk=600), snapshot(available=1200, disk=None))
        self.assertEqual(unknown["extent_data_status"], "unavailable")
        self.assertIsNone(unknown["game_extent_disk_reduction_bytes"])

    def test_mismatched_identity_and_invalid_fixtures_are_rejected(self):
        with self.assertRaises(ValueError):
            space.compare(snapshot(), snapshot(device=124))
        invalid = snapshot(disk=None)
        invalid["extent_measurement"]["disk_bytes"] = 0
        with self.assertRaises(ValueError):
            space.compare(invalid, snapshot())

    def test_refuses_dangling_symlink_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "report.json"
            target.symlink_to(root / "missing-target")
            with self.assertRaises(ValueError):
                space._write_json(str(target), {"kind": "snapshot"})
            self.assertFalse((root / "missing-target").exists())

    def test_capture_and_compare_cli_json(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            p1, p2 = root / "before.json", root / "after.json"
            self.assertEqual(space.main(["capture", str(root), "--output", str(p1)]), 0)
            self.assertEqual(space.main(["capture", str(root), "--output", str(p2)]), 0)
            report = space.compare(json.loads(p1.read_text()), json.loads(p2.read_text()))
            self.assertEqual(report["kind"], "comparison")
            self.assertIsNone(report["game_extent_disk_reduction_bytes"])

    def test_atomic_report_replaces_existing_file_without_residual_staging(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "before.json"
            path.write_text("preserve until ready")
            space._write_json(str(path), {"kind": "snapshot", "version": 1})
            self.assertEqual(json.loads(path.read_text())["version"], 1)
            self.assertEqual([p.name for p in root.iterdir()], ["before.json"])

    def test_atomic_report_preserves_previous_content_when_replace_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "baseline.json"
            path.write_text("original baseline")
            with mock.patch.object(space.os, "replace", side_effect=OSError("simulated rename failure")):
                with self.assertRaisesRegex(OSError, "simulated rename failure"):
                    space._write_json(str(path), {"kind": "snapshot"})
            self.assertEqual(path.read_text(), "original baseline")
            staging = list(root.glob(".baseline.json.*.tmp/report"))
            self.assertEqual(len(staging), 1)
            self.assertEqual(stat.S_IMODE(staging[0].stat().st_mode), 0o600)

    def test_atomic_report_binds_publish_to_created_staging_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "baseline.json"
            path.write_text("original baseline")
            hidden = root / "held-open-staging"
            replacement = {}
            real_replace = os.replace

            def replace_public_staging_name(source, destination, **kwargs):
                public = next(root.glob(".baseline.json.*.tmp"))
                public.rename(hidden)
                public.mkdir()
                foreign = public / "report"
                foreign.write_text("foreign data")
                replacement["foreign"] = foreign
                return real_replace(source, destination, **kwargs)

            with mock.patch.object(space.os, "replace", side_effect=replace_public_staging_name):
                space._write_json(str(path), {"kind": "snapshot", "safe": True})

            self.assertTrue(json.loads(path.read_text())["safe"])
            self.assertEqual(replacement["foreign"].read_text(), "foreign data")

    def test_atomic_report_refuses_staging_directory_replaced_before_open(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "baseline.json"
            path.write_text("original baseline")
            hidden = root / "created-staging"
            real_open = os.open
            replaced = False

            def replace_before_open(name, flags, *args, **kwargs):
                nonlocal replaced
                if (not replaced and kwargs.get("dir_fd") is not None
                        and str(name).startswith(".baseline.json.")):
                    replaced = True
                    public = root / name
                    public.rename(hidden)
                    public.mkdir()
                return real_open(name, flags, *args, **kwargs)

            with mock.patch.object(space.os, "open", side_effect=replace_before_open):
                with self.assertRaisesRegex(OSError, "identity changed"):
                    space._write_json(str(path), {"kind": "snapshot", "safe": True})

            self.assertEqual(path.read_text(), "original baseline")
            self.assertTrue(hidden.is_dir())

    def test_atomic_report_does_not_unlink_reused_temp_name_after_publish(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "report.json"
            real_fsync = os.fsync
            fsync_calls = 0

            def fail_directory_fsync(descriptor):
                nonlocal fsync_calls
                fsync_calls += 1
                if fsync_calls == 2:
                    staging = next(root.glob(".report.json.*.tmp"))
                    (staging / "report").write_text("foreign data")
                    raise OSError("injected staging fsync failure")
                return real_fsync(descriptor)

            with mock.patch.object(space.os, "fsync", side_effect=fail_directory_fsync):
                with self.assertRaisesRegex(OSError, "injected staging fsync failure"):
                    space._write_json(str(path), {"kind": "snapshot", "safe": True})

            self.assertTrue(json.loads(path.read_text())["safe"])
            staging = next(root.glob(".report.json.*.tmp"))
            self.assertEqual((staging / "report").read_text(), "foreign data")

    def test_existing_symlink_output_does_not_modify_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            protected = root / "private.txt"
            protected.write_text("untouched")
            output = root / "report.json"
            output.symlink_to(protected)
            with self.assertRaisesRegex(ValueError, "symlink"):
                space._write_json(str(output), {"kind": "snapshot"})
            self.assertEqual(protected.read_text(), "untouched")
            self.assertTrue(output.is_symlink())
            self.assertEqual(sorted(p.name for p in root.iterdir()), ["private.txt", "report.json"])


if __name__ == "__main__":
    unittest.main()
