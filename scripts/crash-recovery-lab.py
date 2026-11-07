#!/usr/bin/env python3
"""Select one explicit disposable-volume test without privileged disk operations."""

import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import unittest

CONSENT = "disposable-volume-faults"
CASES = (
    "publication_full",
    "receive_full",
    "normalization_full",
    "database_full",
    "backup_detached",
    "backup_wrong_volume",
)
STEPS = ("detach-backup", "swap-backup", "return-original-backup")


def parse(arguments):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("initialize", "run", "continue", "self-test"))
    parser.add_argument("--consent")
    parser.add_argument("--case", choices=CASES)
    parser.add_argument("--step", choices=STEPS)
    for name in ("descriptor", "root", "image", "evidence", "run", "replacement", "replacement-image"):
        parser.add_argument(f"--{name}")
    args = parser.parse_args(arguments)
    if args.action == "self-test":
        return args
    if args.consent != CONSENT:
        parser.error(f"--consent {CONSENT} is required")
    required = {
        "initialize": ("root", "image", "evidence", "run"),
        "run": ("descriptor", "case"),
        "continue": ("descriptor", "step"),
    }
    if any(not getattr(args, name) for name in required[args.action]):
        parser.error(f"{args.action} requires {', '.join(required[args.action])}")
    return args


def main(arguments):
    args = parse(arguments)
    if args.action == "self-test":
        program = unittest.main(argv=[sys.argv[0]], exit=False)
        return 0 if program.result.wasSuccessful() else 1
    if args.action == "continue":
        lab = json.loads(Path(args.descriptor).read_text(encoding="utf-8"))
        directory = Path(lab["evidence"]["path"])
        if (lab["consent"] != CONSENT or not re.fullmatch(r"[A-Za-z0-9-]{1,64}", lab["run"])
                or directory != Path(args.descriptor).absolute().parent):
            raise ValueError("Descriptor must be in its bound evidence directory.")
        request = directory / f"{args.step}.request.json"
        for path in (request, directory, *directory.parents):
            if stat.S_ISLNK(path.lstat().st_mode):
                raise ValueError("Links are not permitted in operator signal paths.")
        pending = json.loads(request.read_text(encoding="utf-8"))
        if pending != {"run": lab["run"], "step": args.step}:
            raise ValueError("The operator request does not match this run.")
        with (directory / f"{args.step}.continue").open("x", encoding="utf-8") as signal:
            signal.write(lab["run"])
            signal.flush()
            os.fsync(signal.fileno())
        return 0
    env = {name: value for name, value in os.environ.items() if not name.startswith("DOMUS_LAB_")}
    env["DOMUS_LAB_CONSENT"] = args.consent
    if args.action == "initialize":
        for name in ("root", "image", "evidence", "run", "replacement", "replacement_image"):
            value = getattr(args, name)
            if value is not None:
                env[f"DOMUS_LAB_{name.upper()}"] = value
        test = "volume_lab_initialize"
    else:
        env["DOMUS_LAB_DESCRIPTOR"] = args.descriptor
        test = f"volume_lab_{args.case}"
    result = subprocess.run(
        ["cargo", "task", "test", "--lib", f"crash_tests::album::lab::{test}",
         "--", "--exact", "--ignored", "--nocapture"],
        cwd=Path(__file__).resolve().parent.parent,
        env=env,
        check=False,
    )
    if result.returncode:
        print("Lab failed. Retain all fixture evidence.", file=sys.stderr)
    return result.returncode


class WrapperTests(unittest.TestCase):
    def test_rejects_missing_or_wrong_consent(self):
        for arguments in (
            ["run", "--case", "publication_full"],
            ["run", "--consent", "wrong", "--case", "publication_full"],
            ["run", "--consent", CONSENT],
            ["initialize", "--consent", CONSENT],
            ["continue", "--consent", CONSENT],
            ["run", "--consent", CONSENT, "--case", "crash_child"],
        ):
            with contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    parse(arguments)
            self.assertNotEqual(error.exception.code, 0)

    def test_selects_only_named_cases(self):
        for case in CASES:
            args = parse(["run", "--consent", CONSENT, "--case", case, "--descriptor", "unused"])
            self.assertEqual(args.case, case)

    def test_operator_signals_require_a_matching_request_and_cannot_overwrite(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).absolute()
            descriptor = directory / "lab.json"
            descriptor.write_text(json.dumps({
                "consent": CONSENT, "run": "generated-case",
                "evidence": {"path": str(directory)},
            }), encoding="utf-8")
            request = directory / "detach-backup.request.json"
            arguments = ["continue", "--consent", CONSENT, "--descriptor", str(descriptor),
                         "--step", "detach-backup"]
            request.write_text(json.dumps({"run": "different", "step": "detach-backup"}), encoding="utf-8")
            with self.assertRaises(ValueError):
                main(arguments)
            signal = directory / "detach-backup.continue"
            self.assertFalse(signal.exists())
            request.write_text(json.dumps({"run": "generated-case", "step": "detach-backup"}), encoding="utf-8")
            self.assertEqual(main(arguments), 0)
            self.assertEqual(signal.read_text(encoding="utf-8"), "generated-case")
            with self.assertRaises(FileExistsError):
                main(arguments)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
