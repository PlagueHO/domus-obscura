"""Bounded Docker checks using only fresh state and generated media."""

import argparse
import base64
import hashlib
import http.client
import io
import itertools
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch
import uuid

ROOT = Path(__file__).resolve().parents[2]
APPLICATION = "/storage/application"
CONFIGURATION = APPLICATION + "/config.json"
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
    "health-bytes",
    "overlay-isolation",
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


def archive_configuration_digest(path):
    with tarfile.open(path) as archive:
        for count, member in enumerate(archive):
            if count >= 256:
                raise ValueError("image archive has too many metadata entries")
        def read(name):
            member = archive.getmember(name)
            if not member.isfile() or not 0 < member.size <= 1024 * 1024:
                raise ValueError("invalid image configuration metadata")
            with archive.extractfile(member) as source:
                return source.read(1024 * 1024 + 1)

        manifests = json.loads(read("manifest.json"))
        if not isinstance(manifests, list) or len(manifests) != 1:
            raise ValueError("one saved candidate image is required")
        configuration = read(manifests[0]["Config"])
        return "sha256:" + hashlib.sha256(configuration).hexdigest()


def image_configuration_digest(image):
    inspected = json.loads(docker("image", "inspect", image).stdout)[0]
    if inspected["Size"] > 1024 * 1024 * 1024:
        raise ValueError("runtime image exceeds the 1 GiB evidence limit")
    (ROOT / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="docker-image-", dir=ROOT / "target") as directory:
        archive = Path(directory) / "candidate.tar"
        docker("image", "save", "--output", str(archive), inspected["Id"], timeout=180)
        return archive_configuration_digest(archive)


def build(image):
    revision = command(["git", "rev-parse", "HEAD"]).stdout.strip()
    dirty = bool(command(["git", "status", "--porcelain"]).stdout.strip())
    common = [
            "docker", "build", "--load", "--provenance=false", "--sbom=false",
            "--platform", "linux/amd64",
            "--build-arg", f"DOMUS_SOURCE_REVISION={revision}",
            "--build-arg", f"DOMUS_SOURCE_DIRTY={str(dirty).lower()}",
    ]
    name = "domus-package-check-" + uuid.uuid4().hex
    verification = "domus-obscura:package-check-" + uuid.uuid4().hex
    volume = name + "-state"
    built = False
    created = False
    try:
        subprocess.run(
            [*common, "--target", "package-verification", "--tag", verification, "."],
            check=True, cwd=ROOT, timeout=3600,
        )
        built = True
        docker("volume", "create", "--label", "io.domus.test=" + name, volume)
        created = True
        result = docker(
            "run", "--rm", "--name", name, "--label", "io.domus.test=" + name,
            "--network", "none", "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--pids-limit", "128",
            "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m",
            "--mount", f"type=volume,source={volume},target=/checks",
            verification, timeout=180,
        )
        print(result.stdout.strip())
        subprocess.run(
            [*common, "--target", "runtime", "--tag", image, "."],
            check=True, cwd=ROOT, timeout=3600,
        )
        def hashes(selected, *paths):
            output = docker(
                "run", "--rm", "--network", "none", "--read-only",
                "--entrypoint", "sha256sum", selected, *paths,
            ).stdout
            return [line.split()[0] for line in output.splitlines()]

        if hashes(
            verification, "/payload/package-manifest.json", "/src/packaging/docker/health.sh",
        ) != hashes(
            image, "/opt/domus-obscura/package-manifest.json", "/usr/local/bin/domus-health",
        ):
            raise RuntimeError("runtime payload or health script differs from the verified build")
    finally:
        existing = docker("container", "inspect", name, check=False)
        if existing.returncode == 0:
            docker("rm", "--force", name)
        elif "No such" not in existing.stderr:
            raise RuntimeError("could not inspect the package-check container: " + existing.stderr)
        if created:
            docker("volume", "rm", volume)
        if built:
            docker("image", "rm", verification)


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
            or report.get("host_runtime") != ("native-linux" if host == "Linux" else "windows-desktop")
            or report.get("image_os") != "linux"
            or report.get("image_architecture") != "amd64"
            or report.get("source_dirty") is not False
            or not REQUIRED_CHECKS.issubset(report.get("checks", []))
        ):
            raise ValueError(f"complete clean-source {host} host evidence is required")
    if (
        not isinstance(linux.get("image_config_digest"), str)
        or not isinstance(linux.get("revision"), str)
        or not re.fullmatch(r"sha256:[0-9a-f]{64}", linux["image_config_digest"])
        or not re.fullmatch(r"[0-9a-f]{40}", linux["revision"])
        or linux["image_config_digest"] != windows.get("image_config_digest")
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
            "host": platform.system(),
            "scope": (
                "engine-only" if engine_only else "private-volume-host" if volume_media else "host"
            ),
            "host_runtime": (
                "wsl" if "microsoft" in platform.release().lower()
                else "native-linux" if platform.system() == "Linux"
                else "windows-desktop" if platform.system() == "Windows" else "unsupported"
            ),
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

    def arguments(self, writable_media=False, media_alias=False, program_readonly=True):
        arguments = [
            "--init", "--user", "1000:1000", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--network", "host",
            "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m",
        ]
        if program_readonly:
            arguments.append("--read-only")
        for volume, target in self.mounts:
            arguments += ["--mount", f"type=volume,source={volume},target={target}"]
        if self.media_volume or self.media:
            arguments += ["--mount", self.media_mount("/media", writable_media)]
        if media_alias:
            application = next(volume for volume, target in self.mounts if target == "/storage")
            arguments += ["--mount", f"type=volume,source={application},target=/application-alias"]
        return arguments

    def once(self, arguments, *, input=None, shell=False, check=True, writable_media=False, media_alias=False, program_readonly=True):
        self.report["phase"] = "shell" if shell else arguments[0]
        name = self.prefix + "-once-" + uuid.uuid4().hex
        self.containers.append(name)
        result = docker(
            "run", "--rm", "--name", name, "--interactive",
            *self.arguments(writable_media, media_alias, program_readonly),
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
            *(["serve", "--storage-root", APPLICATION] if role == "api" else ["worker", CONFIGURATION]),
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

    def wait_scan(self, request_id):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            requests = json.loads(self.request("/api/worker/requests"))
            matching = [item for item in requests if item["id"] == request_id]
            if matching and matching[0]["state"] == "complete":
                return
            if matching and matching[0]["state"] in ("failed", "waiting"):
                self.report["scan_locations"] = json.loads(self.request("/api/locations"))
                raise RuntimeError("generated worker scan failed: " + json.dumps(matching))
            time.sleep(0.5)
        raise RuntimeError("generated worker scan timed out")

    def queue_scan(self, location, guards):
        previous = {item["id"] for item in json.loads(self.request("/api/worker/requests"))}
        self.request(f"/api/locations/{location}/scan", "POST", {}, guards, 202)
        created = [
            item for item in json.loads(self.request("/api/worker/requests"))
            if item["id"] not in previous
        ]
        if len(created) != 1:
            raise RuntimeError("scan did not create exactly one durable worker request")
        return created[0]["id"]

    def inspect_root(self, boundary, scope, root_id, role, relative="", **options):
        return json.loads(self.once([
            "storage-root-inspect", boundary, scope, root_id, "1", role, relative,
        ], **options).stdout)

    def save_fixture_configuration(self, configuration, path=CONFIGURATION):
        # These assurances describe only the fresh test-owned volume topology.
        # This is not a configuration generator or approval for operator storage.
        selection = configuration["storage"]
        roots = []
        # Match Rust struct order even when saved JSON sorts object keys.
        for root in sorted(selection["roots"], key=lambda root: root["root_id"]):
            canonical = {key: root[key] for key in (
                "root_id", "binding_revision", "role", "provider_scope", "relative", "expected",
            )}
            canonical["expected"] = {
                key: root["expected"][key] for key in ("profile", "components")
            }
            roots.append(canonical)
        semantic = [
            selection["format_version"], selection["selection_revision"],
            sorted(provider["scope"] for provider in selection["providers"]),
            roots,
            selection["expected_selection_file"],
        ]
        digest = hashlib.sha256(
            b"domus-storage-selection-v2\0"
            + json.dumps(semantic, separators=(",", ":"), ensure_ascii=False).encode()
        ).hexdigest()
        selection["topology_assurances"] = [{
            "left": {"root_id": left["root_id"], "binding_revision": left["binding_revision"]},
            "right": {"root_id": right["root_id"], "binding_revision": right["binding_revision"]},
            "selection_digest": digest,
        } for left, right in itertools.combinations(
            sorted(selection["roots"], key=lambda root: root["root_id"]), 2
        )]
        content = json.dumps(configuration)
        self.once(["-c", f"umask 077; cat > {path}"], shell=True, input=content)
        return hashlib.sha256(content.encode()).hexdigest()

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
            "image_config_digest": image_configuration_digest(self.image),
            "source_dirty": labels.get("io.domus.source.dirty") != "false",
        })
        if not self.engine_only and self.report["host_runtime"] not in ("native-linux", "windows-desktop"):
            raise RuntimeError("host acceptance requires native Linux or Windows Docker Desktop, not WSL")
        if platform.system() == "Linux" and not self.engine_only:
            engine_system = docker("info", "--format", "{{.OperatingSystem}}").stdout.strip()
            if "docker desktop" in engine_system.lower():
                raise RuntimeError("Docker Desktop is not native Linux-host evidence")
        shipped_health = self.once(
            ["-c", "sha256sum /usr/local/bin/domus-health"], shell=True,
        ).stdout.split()[0]
        if shipped_health != hashlib.sha256(Path(__file__).with_name("health.sh").read_bytes()).hexdigest():
            raise RuntimeError("shipped health script differs from the checked source")
        self.report["checks"].append("health-bytes")
        denied_state = self.once([
            "init", "--storage-root", "/opt/domus-obscura/disallowed-private-state",
        ], check=False)
        if not denied_state.returncode or "verified supported local filesystem" not in denied_state.stderr:
            raise RuntimeError("private state was not refused on the image overlay filesystem")
        writable_assets = self.once(
            ["serve", "--storage-root", APPLICATION], check=False, program_readonly=False,
        )
        if not writable_assets.returncode or "installed browser assets require" not in writable_assets.stderr:
            raise RuntimeError("browser assets were not refused on a writable image overlay")
        self.report["checks"].append("overlay-isolation")
        self.mounts = [(self.volume("application"), "/storage")]
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
            self.once(["init", "--storage-root", APPLICATION])
            self.once(
                ["-c", "set -eu; umask 077; mkdir -p /media/Artist/Album /storage/outside; "
                 "base64 -d > /media/Artist/Album/01.mp3; "
                 "cp /media/Artist/Album/01.mp3 /media/Artist/Album/02.mp3; "
                 "printf 'outside generated fixture\\n' > /storage/outside/must-not-index.txt; "
                 "ln -s /storage/outside /media/escape-link"],
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
            "root": "cache",
            "limits": {"item_bytes": 1048576, "total_bytes": 16777216, "entries": 16},
        }
        self.once(["-c", f"umask 077; mkdir -p {APPLICATION}/cache {APPLICATION}/staging"], shell=True)
        selection = configuration["storage"]
        selection["selection_revision"] += 1
        selection["providers"].append({"scope": "media", "boundary": "/media"})
        selection["roots"] += [
            self.inspect_root(APPLICATION, "application", "cache", "cache", "cache"),
            self.inspect_root(APPLICATION, "application", "staging", "staging", "staging"),
            self.inspect_root("/media", "media", "media", "media"),
        ]
        revision = self.save_fixture_configuration(configuration)
        # Explicit startup binds this fresh cache; passive validation cannot.
        self.stop(self.start("api"))
        validation = self.once(["configuration-check", CONFIGURATION], check=False)
        if validation.returncode:
            self.report["configuration_validation"] = {
                "stdout": validation.stdout, "stderr": validation.stderr,
            }
            raise RuntimeError(
                "generated host configuration validation failed: " + validation.stdout + validation.stderr
            )
        location = self.once(["location-add", "--host", CONFIGURATION, "/media", "main"]).stdout.strip()
        self.once(["staging-init", "--host", CONFIGURATION, APPLICATION + "/staging"])
        self.once(["staging-init", "--host", CONFIGURATION, APPLICATION + "/staging"])
        bad = json.loads(json.dumps(configuration))
        bad["storage"]["selection_revision"] += 1
        bad["storage"]["providers"].append({"scope": "alias", "boundary": "/application-alias"})
        bad["storage"]["roots"].append(self.inspect_root(
            "/application-alias", "alias", "private-alias", "media", "application/cache", media_alias=True,
        ))
        self.save_fixture_configuration(bad, APPLICATION + "/overlap.json")
        overlap = self.once([
            "location-add", "--host", APPLICATION + "/overlap.json", "/application-alias/application/cache", "backup",
        ], check=False, media_alias=True)
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
        queued = self.queue_scan(location, guards)
        self.wait_scan(queued)
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
        queued = self.queue_scan(location, guards)
        self.stop(api, forced=True)
        api = self.start("api")
        worker = self.start("worker")
        self.wait_worker(revision)
        self.wait_scan(queued)
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
        changed = self.once(["scan", "--host", CONFIGURATION, location], check=False)
        if not changed.returncode:
            raise RuntimeError("a replacement media root was accepted")
        retained = json.loads(self.once(["browse", "--host", CONFIGURATION, location, "Artist/Album", "0", "100"]).stdout)
        if {entry["relative_path"] for entry in retained} != {"Artist/Album/01.mp3", "Artist/Album/02.mp3"}:
            raise RuntimeError("failed mount access erased retained observations")
        self.media, self.media_volume = original_media, original_volume
        self.once(["-c", f"set -eu; mv {APPLICATION}/cache {APPLICATION}/cache-retained; "
                   f"mkdir -m 0700 {APPLICATION}/cache; "
                   f"cp {APPLICATION}/cache-retained/owner.lock {APPLICATION}/cache/owner.lock"], shell=True)
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
            "image_config_digest": "sha256:" + "e" * 64,
            "revision": "b" * 40, "checks": sorted(REQUIRED_CHECKS | {"windows-bind-mount"}),
        }
        return (
            {**common, "host": "Linux", "host_runtime": "native-linux"},
            {**common, "host": "Windows", "host_runtime": "windows-desktop"},
        )

    def test_release_rejects_partial_or_engine_only_results(self):
        for change in ({"scope": "engine-only"}, {"scope": "private-volume-host"}, {"result": "failed"}, {"source_dirty": True}, {"checks": []}, {"checks": None}, {"image_architecture": "arm64"}, {"checks": sorted(REQUIRED_CHECKS - {"missing-source"})}):
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
            {"image_config_digest": "sha256:" + "c" * 64},
            {"revision": "d" * 40},
            {"checks": sorted(REQUIRED_CHECKS)},
            {"host_runtime": "wsl"},
        ):
            with self.assertRaises(ValueError):
                validate_evidence(linux, {**windows, **change})
        validate_evidence(linux, {**windows, "image_id": "sha256:" + "c" * 64})

    def test_release_rejects_wsl_as_native_linux(self):
        linux, windows = self.reports()
        with self.assertRaises(ValueError):
            validate_evidence({**linux, "host_runtime": "wsl"}, windows)

    def test_private_volume_host_diagnostic_rejects_engine_mode_and_non_windows(self):
        for host, extra in (("Windows", ["--engine-only"]), ("Linux", [])):
            with (
                patch.object(sys, "argv", ["check.py", "check", "--windows-private-volume-host", *extra]),
                patch.object(platform, "system", return_value=host),
                patch("sys.stderr", new=io.StringIO()),
                patch(__name__ + ".docker") as execute,
            ):
                with self.assertRaises(SystemExit):
                    main()
                execute.assert_not_called()

    def test_scan_wait_uses_the_new_request_not_the_returned_location_id(self):
        check = Check("unused", True)
        with patch.object(check, "request", side_effect=[
            b'[{"id": 1, "state": "complete"}]',
            b'{"id": 1}',
            b'[{"id": 1, "state": "complete"}, {"id": 2, "state": "queued"}]',
        ]):
            self.assertEqual(check.queue_scan("1", {}), 2)

    def test_fixture_assurance_accepts_sorted_configuration_and_struct_field_order(self):
        root = {
            "root_id": "application", "binding_revision": 1, "role": "application",
            "provider_scope": "application", "relative": "",
            "expected": {
                "profile": "linux-exportfs-chain-v1",
                "components": ["linuxfh:0100000000000000:00000001:01"],
            },
        }
        configuration = {"storage": {
            "format_version": 1, "selection_revision": 1,
            "providers": [{"scope": "application", "boundary": APPLICATION}],
            "roots": [root, {**root, "root_id": "state", "role": "state"}],
            "expected_selection_file": None,
        }}
        digests = []
        for content in (
            json.dumps(configuration), json.dumps(configuration, sort_keys=True),
        ):
            check = Check("unused", True)
            with patch.object(check, "once") as saved:
                check.save_fixture_configuration(json.loads(content))
            stored = json.loads(saved.call_args.kwargs["input"])
            digests.append(stored["storage"]["topology_assurances"][0]["selection_digest"])
        self.assertEqual(digests[0], digests[1])

    def test_candidate_digest_survives_legacy_and_containerd_archive_names(self):
        configuration = b'{"architecture":"amd64","rootfs":{"type":"layers","diff_ids":[]}}'
        digest = hashlib.sha256(configuration).hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            for name in (digest + ".json", "blobs/sha256/" + digest):
                path = Path(directory) / "image.tar"
                with tarfile.open(path, "w") as archive:
                    for entry, content in (
                        (name, configuration),
                        ("manifest.json", json.dumps([{"Config": name, "Layers": []}]).encode()),
                    ):
                        member = tarfile.TarInfo(entry)
                        member.size = len(content)
                        archive.addfile(member, io.BytesIO(content))
                self.assertEqual(archive_configuration_digest(path), "sha256:" + digest)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("build", "check", "validate-evidence", "self-test"))
    parser.add_argument("--image", default="domus-obscura:local")
    parser.add_argument("--engine-only", action="store_true")
    parser.add_argument("--volume-media", action="store_true")
    parser.add_argument("--windows-private-volume-host", action="store_true")
    parser.add_argument("--linux-report", type=Path)
    parser.add_argument("--windows-report", type=Path)
    parser.add_argument("--report", type=Path, default=ROOT / "target" / "docker-check.json")
    args = parser.parse_args()
    if args.volume_media and (args.operation != "check" or not args.engine_only):
        parser.error("Linux-private media diagnostics require check --engine-only")
    if args.windows_private_volume_host and (
        args.operation != "check" or args.engine_only or args.volume_media
        or platform.system() != "Windows"
    ):
        parser.error("Windows private-volume host diagnostics require native Windows check mode")
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
        check = Check(
            args.image, args.engine_only,
            args.volume_media or args.windows_private_volume_host,
        )
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
