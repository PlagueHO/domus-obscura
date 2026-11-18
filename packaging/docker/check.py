"""Bounded Docker checks using only fresh state and generated media."""

import argparse
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

ROOT = Path(__file__).resolve().parents[2]
DATABASE = "/data/DomusObscura/state.db"
CONFIGURATION = "/data/DomusObscura/service.json"
REQUIRED_CHECKS = {
    "browser",
    "api-guards",
    "read-only-media",
    "worker-scan",
    "state-recreation",
    "native-bindings",
    "graceful-stop",
    "forced-exit",
    "invalid-config",
    "mount-replacement",
    "root-overlap",
    "missing-source",
}


def command(arguments, *, input=None, check=True, timeout=60, env=None, text=True):
    result = subprocess.run(
        arguments, input=input, capture_output=True, text=text, timeout=timeout, cwd=ROOT,
        env=env,
    )
    if len(result.stdout) + len(result.stderr) > 4 * 1024 * 1024:
        raise RuntimeError("test command output exceeds 4 MiB")
    if check and result.returncode:
        raise RuntimeError(
            f"{arguments[0]} failed ({result.returncode}): {result.stderr[-8192:]}"
        )
    return result


def docker(*arguments, **options):
    return command(["docker", *arguments], **options)


def build(image):
    revision = command(["git", "rev-parse", "HEAD"]).stdout.strip()
    dirty = bool(command(["git", "status", "--porcelain"]).stdout.strip())
    subprocess.run(
        [
            "docker", "build", "--platform", "linux/amd64", "--tag", image,
            "--build-arg", f"DOMUS_SOURCE_REVISION={revision}",
            "--build-arg", f"DOMUS_SOURCE_DIRTY={str(dirty).lower()}", ".",
        ],
        check=True, cwd=ROOT, timeout=3600,
    )


def validate_evidence(linux, windows):
    for report, host in ((linux, "Linux"), (windows, "Windows")):
        if not isinstance(report, dict) or not isinstance(report.get("checks"), list):
            raise ValueError(f"structured {host} host evidence is required")
        if not all(isinstance(check, str) for check in report["checks"]):
            raise ValueError(f"invalid {host} check names")
        if (
            report.get("format") != 1
            or report.get("result") != "passed"
            or report.get("scope") != "host"
            or report.get("host") != host
            or report.get("engine_os") != "linux"
            or report.get("image_os") != "linux"
            or report.get("image_architecture") != "amd64"
            or report.get("source_dirty") is not False
            or not REQUIRED_CHECKS.issubset(report.get("checks", []))
        ):
            raise ValueError(f"complete clean-source {host} host evidence is required")
    if (
        not isinstance(linux.get("image_id"), str)
        or not isinstance(linux.get("revision"), str)
        or not re.fullmatch(r"sha256:[0-9a-f]{64}", linux["image_id"])
        or not re.fullmatch(r"[0-9a-f]{40}", linux["revision"])
        or linux["image_id"] != windows.get("image_id")
        or linux["revision"] != windows.get("revision")
    ):
        raise ValueError("both hosts must check the exact same image and source revision")
    if "windows-bind-mount" not in windows["checks"]:
        raise ValueError("Windows bind-mount safety evidence is required")


class Check:
    def __init__(self, image, engine_only, volume_media=False):
        self.image = image
        self.engine_only = engine_only
        self.prefix = "domus-check-" + uuid.uuid4().hex
        self.containers = []
        self.volumes = []
        self.api = None
        self.media = None
        self.media_volume = None
        self.volume_media = volume_media
        self.mounts = []
        self.report = {
            "format": 1, "result": "failed",
            "host": platform.system(), "scope": "engine-only" if engine_only else "host",
            "checks": [],
        }

    def volume(self, suffix):
        name = self.prefix + "-" + suffix
        docker("volume", "create", "--label", "io.domus.test=" + self.prefix, name)
        self.volumes.append(name)
        return name

    def media_mount(self, target, writable=False):
        kind = "volume" if self.media_volume else "bind"
        source = self.media_volume or self.media
        return f"type={kind},source={source},target={target}" + ("" if writable else ",readonly")

    def arguments(self, writable_media=False, media_alias=False):
        arguments = [
            "--init", "--read-only", "--user", "1000:1000", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--network", "host",
            "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m",
        ]
        for volume, target in self.mounts:
            arguments += ["--mount", f"type=volume,source={volume},target={target}"]
        if self.media_volume or self.media:
            arguments += ["--mount", self.media_mount("/media", writable_media)]
        if media_alias:
            arguments += ["--mount", self.media_mount("/media-alias")]
        return arguments

    def once(self, arguments, *, input=None, shell=False, check=True, writable_media=False, media_alias=False):
        self.report["phase"] = "shell" if shell else arguments[0]
        name = self.prefix + "-once-" + uuid.uuid4().hex
        self.containers.append(name)
        result = docker(
            "run", "--rm", "--name", name, "--interactive", *self.arguments(writable_media, media_alias),
            *(["--entrypoint", "/bin/sh"] if shell else []),
            self.image, *arguments, input=input, check=check, timeout=150,
        )
        inspected = docker("inspect", "--type", "container", name, check=False)
        if inspected.returncode and "No such" in inspected.stderr:
            self.containers.remove(name)
        return result

    def start(self, role):
        self.report["phase"] = "start-" + role
        name = self.prefix + "-" + role + "-" + str(len(self.containers))
        self.containers.append(name)
        docker(
            "run", "--detach", "--name", name,
            "--label", "io.domus.test=" + self.prefix, *self.arguments(),
            *(["--no-healthcheck"] if role == "worker" else []),
            self.image,
            *(["serve"] if role == "api" else ["worker", CONFIGURATION]),
        )
        if role == "api":
            self.api = name
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                state = json.loads(docker("inspect", name).stdout)[0]["State"]
                if not state["Running"]:
                    raise RuntimeError("API exited before readiness: " + docker(
                        "logs", "--tail", "60", name
                    ).stdout)
                if state.get("Health", {}).get("Status") == "healthy":
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("API health readiness timed out")
            session = json.loads(self.request("/api/session"))
            if session.get("object_id") != "_anonymous":
                raise RuntimeError("the real anonymous API session was not served")
        return name

    def request(self, path, method="GET", body=None, headers=None, expected=200):
        headers = {"Host": "127.0.0.1:8080", **(headers or {})}
        data = None if body is None else json.dumps(body)
        if data is not None:
            headers["Content-Type"] = "application/json"
        if self.engine_only:
            arguments = [
                "exec", self.api, "curl", "--silent", "--show-error", "--max-time", "3",
                "--max-filesize", str(4 * 1024 * 1024),
                "--noproxy", "*", "--request", method, "--write-out", "\n%{http_code}",
            ]
            for name, value in headers.items():
                arguments += ["--header", f"{name}: {value}"]
            if data is not None:
                arguments += ["--data-binary", data]
            response = docker(*arguments, "http://127.0.0.1:8080" + path, text=False).stdout
            content, status = response.rsplit(b"\n", 1)
            status = int(status)
        else:
            connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=3)
            try:
                connection.request(method, path, body=data, headers=headers)
                response = connection.getresponse()
                status = response.status
                content = response.read(4 * 1024 * 1024 + 1)
            finally:
                connection.close()
        if status != expected or len(content) > 4 * 1024 * 1024:
            raise RuntimeError(f"{method} {path}: expected {expected}, received {status}")
        return content

    def stop(self, name, forced=False):
        if forced:
            docker("kill", "--signal", "KILL", name)
        else:
            docker("stop", "--time", "120", name, timeout=135)
        code = json.loads(docker("inspect", name).stdout)[0]["State"]["ExitCode"]
        if code != (137 if forced else 0):
            raise RuntimeError(f"unexpected container exit code: {code}")
        docker("rm", name)
        self.containers.remove(name)

    def wait_worker(self, revision):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            status = json.loads(self.request("/api/local/status"))["worker"]
            if status and status["online"] and status["active_revision"] == revision:
                return
            time.sleep(0.5)
        raise RuntimeError("matching worker heartbeat was not observed")

    def missing_sources(self, temporary):
        results = {}
        for compose in (False, True):
            source = temporary / ("missing-compose" if compose else "missing-direct")
            if compose:
                project = self.prefix + "-source"
                name = project + "-api"
                self.containers.append(name)
                try:
                    result = docker(
                        "compose", "--project-name", project,
                        "-f", "compose.yaml", "-f", "compose.media.yaml",
                        "run", "--rm", "--name", name, "--no-deps", "--pull", "never",
                        "--entrypoint", "/bin/sh", "api", "-c", "true", check=False,
                        env={**os.environ, "DOMUS_IMAGE": self.image, "DOMUS_MEDIA_ROOT": str(source)},
                        timeout=150,
                    )
                finally:
                    volumes = docker(
                        "volume", "ls", "--quiet",
                        "--filter", "label=com.docker.compose.project=" + project,
                    ).stdout.split()
                    if any(not volume.startswith(project + "_") for volume in volumes):
                        raise RuntimeError("unexpected test Compose volume ownership")
                    self.volumes.extend(volumes)
            else:
                original = self.media
                self.media = source
                try:
                    result = self.once(["version"], check=False)
                finally:
                    self.media = original
            results["compose" if compose else "direct"] = {
                "exit_code": result.returncode,
                "source_created": source.exists(),
                "error": result.stderr[-8192:],
            }
        self.report["missing_sources"] = results
        self.report["phase"] = "missing-source"
        if any(result["exit_code"] == 0 or result["source_created"] for result in results.values()):
            raise RuntimeError("a missing bind source was accepted or created")
        self.report["checks"].append("missing-source")

    def run(self, temporary):
        server = json.loads(docker("version", "--format", "{{json .Server}}").stdout)
        if server["Os"] != "linux" or server["Arch"] != "amd64":
            raise RuntimeError("Docker checks require a Linux x86_64 engine")
        image = json.loads(docker("image", "inspect", self.image).stdout)[0]
        if image["Os"] != "linux" or image["Architecture"] != "amd64":
            raise RuntimeError("Docker checks require a Linux x86_64 image")
        if image["Config"]["User"] != "1000:1000":
            raise RuntimeError("image runtime must use the documented non-root account")
        self.image = image["Id"]
        labels = image["Config"]["Labels"]
        self.report.update({
            "engine_os": server["Os"], "engine_version": server["Version"],
            "image_os": image["Os"], "image_architecture": image["Architecture"],
            "image_id": image["Id"], "revision": labels["org.opencontainers.image.revision"],
            "source_dirty": labels.get("io.domus.source.dirty") != "false",
        })
        self.mounts = [(self.volume(name), "/" + name) for name in ("data", "cache", "staging")]
        self.media = temporary / "media"
        album = self.media / "Artist" / "Album"
        album.mkdir(parents=True)
        audio = b"\xff\xfb\x90\x64" + bytes(413)
        (album / "01.mp3").write_bytes(audio * 3)
        (album / "02.mp3").write_bytes(audio * 3)
        originals = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in album.iterdir()}
        outside = temporary / "outside"
        outside.mkdir()
        (outside / "must-not-index.txt").write_text("outside generated fixture\n")
        link = self.media / "escape-link"
        if self.volume_media:
            self.media_volume = self.volume("media")
            self.once(
                ["-c", "set -eu; umask 077; mkdir -p /media/Artist/Album /data/outside; "
                 "base64 -d > /media/Artist/Album/01.mp3; "
                 "cp /media/Artist/Album/01.mp3 /media/Artist/Album/02.mp3; "
                 "printf 'outside generated fixture\\n' > /data/outside/must-not-index.txt; "
                 "ln -s /data/outside /media/escape-link"],
                shell=True, writable_media=True,
                input=base64.b64encode(audio * 3).decode(),
            )
        elif os.name == "nt":
            command([
                "powershell.exe", "-NoProfile", "-Command",
                "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:DOMUS_TEST_LINK -Target $env:DOMUS_TEST_TARGET | Out-Null",
            ], env={**os.environ, "DOMUS_TEST_LINK": str(link), "DOMUS_TEST_TARGET": str(outside)})
        else:
            link.symlink_to(outside, target_is_directory=True)

        if not self.volume_media:
            self.report["phase"] = "missing-source"
            self.missing_sources(temporary)
        api = self.start("api")
        page_data = self.request("/")
        page = page_data.decode()
        asset = re.search(r"/assets/[A-Za-z0-9._-]+\.js", page)
        manifest = json.loads(docker("exec", api, "cat", "/opt/domus-obscura/package-manifest.json").stdout)
        if (
            not asset
            or hashlib.sha256(page_data).hexdigest() != manifest["files"]["browser/index.html"]
            or hashlib.sha256(self.request(asset.group())).hexdigest() != manifest["files"]["browser" + asset.group()]
        ):
            raise RuntimeError("real connected browser assets are missing")
        self.report["checks"].append("browser")
        guards = {"Origin": "http://127.0.0.1:8080", "x-domus-csrf": "1"}
        self.request("/api/jobs", "POST", {}, expected=403)
        self.request("/api/jobs", "POST", {}, {"Origin": "http://invalid.example", "x-domus-csrf": "1"}, 403)
        self.request("/api/session", headers={"Host": "invalid.example"}, expected=421)
        job = json.loads(self.request("/api/jobs", "POST", {}, guards))["job_id"]
        self.report["checks"].append("api-guards")
        mounts = json.loads(docker("inspect", api).stdout)[0]["Mounts"]
        media_mounts = [mount for mount in mounts if mount["Destination"] == "/media"]
        if len(media_mounts) != 1 or media_mounts[0].get("RW") is not False:
            raise RuntimeError("the runtime media mount is not explicitly read-only")
        denied = docker("exec", api, "sh", "-c", "touch /media/not-permitted", check=False)
        if not denied.returncode or (self.media / "not-permitted").exists():
            raise RuntimeError("read-only media mount accepted a write")
        self.report["checks"].append("read-only-media")
        self.stop(api)

        configuration = json.loads(self.once(["-c", f"cat {CONFIGURATION}"], shell=True).stdout)
        configuration["system_settings_read_only"] = False
        configuration["production_worker"] = {"queue_capacity": 8, "acquisition": None}
        configuration["cache"] = {
            "root": "/cache",
            "limits": {"item_bytes": 1048576, "total_bytes": 16777216, "entries": 16},
        }
        content = json.dumps(configuration)
        self.once(["-c", f"umask 077; cat > {CONFIGURATION}"], shell=True, input=content)
        revision = hashlib.sha256(content.encode()).hexdigest()
        location = self.once(["location-add", DATABASE, "/media", "main"]).stdout.strip()
        self.once(["staging-init", DATABASE, "/staging"])
        self.once(["staging-init", DATABASE, "/staging"])
        overlap = self.once(["location-add", DATABASE, "/media-alias", "backup"], check=False, media_alias=True)
        if not overlap.returncode or "overlap" not in overlap.stderr:
            raise RuntimeError("an aliased media root was not rejected for native overlap")
        self.report["checks"].append("root-overlap")
        api = self.start("api")
        worker = self.start("worker")
        self.wait_worker(revision)
        duplicate = self.once(["worker", CONFIGURATION], check=False)
        if duplicate.returncode == 0:
            raise RuntimeError("a second worker acquired live state")
        self.report["phase"] = "worker-scan"
        self.request(f"/api/locations/{location}/scan", "POST", {}, guards, 202)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            requests = json.loads(self.request("/api/worker/requests"))
            if requests and requests[0]["state"] == "complete":
                break
            if requests and requests[0]["state"] in ("failed", "waiting"):
                self.report["scan_locations"] = json.loads(self.request("/api/locations"))
                raise RuntimeError("generated worker scan failed: " + json.dumps(requests))
            time.sleep(0.5)
        else:
            raise RuntimeError("generated worker scan timed out")
        entries = json.loads(self.request(f"/api/locations/{location}/entries?folder=Artist%2FAlbum"))
        if (
            {entry["relative_path"] for entry in entries} != {"Artist/Album/01.mp3", "Artist/Album/02.mp3"}
            or not all(entry["kind"] == "file" and entry["present"] and entry["size"] == len(audio) * 3 for entry in entries)
        ):
            raise RuntimeError("mounted album inventory differs from the generated files")
        root_entries = json.loads(self.request(f"/api/locations/{location}/entries"))
        unsafe = [entry for entry in root_entries if entry["relative_path"] == "escape-link"]
        if len(unsafe) != 1 or unsafe[0]["kind"] != "unsafe_link":
            raise RuntimeError("host link/reparse traversal was not rejected")
        if json.loads(self.request(f"/api/locations/{location}/entries?folder=escape-link")):
            raise RuntimeError("files outside the mounted library were indexed")
        self.report["checks"] += ["worker-scan", "native-bindings"]
        if os.name == "nt" and not self.volume_media:
            self.report["checks"].append("windows-bind-mount")
        if self.volume_media:
            self.report["checks"].append("linux-volume-media")
        self.stop(api)
        api = self.start("api")
        self.wait_worker(revision)
        if job not in [item["id"] for item in json.loads(self.request("/api/jobs"))]:
            raise RuntimeError("recreated API lost retained job state")
        self.report["checks"].append("state-recreation")
        self.stop(worker)
        self.stop(api)
        self.report["checks"].append("graceful-stop")
        api = self.start("api")
        worker = self.start("worker")
        self.wait_worker(revision)
        self.stop(worker, forced=True)
        self.stop(api, forced=True)
        api = self.start("api")
        worker = self.start("worker")
        self.wait_worker(revision)
        self.stop(worker)
        self.stop(api)
        self.report["checks"].append("forced-exit")
        self.once(["-c", f"chmod 0666 {CONFIGURATION}"], shell=True)
        invalid = self.once(["serve", CONFIGURATION], check=False)
        self.once(["-c", f"chmod 0600 {CONFIGURATION}"], shell=True)
        if not invalid.returncode:
            raise RuntimeError("insecure configuration selected a fallback")
        self.report["checks"].append("invalid-config")
        replacement = temporary / "replacement"
        replacement.mkdir()
        original_media, original_volume = self.media, self.media_volume
        self.media = replacement
        if self.volume_media:
            self.media_volume = self.volume("replacement-media")
        changed = self.once(["scan", DATABASE, location], check=False)
        if not changed.returncode:
            raise RuntimeError("a replacement media root was accepted")
        retained = json.loads(self.once(["browse", DATABASE, location, "Artist/Album", "0", "100"]).stdout)
        if {entry["relative_path"] for entry in retained} != {"Artist/Album/01.mp3", "Artist/Album/02.mp3"}:
            raise RuntimeError("failed mount access erased retained observations")
        self.media, self.media_volume = original_media, original_volume
        marker = self.once(["-c", "cat /cache/owner.lock"], shell=True).stdout
        replacement_cache = self.volume("replacement-cache")
        self.mounts = [(replacement_cache if target == "/cache" else volume, target) for volume, target in self.mounts]
        self.once(["-c", "umask 077; cat > /cache/owner.lock"], shell=True, input=marker)
        copied = self.once(["serve", CONFIGURATION], check=False)
        if copied.returncode == 0:
            raise RuntimeError("a copied cache owner marker replaced native binding")
        self.report["checks"].append("mount-replacement")
        if self.volume_media:
            hashes = self.once(["-c", "sha256sum /media/Artist/Album/*.mp3"], shell=True).stdout
            observed = {}
            for line in hashes.splitlines():
                digest, path = line.split(maxsplit=1)
                observed[path.rsplit("/", 1)[-1]] = digest
        else:
            observed = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in album.iterdir()}
        if originals != observed:
            raise RuntimeError("read-only workflow changed generated audio")
        self.report["result"] = "passed"
        self.report["phase"] = "complete"

    def cleanup(self):
        errors = []
        for name in reversed(self.containers):
            inspected = docker("inspect", "--type", "container", name, check=False)
            if inspected.returncode and "No such" in inspected.stderr:
                continue
            if inspected.returncode:
                errors.append(inspected.stderr)
                continue
            log = docker("logs", "--tail", "60", name, check=False)
            self.report.setdefault("logs", {})[name] = (log.stdout + log.stderr)[-16384:]
            result = docker("rm", "--force", name, check=False, timeout=135)
            if result.returncode:
                errors.append(result.stderr)
        for name in reversed(self.volumes):
            result = docker("volume", "rm", name, check=False)
            if result.returncode:
                errors.append(result.stderr)
        if errors:
            self.report["result"] = "failed"
            self.report["cleanup_error"] = "\n".join(errors)
            raise RuntimeError("test resource cleanup failed: " + self.report["cleanup_error"])


class EvidenceTests(unittest.TestCase):
    def reports(self):
        common = {
            "format": 1, "result": "passed", "scope": "host", "engine_os": "linux",
            "image_os": "linux", "image_architecture": "amd64",
            "source_dirty": False, "image_id": "sha256:" + "a" * 64,
            "revision": "b" * 40, "checks": sorted(REQUIRED_CHECKS | {"windows-bind-mount"}),
        }
        return {**common, "host": "Linux"}, {**common, "host": "Windows"}

    def test_release_rejects_partial_or_engine_only_results(self):
        for change in ({"scope": "engine-only"}, {"result": "failed"}, {"source_dirty": True}, {"checks": []}, {"checks": None}, {"image_architecture": "arm64"}, {"checks": sorted(REQUIRED_CHECKS - {"missing-source"})}):
            linux, windows = self.reports()
            with self.assertRaises(ValueError):
                validate_evidence(linux, {**windows, **change})

    def test_engine_http_keeps_exact_browser_bytes(self):
        expected = b"\xc3\xa9\r\n"
        result = command(
            [sys.executable, "-c", f"import sys; sys.stdout.buffer.write({expected!r})"],
            text=False,
        )
        self.assertEqual(result.stdout, expected)

    def test_release_rejects_another_candidate_or_missing_windows_mount_check(self):
        linux, windows = self.reports()
        validate_evidence(linux, windows)
        for change in (
            {"image_id": "sha256:" + "c" * 64},
            {"revision": "d" * 40},
            {"checks": sorted(REQUIRED_CHECKS)},
        ):
            with self.assertRaises(ValueError):
                validate_evidence(linux, {**windows, **change})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("build", "check", "validate-evidence", "self-test"))
    parser.add_argument("--image", default="domus-obscura:local")
    parser.add_argument("--engine-only", action="store_true")
    parser.add_argument("--volume-media", action="store_true")
    parser.add_argument("--linux-report", type=Path)
    parser.add_argument("--windows-report", type=Path)
    parser.add_argument("--report", type=Path, default=ROOT / "target" / "docker-check.json")
    args = parser.parse_args()
    if args.volume_media and (args.operation != "check" or not args.engine_only):
        parser.error("Linux-private media diagnostics require check --engine-only")
    if args.operation == "build":
        build(args.image)
    elif args.operation == "self-test":
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(EvidenceTests)
        if not unittest.TextTestRunner().run(suite).wasSuccessful():
            raise RuntimeError("Docker evidence regressions failed")
    elif args.operation == "validate-evidence":
        if not args.linux_report or not args.windows_report:
            parser.error("both platform reports are required")
        validate_evidence(json.loads(args.linux_report.read_text()), json.loads(args.windows_report.read_text()))
    else:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        check = Check(args.image, args.engine_only, args.volume_media)
        with tempfile.TemporaryDirectory(prefix="docker-check-", dir=ROOT / "target") as directory:
            try:
                check.run(Path(directory))
            except Exception as error:
                check.report["error"] = str(error)
                raise
            finally:
                try:
                    check.cleanup()
                finally:
                    args.report.write_text(json.dumps(check.report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, subprocess.SubprocessError, OSError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
