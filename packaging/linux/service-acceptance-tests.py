#!/usr/bin/env python3
"""Unprivileged tests. Never call a host manager, sudo, or the real application."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location(
    "acceptance", Path(__file__).with_name("service-acceptance.py")
)
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)


class AcceptanceTests(unittest.TestCase):
    def test_unit_names_cannot_address_production_units_or_options(self):
        self.assertEqual(
            acceptance.unit_name("test-1", "api"),
            "domus-acceptance-test-1-api.service",
        )
        for value in ("", "../api", "-a", "foo.service", "a b", "a\nb", "a" * 33):
            with self.subTest(value=value), self.assertRaises(ValueError):
                acceptance.unit_name(value, "api")
        with self.assertRaises(ValueError):
            acceptance.unit_name("test", "anything")

    def test_render_preserves_restrictions_and_budgets(self):
        root = Path("/var/lib/domus-acceptance-test")
        package = Path("/opt/domus-acceptance-test")
        for role in ("api", "worker"):
            source = (Path(__file__).parent / "systemd" /
                      f"domus-obscura-{role}.service").read_text()
            result = acceptance.render(source, root, package, "fixture", "fixture", "test")
            for line in source.splitlines():
                if not any(token in line for token in (
                    "domus-obscura", "/var/lib/domus-obscura", "/opt/domus-obscura"
                )):
                    self.assertIn(line, result.splitlines())
            self.assertIn("TimeoutStopSec=180\n", result)
            self.assertIn("User=fixture\n", result)
            self.assertNotIn("domus-obscura-api.service", result)
            self.assertIn(f"ReadWritePaths={root}\n", result)

    def test_only_mutating_manager_calls_use_noninteractive_sudo(self):
        with patch.object(acceptance, "command", return_value=(0, "")) as run:
            acceptance.manager(["show", "test.service"])
            self.assertEqual(run.call_args.args[0], ["systemctl", "--no-pager", "show", "test.service"])
            acceptance.manager(["stop", "test.service"], change=True, timeout=200)
            self.assertEqual(run.call_args.args[0][:4], ["sudo", "-n", "systemctl", "--no-pager"])
            self.assertEqual(run.call_args.args[1], 200)

    def test_command_is_bounded_and_does_not_expose_failure_output(self):
        with patch.object(acceptance.subprocess, "run") as run:
            run.return_value.returncode = 1
            with self.assertRaisesRegex(ValueError, r"tool failed \(1\)"):
                acceptance.command(["tool"], timeout=7)
            self.assertEqual(run.call_args.kwargs["timeout"], 7)
            self.assertNotIn("shell", run.call_args.kwargs)
            self.assertEqual(run.call_args.kwargs["stdin"], subprocess.DEVNULL)

    def test_wrong_host_does_not_invoke_installed_check(self):
        suite = acceptance.Suite.__new__(acceptance.Suite)
        with patch.object(acceptance.socket, "gethostname", return_value="host-a"):
            with patch.object(suite, "installed") as installed:
                with self.assertRaisesRegex(ValueError, "exact host"):
                    suite.approve("host-b")
                installed.assert_not_called()

    def test_active_requires_durable_progress_in_both_lanes(self):
        suite = acceptance.Suite.__new__(acceptance.Suite)
        snapshot = {
            "requests": [{"state": "running"}, {"state": "running"}],
            "scan": {"status": "running", "observed_count": 1},
            "publication": (4, 1),
        }
        with patch.object(suite, "snapshot", return_value=snapshot):
            self.assertTrue(suite.active())
            snapshot["scan"]["observed_count"] = 0
            self.assertFalse(suite.active())
            snapshot["scan"]["observed_count"] = 1
            snapshot["publication"] = (4, 4)
            self.assertFalse(suite.active())
            snapshot["publication"] = (4, 1)
            snapshot["requests"][1]["state"] = "complete"
            self.assertFalse(suite.active())

    def test_poll_has_a_deadline(self):
        with patch.object(acceptance.time, "monotonic", side_effect=[0, 0, 2]):
            with patch.object(acceptance.time, "sleep"):
                with self.assertRaisesRegex(TimeoutError, "not ready"):
                    acceptance.wait_for(lambda: False, 1, "not ready")

    def test_report_defaults_never_claim_manual_host_acceptance(self):
        self.assertIn("physical_cold_boot", acceptance.MANUAL)
        self.assertIn("manager_stop_timeout", acceptance.MANUAL)
        self.assertIn("changed_identity_remount", acceptance.MANUAL)

    def test_preflight_has_no_prepare_or_manager_effects(self):
        with patch.object(sys, "argv", [
            "acceptance", "preflight", "--root", "/var/lib/domus-acceptance-test",
            "--run-id", "test", "--package", "/opt/package",
        ]):
            with patch.object(acceptance, "Suite") as factory:
                suite = factory.return_value
                suite.report = {"checks": {"physical_reboot": "not_run"}}
                with patch("builtins.print"):
                    acceptance.main()
                suite.approve.assert_not_called()
                suite.start.assert_not_called()
                suite.save.assert_not_called()

    def test_failed_run_saves_evidence_without_deleting_state(self):
        with patch.object(sys, "argv", [
            "acceptance", "run", "--root", "/var/lib/domus-acceptance-test",
            "--run-id", "test", "--package", "/opt/package",
            "--approved-host", "host-a",
        ]):
            with patch.object(acceptance, "Suite") as factory:
                suite = factory.return_value
                suite.report = {"checks": {}}
                suite.lifecycle.side_effect = ValueError("fixture failure")
                with self.assertRaisesRegex(ValueError, "fixture failure"):
                    acceptance.main()
                suite.save.assert_called_once_with("lifecycle")
                suite.stop.assert_not_called()
                self.assertEqual(suite.report["checks"]["lifecycle"], "failed")

    def test_interruption_observation_failure_resumes_only_the_owned_worker(self):
        suite = acceptance.Suite.__new__(acceptance.Suite)
        suite.units = {"worker": "domus-acceptance-test-worker.service"}
        with patch.object(suite, "snapshot", return_value={}):
            with patch.object(suite, "start"):
                with patch.object(acceptance, "wait_for", side_effect=[True, TimeoutError("no stop")]):
                    with patch.object(acceptance, "manager") as manager:
                        with self.assertRaisesRegex(TimeoutError, "no stop"):
                            suite.recover(forced=True)
                        self.assertEqual(manager.call_args.args[0], [
                            "kill", "--kill-whom=main", "--signal=SIGCONT",
                            "domus-acceptance-test-worker.service",
                        ])

    def test_unexpected_acquisition_is_rejected_before_starting_any_unit(self):
        suite = acceptance.Suite.__new__(acceptance.Suite)
        suite.fixture = {
            "scan_request": 1, "publication_request": 2, "scan_location": 1,
            "job": 1, "version": 1, "normalization_run": 1, "configuration_binding": "a" * 64,
        }
        requests = [
            {"id": 1, "state": "queued", "body": '{"operation":"scan","location_id":1}',
             "configuration_binding": "a" * 64},
            {"id": 2, "state": "queued", "body": '{"operation":"acquire"}',
             "configuration_binding": "a" * 64},
        ]
        with patch.object(suite, "snapshot", return_value={"requests": requests}):
            with patch.object(acceptance, "manager") as manager:
                with self.assertRaisesRegex(ValueError, "only generated"):
                    suite.start()
                manager.assert_not_called()

    def test_configuration_rejects_live_credentials_and_external_state(self):
        fixture = {"database": "/var/lib/domus-acceptance-test/state/state.db",
                   "staging": "/var/lib/domus-acceptance-test/staging"}
        root = Path("/var/lib/domus-acceptance-test")
        package = Path("/opt/domus-acceptance-test")
        with patch.object(acceptance, "read", return_value=b"{}"):
            with self.assertRaisesRegex(ValueError, "offline fixture"):
                acceptance.fixture_configuration(root, fixture, package)
        expected = {
            "environment": "single_user", "system_settings_read_only": True,
            "services": {"discovery": [], "metadata": [],
                         "artwork": [], "acquisition": ["soulseek"],
                         "artist_images": []},
            "database": fixture["database"], "listen": "127.0.0.1:8080",
            "origin": "http://127.0.0.1:8080",
            "browser_assets": str(package / "browser"),
            "production_worker": {"queue_capacity": 64, "sharing": None,
                                 "acquisition": {
                                     "staging_root": fixture["staging"],
                                     "credentials_file": str(Path(fixture["database"]).parent /
                                                             "generated-credentials.json"),
                                     "server": "1.1.1.1:2242",
                                     "acknowledge_plaintext_login": True}},
        }
        with patch.object(acceptance, "read", side_effect=[
            json.dumps(expected).encode(),
            b'{"username":"generated-service-fixture","password":"not-a-live-credential"}',
        ]):
            acceptance.fixture_configuration(root, fixture, package)
        expected["browser_assets"] = "/outside/browser"
        with patch.object(acceptance, "read", return_value=json.dumps(expected).encode()):
            with self.assertRaisesRegex(ValueError, "offline fixture"):
                acceptance.fixture_configuration(root, fixture, package)
        expected["browser_assets"] = str(package / "browser")
        with patch.object(acceptance, "read", side_effect=[
            json.dumps(expected).encode(), b'{"username":"real","password":"real"}',
        ]):
            with self.assertRaisesRegex(ValueError, "live credentials"):
                acceptance.fixture_configuration(root, fixture, package)
        expected["database"] = "/outside/state.db"
        with patch.object(acceptance, "read", return_value=json.dumps(expected).encode()):
            with self.assertRaisesRegex(ValueError, "offline fixture"):
                acceptance.fixture_configuration(root, fixture, package)

    def test_cleanup_rechecks_units_and_preserves_changed_files(self):
        suite = acceptance.Suite.__new__(acceptance.Suite)
        suite.units = {"api": "domus-acceptance-test-api.service",
                       "worker": "domus-acceptance-test-worker.service"}
        with patch.object(suite, "stop") as stop:
            with patch.object(suite, "installed", side_effect=ValueError("unit changed")):
                with patch.object(acceptance, "manager") as manager:
                    with patch.object(acceptance, "command") as command:
                        with self.assertRaisesRegex(ValueError, "unit changed"):
                            suite.cleanup()
                        stop.assert_called_once_with(graceful=False)
                        manager.assert_called_once_with(
                            ["disable", *suite.units.values()], change=True
                        )
                        command.assert_not_called()

    def test_boot_observation_does_not_claim_physical_or_automatic_start(self):
        before = {
            "run_id": "test", "package_manifest_sha256": "a",
            "host": {"host": "test-host", "manager_start": "100", "boot_id": "a"},
            "snapshot": {"database_identity": [1, 2], "immutable_sha256": "unchanged"},
        }
        with patch.object(sys, "argv", [
            "acceptance", "boot-after", "--root", "/var/lib/domus-acceptance-test",
            "--run-id", "test", "--package", "/opt/package", "--before", "/old.json",
        ]):
            with patch.object(acceptance, "Suite") as factory:
                suite = factory.return_value
                suite.run_id = "test"
                suite.report = {
                    "package_manifest_sha256": "a",
                    "host": {"host": "test-host", "manager_start": "200", "boot_id": "a"},
                    "checks": {name: "not_run" for name in acceptance.MANUAL},
                }
                suite.snapshot.return_value = before["snapshot"].copy()
                with patch.object(acceptance, "read", return_value=json.dumps(before).encode()):
                    with patch.object(acceptance, "checked_path", side_effect=Path):
                        with patch("builtins.print"):
                            acceptance.main()
                self.assertEqual(suite.report["checks"]["running_after_observed_restart"], "passed")
                self.assertEqual(suite.report["checks"]["physical_cold_boot"], "not_run")
                self.assertNotIn("automatic_start", suite.report["checks"])
                suite.approve.assert_not_called()

    @unittest.skipUnless(sys.platform == "linux", "Linux path and no-follow semantics")
    def test_linked_inputs_and_parent_traversal_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "plain").write_text("generated")
            (root / "linked").symlink_to(root / "plain")
            with self.assertRaisesRegex(ValueError, "linked"):
                acceptance.read(root / "linked")
            with self.assertRaisesRegex(ValueError, "parent"):
                acceptance.checked_path(str(root / ".." / "unknown"))
            with self.assertRaisesRegex(ValueError, "read limit"):
                acceptance.read(root / "plain", 2)
            media = root / "Track \u00e9 \U0001f3b5.mp3"
            media.write_bytes(b"generated")
            self.assertEqual(acceptance.read(media), b"generated")
            self.assertEqual(acceptance.digest(media),
                             acceptance.hashlib.sha256(b"generated").hexdigest())
            with self.assertRaisesRegex(ValueError, "characters"):
                acceptance.checked_path(str(root / "control\nname"))

    def test_reports_never_overwrite_an_existing_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            with patch.object(acceptance, "checked_path", side_effect=lambda p: Path(p)):
                acceptance.write_new(path, {"state": "not_run"})
                with self.assertRaises(FileExistsError):
                    acceptance.write_new(path, {"state": "passed"})
                self.assertEqual(json.loads(path.read_text()), {"state": "not_run"})


if __name__ == "__main__":
    unittest.main()
