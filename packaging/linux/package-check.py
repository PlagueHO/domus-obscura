"""Bounded archive, installed-payload, and isolated Linux process checks."""

import argparse
import gzip
import hashlib
import http.client
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import signal
import shutil
import socket
import sqlite3
import stat
import subprocess
import tarfile
import tempfile
import time
import unittest

MAX_FILES = 8192
MAX_FILE = 256 * 1024 * 1024
MAX_TOTAL = 512 * 1024 * 1024
MAX_MANIFEST = 2 * 1024 * 1024
TARGET = "x86_64-unknown-linux-gnu"


class PackageTarInfo(tarfile.TarInfo):
    @classmethod
    def fromtarfile(cls, source):
        # Some Python builds bypass frombuf overrides in their internal parser.
        # Check the header before tarfile can consume extended metadata bodies.
        member = cls.frombuf(
            source.fileobj.read(tarfile.BLOCKSIZE), source.encoding, source.errors
        )
        member.offset = source.fileobj.tell() - tarfile.BLOCKSIZE
        return member._proc_member(source)

    @classmethod
    def frombuf(cls, buffer, encoding, errors):
        member = super().frombuf(buffer, encoding, errors)
        # Reject extended headers before tarfile reads their metadata bodies.
        if member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE):
            raise ValueError("only ordinary ustar files and directories are accepted")
        if member.size < 0 or member.size > MAX_FILE:
            raise ValueError("archive header exceeds its size limit")
        if member.isdir() and member.size != 0:
            raise ValueError("archive directory headers must not contain data")
        return member


def name_checked(name):
    parts = name.split("/")
    if (
        not name
        or len(name) > 512
        or len(parts) > 16
        or any(part in ("", ".", "..") for part in parts)
        or any(ord(char) < 32 or ord(char) > 126 or char in "\\:" for char in name)
    ):
        raise ValueError("unsafe package path")
    return name


def unpack(archive, destination):
    parent = root_handle(archive.absolute().parent)
    try:
        archive_data, _ = read_file(parent, archive.name, MAX_FILE)
    finally:
        os.close(parent)
    seen = set()
    root_seen = False
    total = 0
    with gzip.GzipFile(fileobj=io.BytesIO(archive_data)) as expanded, tarfile.open(
        fileobj=expanded, mode="r|", tarinfo=PackageTarInfo
    ) as source:
        for count, member in enumerate(source, 1):
            if count > MAX_FILES:
                raise ValueError("excessive archive entries")
            name = member.name
            if name in (".", "./") and member.isdir():
                if root_seen or member.mode != 0o755:
                    raise ValueError("duplicate or unsafe archive root directory")
                root_seen = True
                continue
            if name.startswith("./"):
                name = name[2:]
            name = name_checked(name.rstrip("/") if member.isdir() else name)
            if name in seen or len(seen) >= MAX_FILES:
                raise ValueError("duplicate or excessive archive entries")
            seen.add(name)
            if not (member.isdir() or member.isreg()) or member.issparse():
                raise ValueError("links and special archive entries are forbidden")
            expected_mode = 0o755 if member.isdir() or name == "domus" else 0o644
            if member.mode != expected_mode:
                raise ValueError("unsafe archive permissions")
            if member.size < 0 or member.size > MAX_FILE:
                raise ValueError("archive member exceeds its limit")
            total += member.size
            if total > MAX_TOTAL:
                raise ValueError("expanded archive exceeds its limit")
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            if member.isdir():
                target.mkdir(exist_ok=True)
            else:
                with source.extractfile(member) as data, target.open("xb") as output:
                    remaining = member.size
                    while remaining:
                        block = data.read(min(remaining, 1024 * 1024))
                        if not block:
                            raise ValueError("incomplete archive member")
                        output.write(block)
                        remaining -= len(block)
            target.chmod(expected_mode)
        # Consume the gzip footer so a truncated stream or bad CRC is an error.
        while expanded.read(1024 * 1024):
            if expanded.tell() > MAX_TOTAL + MAX_FILES * 1024 + 32768:
                raise ValueError("expanded archive stream exceeds its limit")


def root_handle(path):
    path = Path(path)
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError("payload root must be an absolute path")
    current = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:]:
            next_fd = os.open(
                part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=current
            )
            os.close(current)
            current = next_fd
        return current
    except BaseException:
        os.close(current)
        raise


def read_file(directory, name, maximum):
    descriptor = os.open(
        name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory
    )
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_nlink != 1
            or metadata.st_size > maximum
        ):
            raise ValueError("payload file type or size is invalid")
        data = source.read(maximum + 1)
        if len(data) > maximum:
            raise ValueError("payload file exceeds its limit")
        return data, stat.S_IMODE(metadata.st_mode)


def verify_handle(root):
    manifest_data, mode = read_file(root, "package-manifest.json", MAX_MANIFEST)
    if mode != 0o644:
        raise ValueError("invalid manifest permissions")
    manifest = json.loads(manifest_data)
    if manifest.get("format") != 1 or manifest.get("target") != TARGET:
        raise ValueError("unsupported Linux manifest")
    expected = manifest.get("files")
    modes = manifest.get("modes")
    if not isinstance(expected, dict) or not isinstance(modes, dict):
        raise ValueError("manifest file maps are missing")
    if len(expected) > MAX_FILES or expected.keys() != modes.keys():
        raise ValueError("invalid manifest file maps")
    for name, digest in expected.items():
        name_checked(name)
        if (
            name == "package-manifest.json"
            or not isinstance(digest, str)
            or len(digest) != 64
            or any(char not in "0123456789abcdef" for char in digest)
            or modes[name] != (0o755 if name == "domus" else 0o644)
        ):
            raise ValueError("invalid manifest file evidence")
    for name in (
        "domus",
        "browser/index.html",
        "notices/components.json",
        "systemd/domus-obscura-api.service",
        "systemd/domus-obscura-worker.service",
        "linux-services.md",
        "package-check.py",
    ):
        if name not in expected:
            raise ValueError("required package file is missing")
    observed = {}
    directories = set()
    total = 0
    for directory, children, names, handle in os.fwalk(
        ".", dir_fd=root, follow_symlinks=False
    ):
        if len(observed) + len(directories) > MAX_FILES:
            raise ValueError("payload entry count exceeds its limit")
        prefix = "" if directory == "." else directory[2:] + "/"
        for child in children:
            if len(observed) + len(directories) >= MAX_FILES:
                raise ValueError("payload entry count exceeds its limit")
            name = name_checked(prefix + child)
            metadata = os.stat(child, dir_fd=handle, follow_symlinks=False)
            if not stat.S_ISDIR(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o755:
                raise ValueError("payload contains an unsafe directory")
            directories.add(name)
        for child in names:
            name = name_checked(prefix + child)
            if name == "package-manifest.json":
                continue
            if len(observed) + len(directories) >= MAX_FILES:
                raise ValueError("payload entry count exceeds its limit")
            data, mode = read_file(handle, child, MAX_FILE)
            total += len(data)
            if total > MAX_TOTAL:
                raise ValueError("payload exceeds its total size limit")
            observed[name] = hashlib.sha256(data).hexdigest()
            if name not in modes or mode != modes[name]:
                raise ValueError("payload file permissions differ")
    owned_directories = {
        str(parent)
        for name in expected
        for parent in PurePosixPath(name).parents
        if str(parent) != "."
    }
    if observed != expected or directories != owned_directories:
        raise ValueError("payload changed or contains unrelated entries; files are preserved")
    return manifest, manifest_data, directories


def installed(path, remove=False):
    root = root_handle(path)
    try:
        manifest, manifest_data, directories = verify_handle(root)
        if remove:
            # Each parent and file is reopened without following links.
            # Unknown entries never become part of the deletion list.
            for name, digest in manifest["files"].items():
                parent = os.dup(root)
                try:
                    parts = name.split("/")
                    for part in parts[:-1]:
                        next_fd = os.open(
                            part,
                            os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=parent,
                        )
                        os.close(parent)
                        parent = next_fd
                    data, mode = read_file(parent, parts[-1], MAX_FILE)
                    if (
                        hashlib.sha256(data).hexdigest() != digest
                        or mode != manifest["modes"][name]
                    ):
                        raise ValueError("owned file changed before removal")
                    os.unlink(parts[-1], dir_fd=parent)
                finally:
                    os.close(parent)
            for name in sorted(directories, key=lambda name: (-name.count("/"), name)):
                parent = os.dup(root)
                try:
                    parts = name.split("/")
                    for part in parts[:-1]:
                        next_fd = os.open(
                            part,
                            os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=parent,
                        )
                        os.close(parent)
                        parent = next_fd
                    os.rmdir(parts[-1], dir_fd=parent)
                finally:
                    os.close(parent)
            current, mode = read_file(root, "package-manifest.json", MAX_MANIFEST)
            if current != manifest_data or mode != 0o644:
                raise ValueError("manifest changed before removal; removal is incomplete")
            os.unlink("package-manifest.json", dir_fd=root)
    finally:
        os.close(root)


def verify_units(units, executable):
    with tempfile.TemporaryDirectory(prefix="domus-unit-verify-") as directory:
        root = Path(directory)
        destination = root / "etc/systemd/system"
        destination.mkdir(parents=True)
        for name in ("domus-obscura-api.service", "domus-obscura-worker.service"):
            shutil.copyfile(units / name, destination / name)
        for name in (
            "basic.target",
            "sysinit.target",
            "shutdown.target",
            "multi-user.target",
            "network-online.target",
        ):
            (destination / name).write_text(
                "[Unit]\nDescription=Isolated verification target\nDefaultDependencies=no\n"
            )
        program = root / "opt/domus-obscura/domus"
        program.parent.mkdir(parents=True)
        shutil.copyfile(executable, program)
        program.chmod(0o755)
        command = root / "usr/bin/systemctl"
        command.parent.mkdir(parents=True)
        shutil.copyfile("/usr/bin/systemctl", command)
        command.chmod(0o755)
        (root / "var/lib/domus-obscura/config").mkdir(parents=True)
        subprocess.run(
            [
                "systemd-analyze",
                "--man=no",
                "--root=" + str(root),
                "verify",
                "domus-obscura-api.service",
                "domus-obscura-worker.service",
            ],
            check=True,
            timeout=30,
        )


def request(path):
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=1)
    try:
        connection.request("GET", path)
        response = connection.getresponse()
        data = response.read(4 * 1024 * 1024 + 1)
        if response.status != 200 or len(data) > 4 * 1024 * 1024:
            raise RuntimeError("packaged HTTP response is invalid")
        return data
    finally:
        connection.close()


def wait_http(process):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("packaged API exited before readiness")
        try:
            if b"_anonymous" in request("/api/session"):
                return
        except (OSError, http.client.HTTPException):
            pass
        time.sleep(0.05)
    raise RuntimeError("packaged API readiness timed out")


def stop(process, forced=False):
    if process.poll() is None:
        process.send_signal(signal.SIGKILL if forced else signal.SIGTERM)
    code = process.wait(timeout=30)
    if not forced and code != 0:
        raise RuntimeError("graceful process stop failed")


def smoke(application, temporary):
    if os.geteuid() == 0:
        raise RuntimeError("Linux process checks must not run as root")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 8080))
    outside = temporary / "outside"
    outside.mkdir()
    environment = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(temporary),
        "XDG_DATA_HOME": str(temporary / "state"),
        "TMPDIR": str(temporary),
        "LANG": "C.UTF-8",
    }
    executable = application / "domus"
    log = temporary / "process.log"
    processes = []

    def launch(arguments, notification=None):
        env = environment.copy()
        if notification:
            env["NOTIFY_SOCKET"] = str(notification)
        with log.open("ab") as output:
            process = subprocess.Popen(
                [str(executable), *arguments],
                cwd=outside,
                env=env,
                stdout=output,
                stderr=subprocess.STDOUT,
            )
            processes.append(process)
            return process

    def notified(process, receiver, expected=b"READY=1"):
        receiver.settimeout(30)
        if receiver.recv(1024) != expected:
            raise RuntimeError("unexpected systemd notification")
        if expected == b"READY=1" and process.poll() is not None:
            raise RuntimeError("service exited after notification")

    api = None
    worker = None
    try:
        for forced in (True, False):
            api = launch(["serve"])
            wait_http(api)
            page = request("/").decode()
            asset = re.search(r"/assets/[A-Za-z0-9._-]+\.js", page)
            if not asset or len(request(asset.group())) < 1000:
                raise RuntimeError("real packaged browser assets were not served")
            stop(api, forced)
            api = None
        state = temporary / "state" / "DomusObscura"
        configuration = state / "service.json"
        content = json.loads(configuration.read_text())
        content["production_worker"] = {"queue_capacity": 64, "acquisition": None}
        configuration.write_text(json.dumps(content))
        configuration.chmod(0o600)
        notify = temporary / "notify"
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver:
            receiver.bind(str(notify))
            api = launch(["systemd-service", "api", str(configuration)], notify)
            notified(api, receiver)
            wait_http(api)
            worker = launch(["systemd-service", "worker", str(configuration)], notify)
            notified(worker, receiver)
            # Current-schema API restart must not disturb the live worker.
            api.send_signal(signal.SIGTERM)
            notified(api, receiver, b"STOPPING=1")
            stop(api)
            api = launch(["systemd-service", "api", str(configuration)], notify)
            notified(api, receiver)
            wait_http(api)
            if worker.poll() is not None:
                raise RuntimeError("API restart stopped the independent worker")
            duplicate = launch(["worker", str(configuration)])
            if duplicate.wait(timeout=15) == 0:
                raise RuntimeError("concurrent worker ownership was accepted")
            stop(worker, forced=True)
            worker = launch(["systemd-service", "worker", str(configuration)], notify)
            notified(worker, receiver)
            worker.send_signal(signal.SIGTERM)
            notified(worker, receiver, b"STOPPING=1")
            stop(worker)
            worker = None
            api.send_signal(signal.SIGTERM)
            notified(api, receiver, b"STOPPING=1")
            stop(api)
            api = None
        with sqlite3.connect(state / "state.db") as database:
            if database.execute("PRAGMA integrity_check").fetchone() != ("ok",):
                raise RuntimeError("retained SQLite state is not readable")
        # Private configuration failure must not create an anonymous fallback.
        configuration.chmod(0o644)
        denied = launch(["systemd-service", "api", str(configuration)], notify)
        if denied.wait(timeout=15) == 0:
            raise RuntimeError("insecure configuration permissions were accepted")
        configuration.chmod(0o600)
        configuration.chmod(0o666)
        denied = launch(["serve", str(configuration)])
        if denied.wait(timeout=15) == 0:
            raise RuntimeError("untrusted configuration writes were accepted")
        configuration.chmod(0o600)
        content["database"] = str(temporary / "unavailable" / "state.db")
        configuration.write_text(json.dumps(content))
        missing = launch(["worker", str(configuration)])
        if missing.wait(timeout=15) == 0:
            raise RuntimeError("unavailable state was silently replaced")
        if (temporary / "unavailable").exists():
            raise RuntimeError("unavailable state created replacement files")
    except BaseException:
        print(log.read_text(errors="replace"))
        raise
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.kill()
                process.wait(timeout=30)


class ArchiveTests(unittest.TestCase):
    def fixture(self, root):
        names = (
            "domus",
            "browser/index.html",
            "notices/components.json",
            "systemd/domus-obscura-api.service",
            "systemd/domus-obscura-worker.service",
            "linux-services.md",
            "package-check.py",
        )
        files = {}
        modes = {}
        for name in names:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture")
            mode = 0o755 if name == "domus" else 0o644
            path.chmod(mode)
            files[name] = hashlib.sha256(b"fixture").hexdigest()
            modes[name] = mode
            for parent in path.parents:
                if parent == root:
                    break
                parent.chmod(0o755)
        (root / "package-manifest.json").write_text(
            json.dumps({"format": 1, "target": TARGET, "files": files, "modes": modes})
        )
        (root / "package-manifest.json").chmod(0o644)

    def test_rejects_unsafe_names(self):
        for name in ("../escape", "/absolute", "a//b", "a/./b", "a\\b", "a:b", "a\nb"):
            with self.assertRaises(ValueError):
                name_checked(name)

    def test_ordinary_ustar_payload_round_trips_with_exact_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = root / "payload"
            payload.mkdir()
            payload.chmod(0o755)
            self.fixture(payload)
            archive = root / "fixture.tar.gz"
            with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as output:
                output.add(payload, arcname=".", recursive=False)
                for path in sorted(payload.rglob("*")):
                    output.add(path, arcname=str(path.relative_to(payload)), recursive=False)
            destination = root / "extracted"
            destination.mkdir()
            unpack(archive, destination)
            installed(destination)
            damaged = root / "damaged.tar.gz"
            damaged.write_bytes(archive.read_bytes()[:-4])
            rejected = root / "rejected"
            rejected.mkdir()
            with self.assertRaises((EOFError, OSError, ValueError)):
                unpack(damaged, rejected)

    def test_archive_rejects_links_modes_duplicates_and_limits(self):
        for kind in ("link", "mode", "duplicate", "size", "extended-header"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive = root / "fixture.tar.gz"
                with tarfile.open(archive, "w:gz") as output:
                    entry = tarfile.TarInfo("file")
                    entry.mode = 0o644
                    if kind == "link":
                        entry.type = tarfile.SYMTYPE
                        entry.linkname = "../escape"
                    elif kind == "mode":
                        entry.mode = 0o777
                    elif kind == "size":
                        entry.size = MAX_FILE + 1
                    elif kind == "extended-header":
                        entry.type = tarfile.XHDTYPE
                        entry.size = MAX_FILE + 1
                    output.addfile(entry)
                    if kind == "duplicate":
                        output.addfile(entry)
                destination = root / "out"
                destination.mkdir()
                with self.assertRaises((ValueError, tarfile.TarError, OSError)):
                    unpack(archive, destination)

    def test_archive_rejects_unsafe_root_and_directory_headers(self):
        for kind in ("root-mode", "root-size", "root-duplicate", "directory-size"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive = root / "fixture.tar.gz"
                with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as output:
                    entry = tarfile.TarInfo("child" if kind == "directory-size" else ".")
                    entry.type = tarfile.DIRTYPE
                    entry.mode = 0o777 if kind == "root-mode" else 0o755
                    entry.size = 1 if kind in ("root-size", "directory-size") else 0
                    output.addfile(entry, io.BytesIO(b"x") if entry.size else None)
                    if kind == "root-duplicate":
                        entry.name = "./"
                        output.addfile(entry)
                destination = root / "out"
                destination.mkdir()
                with self.assertRaises(ValueError):
                    unpack(archive, destination)

    def test_extended_header_is_rejected_before_body_read(self):
        entry = tarfile.TarInfo("metadata")
        entry.type = tarfile.XHDTYPE
        entry.size = MAX_FILE + 1
        with self.assertRaisesRegex(ValueError, "only ordinary ustar"):
            with tarfile.open(
                fileobj=io.BytesIO(entry.tobuf(tarfile.USTAR_FORMAT)),
                mode="r|",
                tarinfo=PackageTarInfo,
            ):
                pass

    def test_archive_entry_limit_includes_root_header(self):
        for count in (MAX_FILES, MAX_FILES + 1):
            with self.subTest(count=count), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive = root / "fixture.tar.gz"
                with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as output:
                    entry = tarfile.TarInfo(".")
                    entry.type = tarfile.DIRTYPE
                    entry.mode = 0o755
                    output.addfile(entry)
                    for index in range(count - 1):
                        entry = tarfile.TarInfo(f"file-{index}")
                        entry.mode = 0o644
                        output.addfile(entry)
                destination = root / "out"
                destination.mkdir()
                if count == MAX_FILES:
                    unpack(archive, destination)
                    self.assertEqual(len(list(destination.iterdir())), MAX_FILES - 1)
                else:
                    with self.assertRaisesRegex(ValueError, "excessive archive entries"):
                        unpack(archive, destination)

    def test_installed_check_preserves_changed_extra_missing_and_linked_files(self):
        for change in (
            "changed",
            "extra",
            "missing",
            "link",
            "hardlink",
            "fifo",
            "mode",
            "empty-directory",
        ):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.fixture(root)
                installed(root)
                if change == "changed":
                    (root / "domus").write_bytes(b"changed")
                elif change == "extra":
                    (root / "unrelated").write_bytes(b"keep")
                elif change == "missing":
                    (root / "domus").unlink()
                elif change == "link":
                    (root / "domus").unlink()
                    (root / "domus").symlink_to("/bin/true")
                elif change == "hardlink":
                    os.link(root / "domus", root / "alias")
                elif change == "fifo":
                    os.mkfifo(root / "unrelated")
                elif change == "mode":
                    (root / "domus").chmod(0o777)
                else:
                    (root / "unrelated").mkdir()
                with self.assertRaises((ValueError, OSError)):
                    installed(root, remove=True)
                self.assertTrue((root / "browser/index.html").exists())
                self.assertTrue((root / "package-manifest.json").exists())

    def test_installed_removal_deletes_only_verified_payload(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            installed(root, remove=True)
            self.assertTrue(root.is_dir())
            self.assertEqual(list(root.iterdir()), [])

    def test_installed_root_rejects_linked_ancestors(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = root / "payload"
            payload.mkdir()
            self.fixture(payload)
            (root / "alias").symlink_to(payload, target_is_directory=True)
            with self.assertRaises(OSError):
                installed(root / "alias")
            self.assertTrue((payload / "domus").exists())

    def test_systemd_unit_syntax_in_an_isolated_root(self):
        verify_units(Path(__file__).parent / "systemd", Path("/bin/true"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, nargs="?")
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--self-test", action="store_true")
    actions.add_argument("--verify-only", action="store_true")
    actions.add_argument("--verify-installed", type=Path)
    actions.add_argument("--remove-installed", type=Path)
    args = parser.parse_args()
    if args.archive and (args.self_test or args.verify_installed or args.remove_installed):
        parser.error("an archive cannot be combined with another payload action")
    if args.self_test:
        result = unittest.main(argv=["package-check"], exit=False).result
        if not result.wasSuccessful():
            raise SystemExit(1)
        return
    if args.verify_installed or args.remove_installed:
        installed(args.verify_installed or args.remove_installed, bool(args.remove_installed))
        print("Exact installed payload checked." if not args.remove_installed else "Owned payload files removed; the root and unrelated data were not recursively deleted.")
        return
    if not args.archive:
        parser.error("an archive or installed-payload action is required")
    with tempfile.TemporaryDirectory(prefix="domus-linux-package-") as directory:
        temporary = Path(directory)
        application = temporary / "application"
        application.mkdir()
        unpack(args.archive, application)
        installed(application)
        if not args.verify_only:
            verify_units(application / "systemd", application / "domus")
            smoke(application, temporary)
    print("Linux archive checks passed. These checks do not install services or prove boot or kernel restrictions.")


if __name__ == "__main__":
    main()
