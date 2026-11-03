#!/usr/bin/env python3
"""Opt-in checks for manually installed, isolated Linux test services.

Never installs software/accounts/units, changes permissions, mounts, or reboots.
Only explicit run/cleanup commands invoke sudo, always non-interactively.
"""

import argparse
import contextlib
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import uuid

LIMIT = 2 * 1024 * 1024
MANUAL = (
    "physical_cold_boot",
    "physical_reboot",
    "real_drive_failure",
    "changed_identity_remount",
    "manager_stop_timeout",
    "restart_rate_limit",
    "credential_cache_staging_access",
    "update_uninstall_preservation",
    "stale_claim_and_failed_waiting_work",
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def checked_path(value):
    path = Path(value)
    require(path.is_absolute(), "an absolute path is required")
    require(not any(ord(char) < 32 or ord(char) == 127 for char in str(path)),
            "unsafe path characters")
    require(".." not in path.parts, "parent traversal is not permitted")
    for component in (*reversed(path.parents), path):
        require(not component.is_symlink(), "linked path component is not permitted")
    return path


def read(path, maximum=LIMIT):
    checked_path(str(path))
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(path, flags), "rb") as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), "not a regular file")
        value = stream.read(maximum + 1)
    require(len(value) <= maximum, "file exceeds the read limit")
    return value


def digest(path):
    checked_path(str(path))
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), "not a regular file")
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_new(path, value):
    checked_path(str(path))
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def command(arguments, timeout=30, success=True):
    # Do not keep arbitrary subprocess output in memory or copy it to evidence.
    with tempfile.TemporaryFile() as output:
        result = subprocess.run(
            arguments,
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
            env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"},
        )
        output.seek(0)
        text = output.read(LIMIT + 1)
    require(len(text) <= LIMIT, "command output exceeds the limit")
    if success:
        require(result.returncode == 0, f"{arguments[0]} failed ({result.returncode})")
    return result.returncode, text.decode("utf-8", errors="replace")


def manager(arguments, change=False, timeout=30, success=True):
    prefix = ["sudo", "-n"] if change else []
    return command(
        [*prefix, "systemctl", "--no-pager", *arguments], timeout, success
    )


def properties(unit):
    _, text = manager(
        [
            "show",
            unit,
            "--property=LoadState,ActiveState,SubState,MainPID,User,Group,"
            "FragmentPath,DropInPaths,NeedDaemonReload,NRestarts,Result,ExecMainStatus,"
            "ActiveEnterTimestampMonotonic,ExecMainStartTimestampMonotonic,"
            "NoNewPrivileges,CapabilityBoundingSet,ProtectSystem,ProtectHome",
        ]
    )
    return dict(line.split("=", 1) for line in text.splitlines() if "=" in line)


def wait_for(predicate, seconds, description):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise TimeoutError(description)


def markers():
    system = read(Path("/proc/sys/kernel/osrelease"), 4096).decode().strip()
    process = read(Path("/proc/1/stat"), 8192).decode()
    return {
        "host": socket.gethostname(),
        "kernel": system,
        "wsl": "microsoft" in system.lower(),
        "boot_id": read(Path("/proc/sys/kernel/random/boot_id"), 128).decode().strip(),
        "manager_start": process.rsplit(")", 1)[1].split()[19],
    }


def unit_name(run_id, role):
    require(re.fullmatch(r"[a-z0-9][a-z0-9-]{0,31}", run_id), "invalid run identifier")
    require(role in ("api", "worker"), "invalid service role")
    return f"domus-acceptance-{run_id}-{role}.service"


def render(template, root, package, account, group, run_id):
    replacements = {
        "User=domus-obscura": f"User={account}",
        "Group=domus-obscura": f"Group={group}",
        "/var/lib/domus-obscura/config/service.json": str(root / "service.json"),
        "/var/lib/domus-obscura": str(root),
        "/opt/domus-obscura": str(package),
        "domus-obscura-api.service": unit_name(run_id, "api"),
        "domus-obscura-worker.service": unit_name(run_id, "worker"),
    }
    for old, new in replacements.items():
        template = template.replace(old, new)
    return template


def fixture_configuration(root, fixture, package):
    acquisition = {
        "staging_root": fixture["staging"],
        "credentials_file": str(Path(fixture["database"]).parent / "generated-credentials.json"),
        "server": "1.1.1.1:2242",
        "acknowledge_plaintext_login": True,
    }
    expected = {
        "environment": "single_user",
        "system_settings_read_only": True,
        "services": {
            "discovery": [], "metadata": [],
            "artwork": [], "acquisition": ["soulseek"],
            "artist_images": [],
        },
        "database": fixture["database"], "listen": "127.0.0.1:8080",
        "origin": "http://127.0.0.1:8080",
        "browser_assets": str(package / "browser"),
        "production_worker": {"queue_capacity": 64, "acquisition": acquisition, "sharing": None},
    }
    require(json.loads(read(root / "service.json")) == expected,
            "configuration is not the generated offline fixture")
    require(json.loads(read(Path(acquisition["credentials_file"]))) == {
        "username": "generated-service-fixture", "password": "not-a-live-credential",
    }, "fixture must not contain live credentials")


def environment(root, run_id, package):
    require(sys.platform == "linux", "run this tool on native Linux")
    require(os.geteuid() != 0, "run the checker as the non-root runtime account")
    require(os.uname().machine == "x86_64", "use the supported x86_64 baseline")
    import grp
    import pwd

    account = pwd.getpwuid(os.geteuid())
    group = grp.getgrgid(account.pw_gid).gr_name
    for name in (account.pw_name, group):
        require(re.fullmatch(r"[a-zA-Z_][a-zA-Z0-9_-]*", name), "unsafe account name")
    root = checked_path(root)
    package = checked_path(package)
    require(re.fullmatch(r"/[A-Za-z0-9_./-]+", str(package)),
            "unsafe program path characters")
    require(
        root == Path("/var/lib") / f"domus-acceptance-{run_id}",
        "use the dedicated /var/lib/domus-acceptance-<run-id> directory",
    )
    unit_name(run_id, "api")
    require(root.stat().st_uid == account.pw_uid, "fixture root has a different owner")
    require(stat.S_IMODE(root.stat().st_mode) == 0o700, "fixture root must be private")
    require(not package.is_relative_to(root), "keep the program outside writable state")
    require(
        not any(package.is_relative_to(Path(p)) for p in ("/home", "/root", "/tmp", "/run")),
        "program location conflicts with service isolation",
    )
    require(
        read(Path("/proc/1/comm"), 64).strip() == b"systemd",
        "systemd must be the system manager",
    )
    os_release = read(Path("/usr/lib/os-release"), 8192).decode()
    require('VERSION_ID="24.04"' in os_release and "ID=ubuntu\n" in os_release,
            "use the supported Ubuntu 24.04 baseline")
    require(command(["getconf", "GNU_LIBC_VERSION"])[1].strip() == "glibc 2.39",
            "use the supported glibc baseline")
    require(command(["systemd", "--version"])[1].split()[:2] == ["systemd", "255"],
            "use the supported systemd baseline")
    command(
        ["python3", str(package / "package-check.py"), "--verify-installed", str(package)]
    )
    manifest = json.loads(read(package / "package-manifest.json"))
    fixture = json.loads(read(root / "fixture.json"))
    require(isinstance(fixture, dict), "fixture descriptor must be an object")
    require(fixture["format"] == 1 and fixture["offline_only"] is True, "invalid fixture")
    for key in ("job", "version", "scan_location", "scan_request", "publication_request",
                "normalization_run", "scan_files"):
        require(type(fixture[key]) is int and 0 < fixture[key] < 2 ** 63,
                "invalid fixture identifier or count")
    for key in ("database", "scan_root", "library", "staging"):
        require(isinstance(fixture[key], str), "fixture path must be text")
        require(
            checked_path(fixture[key]).is_relative_to(root),
            "fixture path escapes the isolated state",
        )
    fixture_configuration(root, fixture, package)
    return root, package, account, group, manifest, fixture


class Suite:
    def __init__(self, root, run_id, package):
        self.root, self.package, account, group, manifest, self.fixture = environment(
            root, run_id, package
        )
        self.run_id = run_id
        self.uid = account.pw_uid
        self.units = {role: unit_name(run_id, role) for role in ("api", "worker")}
        self.definitions = {
            role: render(
                read(self.package / "systemd" / f"domus-obscura-{role}.service").decode(),
                self.root, self.package, account.pw_name, group, run_id,
            )
            for role in self.units
        }
        self.report = {
            "format": 1,
            "run_id": run_id,
            "host": markers(),
            "uid": self.uid,
            "package_manifest_sha256": digest(self.package / "package-manifest.json"),
            "revision": manifest["revision"],
            "dirty": manifest["dirty"],
            "baseline": {"distribution": "Ubuntu 24.04", "glibc": "2.39", "systemd": "255"},
            "units": self.definitions,
            "checks": {name: "not_run" for name in MANUAL},
            "complete_issue_acceptance": False,
        }

    def installed(self):
        for role, unit in self.units.items():
            path = Path("/etc/systemd/system") / unit
            require(read(path).decode() == self.definitions[role], "installed unit changed")
            require(path.stat().st_uid == 0, "installed unit is not administrator-owned")
            value = properties(unit)
            require(value["LoadState"] == "loaded", "test unit is not loaded")
            require(value["FragmentPath"] == str(path), "unexpected unit source")
            require(not value.get("DropInPaths"), "unreviewed unit drop-ins")
            require(value["NeedDaemonReload"] == "no", "reload the reviewed test units first")
        for name in ("package-manifest.json", *json.loads(
                read(self.package / "package-manifest.json"))["files"]):
            path = self.package / name
            checked_path(str(path))
            info = path.stat()
            require(info.st_uid == 0 and not info.st_mode & 0o022,
                    "program payload must be administrator-owned and not runtime-writable")
            for directory in path.parents:
                info = directory.stat()
                require(info.st_uid == 0 and not info.st_mode & 0o022,
                        "program parent must be administrator-owned and not runtime-writable")

    def approve(self, host):
        require(host == socket.gethostname(), "explicit approval must name this exact host")
        self.installed()

    @contextlib.contextmanager
    def database(self):
        path = checked_path(self.fixture["database"])
        with contextlib.closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=1)) as db:
            db.row_factory = sqlite3.Row
            yield db

    def snapshot(self):
        info = Path(self.fixture["database"]).stat()
        with self.database() as db:
            db.execute("BEGIN")
            requests = [dict(row) for row in db.execute(
                "SELECT id,state,generation,lease_until,body,configuration_binding,unixepoch() AS observed_at "
                "FROM worker_requests ORDER BY id LIMIT 65"
            )]
            scan = db.execute(
                "SELECT id,status,observed_count,lease_until FROM scans WHERE location_id=? "
                "ORDER BY id DESC LIMIT 1", (self.fixture["scan_location"],)
            ).fetchone()
            job = db.execute(
                "SELECT state,claim_generation,lease_until FROM jobs WHERE id=?",
                (self.fixture["job"],),
            ).fetchone()
            require(job is not None, "generated job is missing")
            publication = db.execute(
                "SELECT count(*),sum(verified_at IS NOT NULL) FROM publication_files"
            ).fetchone()
            immutable = hashlib.sha256()
            for query, parameters, expected in (
                ("SELECT job_id,version,body,approved_at FROM job_plans "
                 "WHERE job_id=? AND version=? LIMIT 2",
                 (self.fixture["job"], self.fixture["version"]), 1),
                ("SELECT * FROM artwork_approvals WHERE job_id=? AND plan_version=? LIMIT 2",
                 (self.fixture["job"], self.fixture["version"]), 1),
                ("SELECT * FROM normalization_receipts WHERE run_id=? ORDER BY track_number LIMIT 4",
                 (self.fixture["normalization_run"],), 3),
            ):
                rows = db.execute(query, parameters).fetchall()
                require(len(rows) == expected, "generated immutable evidence is incomplete")
                for row in rows:
                    immutable.update(json.dumps(tuple(row), default=bytes.hex).encode())
            return {
                "requests": requests,
                "now": requests[0]["observed_at"] if requests else None,
                "job": dict(job),
                "scan": dict(scan) if scan else None,
                "publication": tuple(publication),
                "immutable_sha256": immutable.hexdigest(),
                "database_identity": [info.st_dev, info.st_ino],
            }

    def verify_complete(self, before):
        after = self.snapshot()
        require(after["immutable_sha256"] == before["immutable_sha256"], "approval or receipt changed")
        require(after["database_identity"] == before["database_identity"], "database was replaced")
        require(len(after["requests"]) == 2, "unexpected durable request set")
        require(all(row["state"] == "complete" for row in after["requests"]), "unfinished request")
        with self.database() as db:
            require(db.execute("PRAGMA integrity_check").fetchone()[0] == "ok", "SQLite integrity failed")
            require(db.execute("SELECT state FROM jobs WHERE id=?", (self.fixture["job"],)).fetchone()[0]
                    == "complete", "publication is not complete")
            scan = db.execute("SELECT coverage FROM locations WHERE id=?",
                              (self.fixture["scan_location"],)).fetchone()
            require(scan[0] == "complete", "scan coverage is incomplete")
            count = db.execute("SELECT count(*) FROM entries WHERE location_id=? AND kind='file'",
                               (self.fixture["scan_location"],)).fetchone()[0]
            require(count == self.fixture["scan_files"], "scan did not observe every generated file")
            rows = db.execute(
                "SELECT d.root,d.relative_folder,d.verified_at,f.name,f.bytes,f.sha256,"
                "f.verified_at FROM publication_destinations d JOIN publication_files f "
                "ON f.destination_id=d.id WHERE d.job_id=? AND d.plan_version=?",
                (self.fixture["job"], self.fixture["version"]),
            ).fetchall()
        require(len(rows) == 4, "missing publication files")
        for row in rows:
            path = checked_path(str(Path(row[0]) / row[1] / row[3]))
            require(path.is_relative_to(Path(self.fixture["library"])), "output escaped fixture")
            require(row[2] is not None and row[6] is not None, "missing publication receipt")
            require(path.stat().st_size == row[4] and digest(path) == row[5], "output hash differs")
        return after

    def http_ready(self):
        with contextlib.closing(http.client.HTTPConnection("127.0.0.1", 8080, timeout=2)) as connection:
            connection.request("GET", "/")
            response = connection.getresponse()
            page = response.read(LIMIT + 1)
            require(response.status == 200 and len(page) <= LIMIT, "browser response failed")
            asset = re.search(rb"/assets/[A-Za-z0-9._-]+\.js", page)
            require(asset is not None, "real browser asset is absent")
            connection.request("GET", asset.group().decode())
            response = connection.getresponse()
            body = response.read(4 * LIMIT + 1)
            require(response.status == 200 and 1000 < len(body) <= 4 * LIMIT, "browser asset failed")

    def runtime_restrictions(self):
        result = {}
        for role, unit in self.units.items():
            value = properties(unit)
            require(value["ActiveState"] == "active", "service is not ready")
            pid = int(value["MainPID"])
            require(pid > 1, "invalid service PID")
            status = read(Path(f"/proc/{pid}/status"), 16384).decode()
            fields = dict(line.split(":", 1) for line in status.splitlines() if ":" in line)
            require(int(fields["Uid"].split()[1]) == self.uid, "runtime UID differs")
            require(all(int(fields[key].strip(), 16) == 0 for key in (
                "CapEff", "CapPrm", "CapInh", "CapBnd", "CapAmb"
            )), "runtime has capabilities")
            require(fields["NoNewPrivs"].strip() == "1", "no-new-privileges is inactive")
            require(os.readlink(f"/proc/{pid}/ns/mnt") != os.readlink("/proc/self/ns/mnt"),
                    "service has no separate mount namespace")
            # Inspect the actual process mount view, not only configured directives.
            mounts = read(Path(f"/proc/{pid}/mountinfo"), LIMIT).decode().splitlines()
            root_mount = [line.split() for line in mounts if line.split()[4] == "/"]
            require(root_mount and "ro" in root_mount[-1][5].split(","),
                    "service root filesystem is not read-only")
            state_mount = [line.split() for line in mounts if line.split()[4] == str(self.root)]
            require(state_mount and "rw" in state_mount[-1][5].split(","),
                    "fixture state is not writable in the service namespace")
            require(os.readlink(f"/proc/{pid}/exe") == str(self.package / "domus"),
                    "running executable differs from the verified program")
            result[role] = value
        self.report["runtime"] = result
        self.report["checks"]["runtime_identity_and_restrictions"] = "passed"

    def start(self):
        # Refuse to interrupt a previous run or consume unreviewed existing work.
        requests = self.snapshot()["requests"]
        require([row["id"] for row in requests] == [
            self.fixture["scan_request"], self.fixture["publication_request"]
        ] and all(row["state"] == "queued" for row in requests),
                "use fresh queued fixtures for each service test")
        expected = [
            {"operation": "scan", "location_id": self.fixture["scan_location"]},
            {"operation": "publish", "job_id": self.fixture["job"],
             "plan_version": self.fixture["version"],
             "normalization_run": self.fixture["normalization_run"],
             "acknowledge_new_only_publication": True},
        ]
        require([json.loads(row["body"]) for row in requests] == expected
                and all(row["configuration_binding"] == self.fixture["configuration_binding"]
                        for row in requests), "only generated scan and local publication are allowed")
        require(all(properties(unit)["ActiveState"] in ("inactive", "failed")
                    for unit in self.units.values()), "test services are already active")
        # Starting only the worker tests the real Wants/After readiness dependency.
        manager(["start", self.units["worker"]], change=True, timeout=260)
        self.http_ready()
        self.runtime_restrictions()
        api = self.report["runtime"]["api"]
        worker = self.report["runtime"]["worker"]
        require(int(worker["ExecMainStartTimestampMonotonic"])
                >= int(api["ActiveEnterTimestampMonotonic"]), "worker started before API readiness")
        self.report["checks"]["readiness_order_and_browser"] = "passed"

    def stop(self, graceful=True):
        for role in ("worker", "api"):
            manager(["stop", self.units[role]], change=True, timeout=200)
            value = properties(self.units[role])
            require(value["ActiveState"] in ("inactive", "failed"), "service did not stop")
            if graceful:
                require(value["Result"] == "success", "service did not drain successfully")

    def active(self):
        snapshot = self.snapshot()
        scan = snapshot["scan"]
        total, verified = snapshot["publication"]
        return (
            len(snapshot["requests"]) == 2
            and all(row["state"] == "running" for row in snapshot["requests"])
            and scan and scan["status"] == "running" and scan["observed_count"] > 0
            and verified is not None and 0 < verified < total
        )

    def recover(self, forced):
        before = self.snapshot()
        self.start()
        wait_for(self.active, 90, "no simultaneous active scan/publication window; result is inconclusive")
        worker = self.units["worker"]
        manager(["kill", "--kill-whom=main", "--signal=SIGSTOP", worker], change=True)
        frozen = True
        try:
            wait_for(lambda: "T" in read(Path(f"/proc/{int(properties(worker)['MainPID'])}/status"),
                                            16384).decode().split("State:", 1)[1].splitlines()[0],
                     5, "worker did not stop for observation")
            require(self.active(), "work finished before the signal; not active recovery evidence")
            held = self.snapshot()
            code, output = command([str(self.package / "domus"), "worker",
                                    str(self.root / "service.json")], timeout=15, success=False)
            require(code != 0 and "another process owns this worker or account" in output,
                    "native ownership exclusion was not confirmed")
            self.report["checks"]["active_work_and_native_ownership"] = "passed"
            if forced:
                manager(["kill", "--kill-whom=all", "--signal=SIGKILL", worker], change=True)
                manager(["stop", worker], change=True, timeout=40)
            else:
                manager(["stop", worker], change=True, timeout=200)
            frozen = False
            manager(["start", worker], change=True, timeout=130)
            self.report["interrupted"] = held
            def completed():
                current = self.snapshot()
                for old, new in zip(held["requests"], current["requests"], strict=True):
                    require(old["id"] == new["id"], "request identity changed")
                    if forced and old["lease_until"] and current["now"] < old["lease_until"]:
                        require(old["generation"] == new["generation"], "request reclaimed before lease expiry")
                if forced and held["job"]["lease_until"] and current["now"] < held["job"]["lease_until"]:
                    require(held["job"]["claim_generation"] == current["job"]["claim_generation"],
                            "job reclaimed before lease expiry")
                require(all(row["state"] not in ("failed", "waiting") for row in current["requests"]),
                        "recovery requires operator action; inspect retained state")
                return all(row["state"] == "complete" for row in current["requests"])
            wait_for(completed, 720, "lease-fenced recovery did not complete")
            self.report["after"] = self.verify_complete(before)
            self.report["checks"]["forced_active_recovery" if forced else "graceful_active_recovery"] = "passed"
        finally:
            if frozen:
                manager(["kill", "--kill-whom=main", "--signal=SIGCONT", worker], change=True)

    def lifecycle(self):
        before = self.snapshot()
        self.start()
        worker_pid = properties(self.units["worker"])["MainPID"]
        manager(["restart", self.units["api"]], change=True, timeout=200)
        require(properties(self.units["worker"])["MainPID"] == worker_pid, "API restart replaced worker")
        self.http_ready()
        self.report["checks"]["api_restart_preserves_worker"] = "passed"
        wait_for(lambda: all(row["state"] == "complete" for row in self.snapshot()["requests"]),
                 720, "generated work did not complete")
        self.report["after"] = self.verify_complete(before)
        self.stop()
        self.report["checks"]["graceful_stop"] = "passed"

    def save(self, case):
        path = self.root / f"report-{case}-{uuid.uuid4().hex}.json"
        write_new(path, self.report)
        return path

    def cleanup(self):
        # Keep all generated data and reports. Remove only unchanged owned units.
        self.stop(graceful=False)
        manager(["disable", *self.units.values()], change=True)
        self.installed()
        for unit in self.units.values():
            path = Path("/etc/systemd/system") / unit
            command(["sudo", "-n", "unlink", str(path)])
            require(not path.exists(), "unit file removal is incomplete")
        manager(["daemon-reload"], change=True)
        self.report["checks"]["test_unit_cleanup"] = "passed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("preflight", "prepare", "run", "collect", "cleanup", "boot-before", "boot-after"))
    parser.add_argument("--root", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--approved-host")
    parser.add_argument("--case", choices=("lifecycle", "forced", "graceful"), default="lifecycle")
    parser.add_argument("--before", help="previous boot-before report")
    args = parser.parse_args()
    suite = Suite(args.root, args.run_id, args.package)
    suite.report["mode"] = args.mode
    suite.report["case"] = args.case if args.mode == "run" else None
    if args.mode == "preflight":
        print(json.dumps(suite.report, indent=2))
        return
    if args.mode == "prepare":
        # Only render files. An operator must separately approve and install them.
        directory = suite.root / "units"
        directory.mkdir(mode=0o700)
        for role, name in suite.units.items():
            path = directory / name
            with path.open("x", encoding="utf-8", newline="\n") as stream:
                stream.write(suite.definitions[role])
        print("Rendered isolated units. No services were installed or started.")
        return
    if args.mode in ("run", "cleanup"):
        suite.approve(args.approved_host)
    if args.mode in ("run", "cleanup"):
        try:
            if args.mode == "cleanup":
                suite.cleanup()
            elif args.case == "lifecycle":
                suite.lifecycle()
            else:
                suite.recover(forced=args.case == "forced")
                suite.stop()
        except (OSError, ValueError, KeyError, TimeoutError, subprocess.SubprocessError, sqlite3.Error):
            case = args.case if args.mode == "run" else "cleanup"
            suite.report["checks"][case] = "failed"
            suite.save(case)
            raise
    elif args.mode in ("boot-before", "boot-after"):
        suite.installed()
        suite.runtime_restrictions()
        suite.http_ready()
        suite.report["snapshot"] = suite.snapshot()
        if args.mode == "boot-after":
            require(args.before is not None, "--before is required")
            before = json.loads(read(checked_path(args.before)))
            require(before["run_id"] == suite.run_id
                    and before["package_manifest_sha256"] == suite.report["package_manifest_sha256"],
                    "boot report belongs to different inputs")
            require(before["host"]["host"] == suite.report["host"]["host"], "boot host changed")
            require(before["host"]["manager_start"] != suite.report["host"]["manager_start"]
                    or before["host"]["boot_id"] != suite.report["host"]["boot_id"],
                    "no manager or kernel restart was observed")
            require(before["snapshot"]["database_identity"]
                    == suite.report["snapshot"]["database_identity"], "database was replaced")
            require(before["snapshot"]["immutable_sha256"]
                    == suite.report["snapshot"]["immutable_sha256"],
                    "approval or normalization evidence changed across restart")
            suite.report["checks"]["running_after_observed_restart"] = "passed"
            # This cannot prove a physical cold boot, nor exclude a manual start.
    else:
        suite.installed()
        suite.report["snapshot"] = suite.snapshot()
        suite.report["runtime"] = {role: properties(unit) for role, unit in suite.units.items()}
    print(suite.save(args.mode))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TimeoutError, subprocess.SubprocessError, sqlite3.Error) as error:
        print(f"Acceptance check failed: {error}", file=sys.stderr)
        sys.exit(1)
