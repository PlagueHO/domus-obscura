#!/usr/bin/env python3
"""Isolated inventory and memory-evidence checks."""

import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest import mock
import subprocess
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location(
    "physical_scale", Path(__file__).with_name("physical-scale.py"))
scale = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scale)


class EvidenceTests(unittest.TestCase):
    def test_inventory_counts_files_without_exporting_names(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "private-album").mkdir()
            (root / "private-album" / "private-song.mp3").write_bytes(b"generated")
            (root / "note.custom").write_bytes(b"other")
            result = scale.Inventory()
            if os.name == "nt":
                scale.Windows().inventory(root, result)
            else:
                scale.inventory_posix(root, result)
            report = result.report()
            self.assertTrue(report["complete"])
            self.assertEqual(report["regular_files"], 2)
            self.assertEqual(report["full_profile_files"], 4)
            self.assertEqual(report["logical_bytes"], 14)
            self.assertEqual(report["directories"], 2)
            self.assertEqual(report["extensions"], {".mp3": 1, "other": 1})
            self.assertNotIn("private", json.dumps(report))

    @unittest.skipIf(os.name == "nt", "Windows junction case uses the native workflow fixture")
    def test_inventory_does_not_follow_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "file.mp3").write_bytes(b"generated")
            (root / "loop").symlink_to(root, target_is_directory=True)
            result = scale.Inventory()
            scale.inventory_posix(root, result)
            self.assertEqual(result.report()["excluded_links_or_reparse_points"], 1)
            self.assertEqual(result.files, 1)

    @unittest.skipIf(os.name == "nt", "POSIX directory-entry race")
    def test_live_storage_ignores_only_disappearing_entries(self):
        class Entry:
            name = "removed.mp3"

            def __init__(self, error):
                self.error = error

            def stat(self, follow_symlinks):
                raise self.error

        class Entries:
            def __init__(self, error):
                self.error = error

            def __enter__(self):
                return iter((Entry(self.error),))

            def __exit__(self, *_):
                return False

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original_scandir = scale.os.scandir

            def changing_scandir(path, error):
                if isinstance(path, int):
                    return Entries(error)
                return original_scandir(path)

            with mock.patch.object(
                scale.os, "scandir",
                side_effect=lambda path: changing_scandir(
                    path, FileNotFoundError(2, "entry disappeared")
                ),
            ):
                self.assertEqual(scale.storage(root), 4096)
            with mock.patch.object(
                scale.os, "scandir",
                side_effect=lambda path: changing_scandir(
                    path, PermissionError(13, "access denied")
                ),
            ):
                with self.assertRaisesRegex(ValueError, "cannot verify"):
                    scale.storage(root)

    @unittest.skipUnless(os.name == "nt", "Windows directory handle sharing")
    def test_live_storage_directory_handles_allow_delete_sharing(self):
        windows = scale.Windows.__new__(scale.Windows)
        windows.kernel = mock.Mock()
        windows.kernel.CreateFileW.return_value = 1

        def directory_info(_handle, _kind, buffer, _length):
            info = scale.ctypes.cast(
                buffer, scale.ctypes.POINTER(scale.Windows.AttributeTag)
            ).contents
            info.attributes = scale.Windows.DIRECTORY
            return True

        windows.kernel.GetFileInformationByHandleEx.side_effect = directory_info
        path = Path(tempfile.gettempdir())
        handle = windows.directory(path, allow_delete=True)
        self.assertEqual(windows.kernel.CreateFileW.call_args.args[2], 0x7)
        windows.kernel.CloseHandle(handle)

    @unittest.skipUnless(os.name == "nt", "Windows junction handle check")
    def test_windows_inventory_excludes_a_junction_and_rejects_it_as_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            outside = root / "outside"
            source.mkdir()
            outside.mkdir()
            (outside / "sentinel.txt").write_bytes(b"outside")
            junction = source / "link"
            subprocess.run(
                ["cmd.exe", "/d", "/c", "mklink", "/J", str(junction), str(outside)],
                check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                result = scale.Inventory()
                scale.Windows().inventory(source, result)
                self.assertEqual(result.links, 1)
                self.assertEqual(result.files, 0)
                with self.assertRaisesRegex(ValueError, "no-follow"):
                    scale.Windows().inventory(junction, scale.Inventory())
                self.assertEqual((outside / "sentinel.txt").read_bytes(), b"outside")
            finally:
                junction.rmdir()

    def test_incomplete_inventory_cannot_admit_full_profile(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "manifest.json"
            result = scale.Inventory()
            result.file("generated.mp3", 1)
            result.error(PermissionError(13, "sensitive path must not be stored"))
            scale.write_new(output, result.report())
            with self.assertRaisesRegex(ValueError, "complete inventory"):
                scale.read_manifest(output)
            self.assertNotIn("sensitive", output.read_text())
            self.assertIsNone(result.report()["full_profile_files"])

    def test_inventory_rejects_output_inside_source_before_writing(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "must-not-exist.json"
            with self.assertRaisesRegex(ValueError, "read-only source"):
                scale.main(["inventory", "--root", temporary, "--output", str(output)])
            self.assertFalse(output.exists())

    def test_manifest_binds_exact_doubled_count(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "manifest.json"
            result = scale.Inventory()
            result.file("generated.flac", 100)
            scale.write_new(output, result.report())
            manifest, digest = scale.read_manifest(output)
            self.assertEqual(manifest["full_profile_files"], 2)
            self.assertEqual(len(digest), 64)
            manifest["full_profile_files"] = 3
            changed = Path(temporary) / "changed.json"
            scale.write_new(changed, manifest)
            with self.assertRaisesRegex(ValueError, "doubled"):
                scale.read_manifest(changed)

    def test_native_kib_units_are_converted_to_bytes(self):
        self.assertEqual(
            scale.parse_kib_fields("Rss: 123 kB\nVmHWM: 456 kB\nVmSize: 999 kB"),
            {"Rss": 123 * 1024, "VmHWM": 456 * 1024})
        for text in ("Rss: 1 MB", "VmHWM: -1 kB"):
            with self.assertRaises(ValueError):
                scale.parse_kib_fields(text)

    def test_threshold_uses_combined_samples_not_largest_process(self):
        samples = [{"api_bytes": 300 * 1024**2, "worker_bytes": 300 * 1024**2,
                    "phase": "active"}]
        result = scale.memory_summary(samples, {"api": 1, "worker": 1}, True, True)
        self.assertEqual(result["verdict"], "failed")

    def test_exact_512_mib_boundary(self):
        half = scale.LIMIT_BYTES // 2
        samples = [{"api_bytes": half, "worker_bytes": half, "phase": "active"}]
        self.assertEqual(scale.memory_summary(
            samples, {"api": half, "worker": half}, True, True)["verdict"],
            "passed_profile_only")
        samples[0]["worker_bytes"] += 1
        self.assertEqual(scale.memory_summary(
            samples, {"api": half, "worker": half + 1}, True, True)["verdict"], "failed")

    def test_peak_sum_is_not_reported_as_simultaneous_peak(self):
        samples = [{"api_bytes": 100, "worker_bytes": 200, "phase": "active"}]
        result = scale.memory_summary(samples, {"api": 500, "worker": 600}, True, True)
        self.assertEqual(result["sampled_combined_peak_bytes"], 300)
        self.assertEqual(result["conservative_sum_of_peaks_bytes"], 1100)
        self.assertEqual(result["verdict"], "passed_profile_only")
        peaks = {"api": scale.LIMIT_BYTES, "worker": 600}
        self.assertEqual(scale.memory_summary(samples, peaks, True, True)["verdict"],
                         "inconclusive")

    def test_process_peak_above_limit_fails_without_a_paired_sample(self):
        samples = [{"api_bytes": 100, "worker_bytes": 200, "phase": "active"}]
        peaks = {"api": scale.LIMIT_BYTES + 1, "worker": 200}
        result = scale.memory_summary(samples, peaks, True, True)
        self.assertEqual(result["sampled_combined_peak_bytes"], 300)
        self.assertEqual(result["verdict"], "failed")

    def test_missing_phase_or_samples_never_pass(self):
        peaks = {"api": 10, "worker": 20}
        sample = [{"api_bytes": 10, "worker_bytes": 20, "phase": "active"}]
        for samples, complete, active in (([], True, True), (sample, False, True),
                                          (sample, True, False)):
            self.assertEqual(scale.memory_summary(samples, peaks, complete, active)["verdict"],
                             "inconclusive")

    def test_inconsistent_peak_evidence_never_passes(self):
        sample = [{"api_bytes": 100, "worker_bytes": 200, "phase": "active"}]
        result = scale.memory_summary(sample, {"api": 99, "worker": 200}, True, True)
        self.assertFalse(result["complete"])
        self.assertEqual(result["verdict"], "inconclusive")

    def test_restart_peak_uses_maximum_segment_and_the_verified_steady_window(self):
        with tempfile.TemporaryDirectory() as temporary:
            monitor = scale.Monitor(Path(temporary), scale.admission("small"))
            monitor.retired = {"api": [(100, "api")],
                               "worker": [(200, "worker-one"), (300, "worker-two")]}
            monitor.samples = [
                {"phase": "startup", "elapsed_seconds": 0, "api_bytes": 80, "worker_bytes": 300},
                {"phase": "workload", "elapsed_seconds": 2, "api_bytes": 80, "worker_bytes": 100},
                {"phase": "workload", "elapsed_seconds": 4, "api_bytes": 80, "worker_bytes": 200},
            ]
            result = monitor.finish(True, (1, 4))
            self.assertEqual(result["conservative_sum_of_peaks_bytes"], 400)
            self.assertEqual(result["sampled_combined_peak_bytes"], 380)
            self.assertEqual(result["steady_mean_bytes"], 230)
            self.assertEqual(result["process_start_identities"]["worker"],
                             ["worker-one", "worker-two"])
            self.assertEqual(result["verdict"], "passed_profile_only")

    def test_current_process_has_real_resident_memory(self):
        process = scale.Windows().process(os.getpid()) if os.name == "nt" else (
            scale.LinuxProcess(os.getpid()))
        try:
            sample = process.sample()
            self.assertGreater(sample, 0)
            self.assertGreaterEqual(process.peak, sample)
        finally:
            process.close()

    @unittest.skipUnless(os.name == "nt", "Windows retained process handle")
    def test_windows_peak_remains_available_after_child_exit(self):
        child = subprocess.Popen(
            [sys.executable, "-c", "import time; data = bytearray(1024 * 1024); time.sleep(.2)"])
        native = scale.Windows().process(child.pid)
        try:
            child.wait(timeout=10)
            native.sample()
            self.assertGreater(native.peak, 0)
        finally:
            native.close()
            if child.poll() is None:
                child.kill()
            child.wait(timeout=10)

    def test_full_profile_cannot_default_to_small_or_ignore_approval(self):
        for manifest, approved in ((None, None), ("absent.json", None)):
            with self.assertRaisesRegex(ValueError, "separate lab admission"):
                scale.admission("full", manifest, approved)
        small = scale.admission("small")
        self.assertEqual(small["files"], 4000)
        self.assertEqual(small["max_generated_files"], 4096)
        self.assertEqual(small["max_run_bytes"], 1024**3)

    def test_full_profile_requires_current_platform_and_exact_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inventory = scale.Inventory()
            inventory.file("generated.mp3", 1)
            manifest = root / "manifest.json"
            scale.write_new(manifest, inventory.report())
            _, digest = scale.read_manifest(manifest)
            approved = {
                "schema": 1, "baseline_sha256": digest,
                "platform": scale.platform.system(), "azure_only": True,
                "separate_resource_approval": True, "cleanup_verified": True,
                "approval_reference": "generated approval test",
                "vm_id": "generated-vm-identity",
                "max_run_bytes": 100000, "deadline_unix": time.time() + 60,
                "max_runtime_seconds": 30,
                "linux_permission_fixture_approved": True,
            }
            path = root / "approved.json"
            scale.write_new(path, approved)
            self.assertEqual(scale.admission("full", manifest, path)["files"], 2)
            approved["baseline_sha256"] = "0" * 64
            changed = root / "changed.json"
            scale.write_new(changed, approved)
            with self.assertRaisesRegex(ValueError, "does not match"):
                scale.admission("full", manifest, changed)

    def test_linux_full_profile_requires_explicit_permission_fixture_approval(self):
        manifest = {"full_profile_files": 2}
        approved = {
            "schema": 1, "baseline_sha256": "generated",
            "platform": "Linux", "azure_only": True,
            "separate_resource_approval": True, "cleanup_verified": True,
            "approval_reference": "generated approval test", "vm_id": "generated",
            "max_run_bytes": 100000, "deadline_unix": time.time() + 60,
            "max_runtime_seconds": 30,
        }
        with mock.patch.object(scale, "read_manifest", return_value=(manifest, "generated")), \
                mock.patch.object(scale.platform, "system", return_value="Linux"), \
                mock.patch("builtins.open", mock.mock_open(read_data=json.dumps(approved))):
            with self.assertRaisesRegex(ValueError, "separate Linux permission approval"):
                scale.admission("full", "manifest", "approval")
        with self.assertRaisesRegex(ValueError, "separate Linux permission approval"):
            with scale.linux_denied_file(Path.cwd(), False):
                self.fail("unapproved permission fixture was admitted")

    def test_linux_denial_restores_mode_on_failure_and_rejects_bypassed_denial(self):
        root = Path.cwd()
        info = mock.Mock(st_mode=0o100640, st_dev=1, st_ino=2, st_size=3, st_mtime_ns=4)
        for denied in (False, True):
            with self.subTest(denied=denied):
                attempts = 0

                def opened(name, *_args, **_kwargs):
                    nonlocal attempts
                    if name == "track-000000000000.mp3":
                        attempts += 1
                        if attempts == 2 and denied:
                            raise PermissionError("generated denial")
                        return 42
                    return 11

                chmod = mock.Mock()
                with mock.patch.object(scale.platform, "system", return_value="Linux"), \
                        mock.patch.multiple(
                            scale.os, create=True, O_DIRECTORY=1, O_NOFOLLOW=2, O_CLOEXEC=4,
                            open=mock.Mock(side_effect=opened), close=mock.Mock(),
                            fstat=mock.Mock(return_value=info), fchmod=chmod):
                    expected = RuntimeError if denied else ValueError
                    with self.assertRaises(expected):
                        with scale.linux_denied_file(root, True) as proof:
                            self.assertTrue(proof["actual_os_denial"])
                            raise RuntimeError("generated workload failure")
                self.assertEqual(chmod.call_args_list,
                                 [mock.call(42, 0o200), mock.call(42, 0o640)])

    def test_inventory_tracks_allocation_not_only_small_logical_files(self):
        result = scale.Inventory()
        result.file("tiny.mp3", 1, 4096)
        self.assertEqual(result.report()["allocated_file_bytes"], 4096)

    def test_imds_uses_no_proxy_and_rejects_another_vm(self):
        response = mock.MagicMock()
        response.__enter__.return_value.read.return_value = b'{"vmId":"another"}'
        opener = mock.MagicMock()
        opener.open.return_value = response
        with mock.patch.object(scale.urllib.request, "build_opener", return_value=opener):
            with self.assertRaisesRegex(ValueError, "not the approved Azure"):
                scale.verify_azure_vm("expected")
        request = opener.open.call_args.args[0]
        self.assertEqual(request.full_url.split("/")[2], "169.254.169.254")
        self.assertEqual(request.get_header("Metadata"), "true")
        self.assertEqual(opener.open.call_args.kwargs["timeout"], 3)

    def test_private_response_is_bounded_and_surfaces_errors(self):
        child = scale.Child.__new__(scale.Child)
        child.buffer = b""
        child.channel = mock.Mock()
        child.channel.recv.return_value = b'{"error":"generated failure"}\n'
        with self.assertRaisesRegex(ValueError, "generated failure"):
            child.receive()
        child.buffer = b""
        child.channel.recv.return_value = b"x" * (64 * 1024 + 1)
        with self.assertRaisesRegex(ValueError, "exceeds limit"):
            child.receive()

    def test_run_admission_fails_before_creating_an_undersized_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "new-run"
            args = scale.argparse.Namespace(test_host=__file__, output=str(output))
            settings = scale.admission("small")
            settings["max_run_bytes"] = 1
            with self.assertRaisesRegex(ValueError, "cannot admit"):
                scale.new_run(args, settings)
            self.assertFalse(output.exists())

    def test_run_never_reuses_existing_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            args = scale.argparse.Namespace(test_host=__file__, output=temporary)
            sentinel = Path(temporary) / "sentinel"
            sentinel.write_bytes(b"unchanged")
            with self.assertRaises(FileExistsError):
                scale.new_run(args, scale.admission("small"))
            self.assertEqual(sentinel.read_bytes(), b"unchanged")

    def test_cleanup_rejects_an_unowned_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            root = output / "fixture"
            root.mkdir()
            scale.write_new(output / "ownership.json", {"schema": 1, "run": "another-run"})
            with self.assertRaisesRegex(ValueError, "ownership"):
                scale.remove_fixture(output, root)
            self.assertTrue(root.exists())

    @unittest.skipUnless(os.name == "nt", "Windows long-path cleanup")
    def test_cleanup_removes_long_lock_paths_without_following_a_junction(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / ("a" * 120)
            output.mkdir()
            scale.write_new(output / "ownership.json", {"schema": 1, "run": output.name})
            root = output / "fixture"
            root.mkdir()
            resolved = str(root.resolve())
            if resolved.startswith("\\\\?\\"):
                resolved = resolved[4:]
            self.assertTrue(scale._same_native_path(
                Path("\\\\?\\" + resolved), root.absolute()))
            extended = Path("\\\\?\\" + str(root))
            (extended / ("generated-lock-" + "0" * 120)).write_bytes(b"generated")
            outside = Path(temporary) / "outside"
            outside.mkdir()
            sentinel = outside / "sentinel.txt"
            sentinel.write_bytes(b"unchanged")
            subprocess.run(
                ["cmd.exe", "/d", "/c", "mklink", "/J", str(root / "link"), str(outside)],
                check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            scale.remove_fixture(output, root)
            self.assertFalse(root.exists())
            self.assertEqual(sentinel.read_bytes(), b"unchanged")

    @unittest.skipIf(os.name == "nt", "Linux process identity check")
    def test_reused_process_identity_fails_instead_of_sampling_another_process(self):
        process = scale.LinuxProcess(os.getpid())
        with mock.patch.object(scale, "linux_identity", return_value="different"):
            with self.assertRaisesRegex(ValueError, "identity changed"):
                process.sample()


if __name__ == "__main__":
    unittest.main()
