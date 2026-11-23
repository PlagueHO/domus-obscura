"""Check helper export without changing the cached compiler artifact."""

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "build_media_fixture", Path(__file__).with_name("build-media-fixture.py")
)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class MediaFixtureTests(unittest.TestCase):
    def test_export_strips_only_the_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "cached"
            output = Path(directory) / "exported"
            artifact.write_bytes(b"executable with debug sections")

            def strip(command, **kwargs):
                self.assertEqual(command, ["strip", "--strip-debug", str(output)])
                self.assertTrue(kwargs["check"])
                self.assertEqual(output.read_bytes(), artifact.read_bytes())
                output.write_bytes(b"executable")

            with patch.object(fixture.subprocess, "run", side_effect=strip):
                fixture.export_executable(artifact, output)
            self.assertEqual(output.read_bytes(), b"executable")
            self.assertEqual(artifact.read_bytes(), b"executable with debug sections")

    def test_strip_failure_is_not_success(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "cached"
            output = Path(directory) / "exported"
            artifact.write_bytes(b"executable with debug sections")
            error = subprocess.CalledProcessError(1, ["strip"])
            with patch.object(fixture.subprocess, "run", side_effect=error):
                with self.assertRaises(subprocess.CalledProcessError):
                    fixture.export_executable(artifact, output)
            self.assertEqual(artifact.read_bytes(), b"executable with debug sections")


if __name__ == "__main__":
    unittest.main()
