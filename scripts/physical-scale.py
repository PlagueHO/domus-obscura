#!/usr/bin/env python3
"""Opt-in generated scan evidence. Never provisions hosts or reads media content."""

import argparse
import collections
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import time
import platform
import urllib.request
import secrets
import socket
import subprocess
import threading
import uuid
import select
import sqlite3
import shutil

LIMIT_BYTES = 512 * 1024 * 1024
LOCAL_FILES = 4096
LOCAL_BYTES = 1024 * 1024 * 1024
MAX_DEPTH = 256
MAX_GROUPS = 256


def require(condition, message):
    if not condition:
        raise ValueError(message)


def write_new(path, value):
    with open(path, "x", encoding="utf-8", newline="\n") as output:
        json.dump(value, output, indent=2, sort_keys=True)
        output.write("\n")


def group(value):
    if value == 0:
        return "0"
    if value <= 10:
        return "1-10"
    if value <= 100:
        return "11-100"
    if value <= 1000:
        return "101-1000"
    return "1001+"


class Inventory:
    def __init__(self):
        self.files = 0
        self.logical_bytes = 0
        self.allocated_bytes = 0
        self.directories = 0
        self.links = 0
        self.other = 0
        self.extensions = collections.Counter()
        self.depths = collections.Counter()
        self.widths = collections.Counter()
        self.errors = collections.Counter()

    def file(self, name, size, allocated=None):
        self.files += 1
        self.logical_bytes += size
        self.allocated_bytes += max(size, allocated if allocated is not None else size)
        extension = Path(name).suffix.lower()
        # Never retain arbitrary source filenames or unbounded extension keys.
        extension = extension if extension in (
            ".mp3", ".flac", ".jpg", ".jpeg", ".png", ".m4a", ".wav",
            ".ogg", ".opus", ".txt", ".log", ".cue", ".nfo", ".pdf",
        ) else "other"
        self.extensions[extension] += 1

    def report(self):
        return {
            "schema": 1,
            "kind": "read_only_aggregate_inventory",
            "complete": not self.errors,
            "regular_files": self.files,
            "full_profile_files": 2 * self.files if not self.errors else None,
            "logical_bytes": self.logical_bytes,
            "allocated_file_bytes": self.allocated_bytes,
            "directories": self.directories,
            "excluded_links_or_reparse_points": self.links,
            "other_entries": self.other,
            "extensions": dict(self.extensions),
            "directories_by_depth": dict(self.depths),
            "files_per_directory": dict(self.widths),
            "errors": dict(self.errors),
            "snapshot": False,
            "limits": "Concurrent source changes are not a filesystem snapshot.",
        }

    def error(self, error):
        # Diagnostics must not export source paths, exception text or names.
        code = getattr(error, "winerror", None) or getattr(error, "errno", None)
        key = f"{type(error).__name__}:{code}"
        if key not in self.errors and len(self.errors) >= MAX_GROUPS:
            key = "other"
        self.errors[key] += 1


def _expected_churn(error):
    return isinstance(error, (FileNotFoundError, NotADirectoryError)) or getattr(
        error, "winerror", None
    ) in (2, 3, 267)


def inventory_posix(root, result, tolerate_churn=False):
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    require(root.is_absolute(), "inventory root must be absolute")
    with contextlib.ExitStack() as parents:
        fd = os.open(root.anchor, flags)
        parents.callback(os.close, fd)
        for component in root.parts[1:]:
            fd = os.open(component, flags, dir_fd=fd)
            parents.callback(os.close, fd)
        _inventory_posix_directory(fd, 0, flags, result, tolerate_churn)


def _inventory_posix_directory(fd, depth, flags, result, tolerate_churn):
    require(depth <= MAX_DEPTH, "inventory depth limit exceeded")
    count = 0
    result.directories += 1
    result.depths[str(depth)] += 1
    with os.scandir(fd) as entries:
        for entry in entries:
            try:
                info = entry.stat(follow_symlinks=False)
                if stat.S_ISLNK(info.st_mode):
                    result.links += 1
                elif stat.S_ISDIR(info.st_mode):
                    child = os.open(entry.name, flags, dir_fd=fd)
                    try:
                        actual = os.fstat(child)
                        require((actual.st_dev, actual.st_ino) == (info.st_dev, info.st_ino),
                                "inventory directory identity changed")
                        _inventory_posix_directory(
                            child, depth + 1, flags, result, tolerate_churn)
                    finally:
                        os.close(child)
                elif stat.S_ISREG(info.st_mode):
                    count += 1
                    result.file(entry.name, info.st_size, info.st_blocks * 512)
                else:
                    result.other += 1
            except (OSError, ValueError) as error:
                if not tolerate_churn or not _expected_churn(error):
                    result.error(error)
    result.widths[group(count)] += 1


class Windows:
    """Keep directory ancestors pinned; enumerate metadata on the no-follow handle."""

    REPARSE = 0x400
    DIRECTORY = 0x10
    NO_MORE_FILES = 18

    class DirectoryEntry(ctypes.Structure):
        _pack_ = 1
        _fields_ = [
            ("next", ctypes.c_uint32), ("index", ctypes.c_uint32),
            ("created", ctypes.c_int64), ("accessed", ctypes.c_int64),
            ("written", ctypes.c_int64), ("changed", ctypes.c_int64),
            ("size", ctypes.c_int64), ("allocated", ctypes.c_int64),
            ("attributes", ctypes.c_uint32), ("name_bytes", ctypes.c_uint32),
            ("ea_size", ctypes.c_uint32),
        ]

    class AttributeTag(ctypes.Structure):
        _fields_ = [("attributes", ctypes.c_uint32), ("tag", ctypes.c_uint32)]

    class Memory(ctypes.Structure):
        _fields_ = [
            ("cb", ctypes.c_uint32), ("faults", ctypes.c_uint32),
            ("peak", ctypes.c_size_t), ("working", ctypes.c_size_t),
            ("peak_paged", ctypes.c_size_t), ("paged", ctypes.c_size_t),
            ("peak_nonpaged", ctypes.c_size_t), ("nonpaged", ctypes.c_size_t),
            ("pagefile", ctypes.c_size_t), ("peak_pagefile", ctypes.c_size_t),
        ]

    def __init__(self):
        from ctypes import wintypes
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self.kernel.CreateFileW.argtypes = [
            wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
            wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
        ]
        self.kernel.CreateFileW.restype = wintypes.HANDLE
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.CloseHandle.restype = wintypes.BOOL
        self.kernel.GetFileInformationByHandleEx.argtypes = [
            wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD,
        ]
        self.kernel.GetFileInformationByHandleEx.restype = wintypes.BOOL
        self.kernel.OpenProcess.argtypes = [
            wintypes.DWORD, wintypes.BOOL, wintypes.DWORD,
        ]
        self.kernel.OpenProcess.restype = wintypes.HANDLE
        self.kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [
            ctypes.POINTER(wintypes.FILETIME)
        ] * 4
        self.kernel.GetProcessTimes.restype = wintypes.BOOL
        self.psapi.GetProcessMemoryInfo.argtypes = [
            wintypes.HANDLE, ctypes.POINTER(self.Memory), wintypes.DWORD,
        ]
        self.psapi.GetProcessMemoryInfo.restype = wintypes.BOOL

    def check(self, success):
        if not success:
            raise ctypes.WinError(ctypes.get_last_error())

    def directory(self, path, allow_delete=False):
        text = str(path)
        if not text.startswith("\\\\?\\"):
            text = "\\\\?\\UNC\\" + text[2:] if text.startswith("\\\\") else "\\\\?\\" + text
        share = 0x1 | 0x2 | (0x4 if allow_delete else 0)
        # Live storage samples allow renames; source inventories pin their ancestors.
        handle = self.kernel.CreateFileW(text, 0x1 | 0x80, share, None, 3,
                                        0x02000000 | 0x00200000, None)
        self.check(handle != ctypes.c_void_p(-1).value)
        try:
            info = self.AttributeTag()
            self.check(self.kernel.GetFileInformationByHandleEx(
                handle, 9, ctypes.byref(info), ctypes.sizeof(info)))
            require(info.attributes & self.DIRECTORY and not info.attributes & self.REPARSE,
                    "inventory directory is not a no-follow regular directory")
            return handle
        except BaseException:
            self.kernel.CloseHandle(handle)
            raise

    def entries(self, handle):
        buffer = ctypes.create_string_buffer(64 * 1024)
        while True:
            success = self.kernel.GetFileInformationByHandleEx(
                handle, 14, buffer, len(buffer))
            if not success and ctypes.get_last_error() == self.NO_MORE_FILES:
                return
            self.check(success)
            offset = 0
            while True:
                size = ctypes.sizeof(self.DirectoryEntry)
                require(offset + size <= len(buffer), "invalid directory response")
                item = self.DirectoryEntry.from_buffer_copy(buffer, offset)
                end = offset + size + item.name_bytes
                require(item.name_bytes % 2 == 0 and end <= len(buffer),
                        "invalid directory name response")
                name = buffer.raw[offset + size:end].decode("utf-16-le")
                if name not in (".", ".."):
                    yield name, item
                if not item.next:
                    break
                require(item.next >= size and offset + item.next < len(buffer),
                        "invalid directory response offset")
                offset += item.next

    def inventory(self, root, result, tolerate_churn=False):
        require(root.is_absolute(), "inventory root must be absolute")
        with contextlib.ExitStack() as parents:
            path = Path(root.anchor)
            handle = self.directory(path, allow_delete=tolerate_churn)
            parents.callback(self.kernel.CloseHandle, handle)
            for component in root.parts[1:]:
                path /= component
                handle = self.directory(path, allow_delete=tolerate_churn)
                parents.callback(self.kernel.CloseHandle, handle)
            self._inventory_directory(handle, path, 0, result, tolerate_churn)

    def _inventory_directory(self, handle, path, depth, result, tolerate_churn):
        require(depth <= MAX_DEPTH, "inventory depth limit exceeded")
        result.directories += 1
        result.depths[str(depth)] += 1
        count = 0
        for name, info in self.entries(handle):
            try:
                if info.attributes & self.REPARSE:
                    result.links += 1
                elif info.attributes & self.DIRECTORY:
                    child = self.directory(path / name, allow_delete=tolerate_churn)
                    try:
                        self._inventory_directory(
                            child, path / name, depth + 1, result, tolerate_churn)
                    finally:
                        self.kernel.CloseHandle(child)
                else:
                    require(info.size >= 0, "negative inventory size")
                    result.file(name, info.size, info.allocated)
                    count += 1
            except (OSError, ValueError) as error:
                if not tolerate_churn or not _expected_churn(error):
                    result.error(error)
        result.widths[group(count)] += 1

    def process(self, pid):
        handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        self.check(handle)
        try:
            return WindowsProcess(self, handle)
        except BaseException:
            self.kernel.CloseHandle(handle)
            raise


class WindowsProcess:
    def __init__(self, windows, handle):
        from ctypes import wintypes
        self.windows = windows
        self.handle = handle
        times = [wintypes.FILETIME() for _ in range(4)]
        windows.check(windows.kernel.GetProcessTimes(
            handle, *(ctypes.byref(value) for value in times)))
        self.identity = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
        self.peak = 0

    def sample(self):
        memory = self.windows.Memory()
        memory.cb = ctypes.sizeof(memory)
        self.windows.check(self.windows.psapi.GetProcessMemoryInfo(
            self.handle, ctypes.byref(memory), memory.cb))
        self.peak = max(self.peak, memory.peak)
        return memory.working

    def close(self):
        self.windows.kernel.CloseHandle(self.handle)


def linux_identity(pid):
    text = Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
    return text.rsplit(")", 1)[1].split()[19]


def parse_kib_fields(text):
    fields = {}
    for line in text.splitlines():
        parts = line.split()
        if parts and parts[0] in ("Rss:", "VmHWM:"):
            require(len(parts) == 3 and parts[2] == "kB",
                    "unexpected resident-memory unit")
            value = int(parts[1])
            require(value >= 0, "negative resident memory")
            fields[parts[0][:-1]] = value * 1024
    return fields


class LinuxProcess:
    def __init__(self, pid):
        self.pid = pid
        self.identity = linux_identity(pid)
        self.peak = 0

    def sample(self):
        require(linux_identity(self.pid) == self.identity, "process identity changed")
        current = parse_kib_fields(
            Path(f"/proc/{self.pid}/smaps_rollup").read_text(encoding="ascii"))
        status = parse_kib_fields(
            Path(f"/proc/{self.pid}/status").read_text(encoding="ascii"))
        require("Rss" in current and "VmHWM" in status, "resident-memory evidence missing")
        require(linux_identity(self.pid) == self.identity, "process identity changed")
        self.peak = max(self.peak, current["Rss"], status["VmHWM"])
        return current["Rss"]

    def close(self):
        pass


def memory_summary(samples, peaks, complete, active_window):
    require(all(type(value) is int and value >= 0 for value in peaks.values()),
            "invalid resident-memory peak")
    complete = complete and set(peaks) == {"api", "worker"} and all(
        peaks[role] >= max((sample[f"{role}_bytes"] for sample in samples), default=0)
        for role in ("api", "worker")
    )
    totals = [sample["api_bytes"] + sample["worker_bytes"] for sample in samples]
    steady = [
        sample["api_bytes"] + sample["worker_bytes"] for sample in samples
        if sample["phase"] == "active"
    ]
    sampled_peak = max(totals, default=None)
    conservative = sum(peaks.values()) if set(peaks) == {"api", "worker"} else None
    if sampled_peak is not None and sampled_peak > LIMIT_BYTES:
        verdict = "failed"
    elif any(peak > LIMIT_BYTES for peak in peaks.values()):
        verdict = "failed"
    elif (complete and active_window and steady and conservative is not None
          and conservative <= LIMIT_BYTES):
        verdict = "passed_profile_only"
    else:
        verdict = "inconclusive"
    return {
        "threshold_bytes": LIMIT_BYTES,
        "sampled_combined_peak_bytes": sampled_peak,
        "per_process_peak_bytes": peaks,
        "conservative_sum_of_peaks_bytes": conservative,
        "steady_mean_bytes": sum(steady) // len(steady) if steady else None,
        "steady_max_bytes": max(steady, default=None),
        "complete": complete,
        "verified_active_window": active_window,
        "verdict": verdict,
        "limits": (
            "Paired samples are sequential native reads, not an atomic snapshot. "
            "Per-process peaks need not coincide. Shared resident pages count in both processes. "
            "Linux VmHWM is approximate. Test-build results are not production acceptance."
        ),
    }


def read_manifest(path):
    with open(path, "rb") as source:
        body = source.read(1024 * 1024 + 1)
    require(len(body) <= 1024 * 1024, "manifest exceeds size limit")
    manifest = json.loads(body)
    require(manifest.get("schema") == 1
            and manifest.get("kind") == "read_only_aggregate_inventory"
            and manifest.get("complete") is True
            and manifest.get("errors") == {}, "complete inventory is required")
    count = manifest.get("regular_files")
    require(type(count) is int and count > 0, "positive inventory count is required")
    require(manifest.get("full_profile_files") == count * 2, "invalid doubled file count")
    return manifest, hashlib.sha256(body).hexdigest()


def admission(profile, manifest_path=None, admission_path=None):
    if profile == "small":
        require(manifest_path is None and admission_path is None,
                "small profile cannot use full-profile inputs")
        return {
            "profile": "small", "files": LOCAL_FILES - 96, "max_run_bytes": LOCAL_BYTES,
            "max_generated_files": LOCAL_FILES,
            "baseline_sha256": None, "full_size_acceptance": False,
        }
    require(profile == "full" and manifest_path and admission_path,
            "full profile requires the frozen manifest and separate lab admission")
    manifest, digest = read_manifest(manifest_path)
    with open(admission_path, "rb") as source:
        body = source.read(64 * 1024 + 1)
    require(len(body) <= 64 * 1024, "lab admission exceeds size limit")
    approval = json.loads(body)
    require(approval.get("schema") == 1
            and approval.get("baseline_sha256") == digest
            and approval.get("platform") == platform.system()
            and approval.get("azure_only") is True
            and approval.get("separate_resource_approval") is True
            and approval.get("cleanup_verified") is True,
            "lab admission does not match the separately approved Azure run")
    require(isinstance(approval.get("approval_reference"), str)
            and approval["approval_reference"].strip(), "approval reference is required")
    limit = approval.get("max_run_bytes")
    require(type(limit) is int and limit > 0, "positive approved storage bound is required")
    require(type(approval.get("deadline_unix")) in (int, float)
            and approval["deadline_unix"] > time.time(), "lab admission has expired")
    require(type(approval.get("max_runtime_seconds")) is int
            and 0 < approval["max_runtime_seconds"] <= 86400,
            "finite approved runtime bound is required")
    require(isinstance(approval.get("vm_id"), str) and approval["vm_id"],
            "exact Azure VM identity is required")
    if platform.system() == "Linux":
        require(approval.get("linux_permission_fixture_approved") is True,
                "full Linux admission requires separate Linux permission approval")
    return {
        "profile": "full", "files": manifest["full_profile_files"],
        "max_run_bytes": limit, "baseline_sha256": digest,
        "full_size_acceptance": False,
        "deadline_unix": approval["deadline_unix"],
        "max_runtime_seconds": approval["max_runtime_seconds"],
        "approval_reference": approval["approval_reference"],
        "vm_id": approval["vm_id"],
        "linux_permission_fixture_approved":
            approval.get("linux_permission_fixture_approved") is True,
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        raise ValueError("metadata redirects are not permitted")


def verify_azure_vm(expected):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    request = urllib.request.Request(
        "http://169.254.169.254/metadata/instance/compute?api-version=2021-02-01",
        headers={"Metadata": "true"})
    with opener.open(request, timeout=3) as response:
        body = response.read(64 * 1024 + 1)
    require(len(body) <= 64 * 1024, "Azure metadata response exceeds limit")
    compute = json.loads(body)
    require(compute.get("vmId") == expected, "this host is not the approved Azure VM")
    # Do not export subscription, account or host identifiers.
    return {key: compute.get(key) for key in ("vmSize", "location", "osType")}


def storage(root, maximum_files=None):
    observed = Inventory()
    if os.name == "nt":
        Windows().inventory(root, observed, tolerate_churn=True)
    else:
        inventory_posix(root, observed, tolerate_churn=True)
    require(not observed.errors, "cannot verify generated run storage")
    if maximum_files is not None:
        require(observed.files <= maximum_files, "generated file-count bound exceeded")
    # Include a directory allocation allowance; filesystem-wide metadata is not
    # attributable precisely to one run.
    return observed.allocated_bytes + observed.directories * 4096


class Monitor:
    def __init__(self, output, settings):
        self.output = output
        self.settings = settings
        self.processes = {}
        self.retired = {}
        self.expected_exits = set()
        self.samples = []
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        self.thread = None
        self.budget_thread = None
        self.phase = "startup"
        self.failure = None
        self.started = time.monotonic()
        self.max_storage = 0
        self.max_gap = 0
        self.sequence = 0
        self.deadline = min(
            time.time() + settings.get("max_runtime_seconds", 600),
            settings.get("deadline_unix", time.time() + 600))

    def add(self, role, process):
        with self.lock:
            require(role not in self.processes, "duplicate measured process role")
            native = Windows().process(process.pid) if os.name == "nt" else LinuxProcess(process.pid)
            self.expected_exits.discard(role)
            self.processes[role] = (process, native)

    def start(self):
        self.thread = threading.Thread(target=self._collect, name="scale-memory")
        self.budget_thread = threading.Thread(target=self._budget, name="scale-storage")
        self.thread.start()
        self.budget_thread.start()

    def expect_exit(self, role):
        with self.lock:
            self.expected_exits.add(role)

    def retire(self, role, child, forced=False):
        child.wait(forced=forced)
        with self.lock:
            _, native = self.processes.pop(role)
            try:
                if os.name == "nt":
                    native.sample()
                else:
                    require(child.final_peak_bytes is not None, "final peak RSS is missing")
                    native.peak = max(native.peak, child.final_peak_bytes)
                self.retired.setdefault(role, []).append((native.peak, str(native.identity)))
            finally:
                native.close()

    def check(self):
        if self.failure:
            raise RuntimeError(self.failure)
        require(time.time() < self.deadline, "approved scale runtime expired")

    def _collect(self):
        try:
            previous = time.monotonic()
            with open(self.output / "samples.ndjson", "x", encoding="utf-8") as evidence:
                while not self.stopping.is_set():
                    now = time.monotonic()
                    self.max_gap = max(self.max_gap, now - previous)
                    previous = now
                    require(time.time() < self.deadline, "approved scale runtime expired")
                    with self.lock:
                        values = {}
                        for role, (_, native) in self.processes.items():
                            if role not in self.expected_exits:
                                values[role] = native.sample()
                        if set(values) == {"api", "worker"}:
                            require(len(self.samples) < 100000, "memory sample bound exceeded")
                            sample = {
                                "sequence": self.sequence, "phase": self.phase,
                                "elapsed_seconds": now - self.started,
                                "api_bytes": values["api"], "worker_bytes": values["worker"],
                            }
                            self.sequence += 1
                            self.samples.append(sample)
                            evidence.write(json.dumps(sample, separators=(",", ":")) + "\n")
                            evidence.flush()
                    self.stopping.wait(0.1)
        except (OSError, ValueError) as error:
            self._fail(f"native measurement stopped: {error}")

    def _budget(self):
        try:
            while not self.stopping.is_set():
                self.max_storage = max(self.max_storage, storage(
                    self.output, self.settings.get("max_generated_files")))
                require(self.max_storage <= self.settings["max_run_bytes"],
                        "generated run storage bound exceeded")
                self.stopping.wait(2)
        except (OSError, ValueError) as error:
            self._fail(f"storage measurement stopped: {error}")

    def _fail(self, reason):
        self.failure = reason
        self.stopping.set()
        # Only exact child processes owned by this controller are stopped.
        with self.lock:
            for process, _ in self.processes.values():
                if process.poll() is None:
                    process.kill()

    def finish(self, active_window, steady_window=None):
        self.stopping.set()
        if self.thread:
            self.thread.join()
        if self.budget_thread:
            self.budget_thread.join()
        peaks = {}
        identities = {}
        with self.lock:
            for role, segments in self.retired.items():
                peaks[role] = max(peak for peak, _ in segments)
                identities[role] = [identity for _, identity in segments]
            for role, (_, native) in self.processes.items():
                peaks[role] = max(peaks.get(role, 0), native.peak)
                identities.setdefault(role, []).append(str(native.identity))
                native.close()
        complete = not self.failure and set(self.retired) == {"api", "worker"}
        summary_samples = self.samples
        if steady_window is not None:
            begin, end = steady_window
            require(end - begin >= 2, "verified steady workload window is too short")
            summary_samples = [
                {**sample, "phase": "active" if begin <= sample["elapsed_seconds"] <= end else "other"}
                for sample in self.samples
            ]
        result = memory_summary(summary_samples, peaks, complete, active_window)
        phases = collections.defaultdict(list)
        for sample in self.samples:
            phases[sample["phase"]].append(sample["api_bytes"] + sample["worker_bytes"])
        result.update({
            "process_start_identities": identities,
            "sample_interval_seconds": 0.1,
            "maximum_sample_gap_seconds": self.max_gap,
            "sample_count": len(self.samples),
            "maximum_observed_run_storage_bytes": self.max_storage,
            "error": self.failure,
            "steady_window_elapsed_seconds": steady_window,
            "phase_samples": {
                phase: {"count": len(values), "mean_combined_bytes": sum(values) // len(values),
                        "maximum_combined_bytes": max(values)}
                for phase, values in phases.items()
            },
        })
        if self.max_gap > 1:
            result["complete"] = False
            if result["verdict"] != "failed":
                result["verdict"] = "inconclusive"
        return result


class Child:
    def __init__(self, executable, command, output, monitor=None):
        self.process = None
        self.channel = None
        self.buffer = b""
        self.log = None
        self.final_peak_bytes = None
        token = secrets.token_hex(32)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            listener.settimeout(30)
            environment = {
                key: value for key, value in os.environ.items()
                if not key.startswith("DOMUS_SCALE_")
            }
            environment["DOMUS_SCALE_CONTROL"] = f"127.0.0.1:{listener.getsockname()[1]}"
            environment["DOMUS_SCALE_TOKEN"] = token
            environment["DOMUS_SCALE_ROLE"] = command["role"]
            self.log = open(output / f"{command['role']}-{uuid.uuid4().hex}.log", "xb")
            try:
                self.process = subprocess.Popen(
                    [str(executable), "--exact", "scale_tests::scale_child",
                     "--ignored", "--nocapture", "--test-threads=1"],
                    env=environment, stdin=subprocess.DEVNULL,
                    stdout=self.log, stderr=subprocess.STDOUT)
                if monitor and command["role"] in ("api", "worker"):
                    monitor.add(command["role"], self.process)
                self.channel, _ = listener.accept()
                self.channel.settimeout(120)
                hello = self.receive()
                require(hello.get("token") == token and hello.get("pid") == self.process.pid,
                        "private test-host handshake failed")
                self.send(command)
            except BaseException:
                self.close()
                raise

    def send(self, value):
        body = json.dumps(value, separators=(",", ":")).encode() + b"\n"
        require(len(body) <= 64 * 1024, "private command exceeds limit")
        self.channel.sendall(body)

    def receive(self):
        while b"\n" not in self.buffer:
            chunk = self.channel.recv(4096)
            require(chunk, "private test host closed before returning evidence")
            self.buffer += chunk
            require(len(self.buffer) <= 64 * 1024, "private response exceeds limit")
        line, self.buffer = self.buffer.split(b"\n", 1)
        message = json.loads(line)
        require(isinstance(message, dict), "private response must be an object")
        require("error" not in message, f"private workload failed: {message.get('error')}")
        require(message.get("event") not in ("error", "peer_error"),
                f"private workload failed: {message.get('message')}")
        return message

    def close(self):
        if self.channel:
            self.channel.close()
        if self.process:
            if self.process.poll() is None:
                self.process.kill()
            self.process.wait(timeout=30)
        if self.log:
            self.log.close()

    def wait(self, forced=False):
        if os.name == "nt":
            status = self.process.wait(timeout=30)
            require(forced or status == 0, "private child exited unsuccessfully")
            return
        require(self.process.returncode is None, "private child was reaped without peak evidence")
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            pid, status, usage = os.wait4(self.process.pid, os.WNOHANG)
            if pid:
                self.process.returncode = os.waitstatus_to_exitcode(status)
                self.final_peak_bytes = usage.ru_maxrss * 1024
                require(forced or self.process.returncode == 0, "private child exited unsuccessfully")
                return
            time.sleep(0.01)
        raise TimeoutError("private child did not exit after draining")


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(64 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def source_identity():
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True).strip()
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=normal"], text=True)
    return {"git_head": revision, "worktree_dirty": bool(status)}


def new_run(args, settings):
    executable = Path(args.test_host).resolve(strict=True)
    require(executable.is_file(), "private test host must be a regular file")
    require(platform.system() in ("Windows", "Linux"), "native Windows or Linux is required")
    output = Path(args.output).absolute() if args.output else (
        Path("target") / "physical-scale" / uuid.uuid4().hex).absolute()
    require(".." not in output.parts, "parent traversal is not permitted")
    require(not str(output).startswith("\\\\"), "generated run storage must be local")
    # Leave space for the database, logs, controller evidence and output files.
    require((settings["files"] + (settings["files"] + 63) // 64) * 65536
            + 64 * 1024**2 <= settings["max_run_bytes"],
            "approved run storage cannot admit the initial dataset and reserve")
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "schema": 1, "kind": "physical_scan_and_native_process_memory",
        "status": "incomplete", "profile": settings["profile"],
        "initial_dataset_files": settings["files"],
        "max_run_bytes": settings["max_run_bytes"],
        "baseline_sha256": settings["baseline_sha256"],
        "platform": platform.platform(),
        "test_host_sha256": file_sha256(executable),
        "controller_sha256": file_sha256(Path(__file__)),
        "build": "optimized cfg(test) host; not an unmodified production binary",
        "source": source_identity(),
        "full_size_acceptance": False,
    }
    write_new(output / "ownership.json", {"schema": 1, "run": output.name})
    return executable, output, report


def poll_message(children, timeout=1):
    for role, child in children.items():
        if b"\n" in child.buffer:
            return role, child.receive()
    sockets = {child.channel: role for role, child in children.items()}
    ready, _, _ = select.select(list(sockets), [], [], timeout)
    if not ready:
        return None, None
    role = sockets[ready[0]]
    return role, children[role].receive()


def _same_native_path(left, right):
    paths = [os.fspath(left), os.fspath(right)]
    if os.name == "nt":
        # Python may return an extended-length spelling for a long Windows path.
        paths = [path.removeprefix("\\\\?\\") for path in paths]
    return os.path.normcase(os.path.normpath(paths[0])) == os.path.normcase(
        os.path.normpath(paths[1])
    )


def remove_fixture(output, root):
    owner = json.loads((output / "ownership.json").read_text(encoding="utf-8"))
    require(owner == {"schema": 1, "run": output.name}
            and root.name == "fixture" and root.parent == output
            and _same_native_path(root.resolve(), root.absolute()),
            "generated fixture cleanup ownership does not match")
    target = str(root.absolute())
    if os.name == "nt" and not target.startswith("\\\\?\\"):
        target = "\\\\?\\" + target
    shutil.rmtree(target)


@contextlib.contextmanager
def linux_denied_file(root, approved):
    require(approved is True, "separate Linux permission approval is required")
    require(platform.system() == "Linux" and root.is_absolute(),
            "permission fixture requires an absolute generated Linux root")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    with contextlib.ExitStack() as handles:
        parent = os.open(root.anchor, flags)
        handles.callback(os.close, parent)
        for component in (*root.parts[1:], "media", "catalog", "shard-00000000"):
            parent = os.open(component, flags, dir_fd=parent)
            handles.callback(os.close, parent)
        name = "track-000000000000.mp3"
        file_flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC
        descriptor = os.open(name, file_flags, dir_fd=parent)
        handles.callback(os.close, descriptor)
        before = os.fstat(descriptor)
        require(stat.S_ISREG(before.st_mode), "permission fixture is not a regular file")
        original_mode = stat.S_IMODE(before.st_mode)
        try:
            os.fchmod(descriptor, original_mode & ~0o444)
            try:
                probe = os.open(name, file_flags, dir_fd=parent)
            except PermissionError:
                yield {"actual_os_denial": True, "method": "descriptor_relative_fchmod"}
            else:
                os.close(probe)
                raise ValueError("native Linux denial requires a nonprivileged probe")
        finally:
            os.fchmod(descriptor, original_mode)
            after = os.fstat(descriptor)
            require((after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns,
                     stat.S_IMODE(after.st_mode))
                    == (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns,
                        original_mode),
                    "permission fixture changed identity, bytes, modification time or mode")


def run_profile(args, settings):
    executable, output, report = new_run(args, settings)
    root = output / "fixture"
    children = {}
    events = []
    monitor = Monitor(output, settings)
    memory = None
    failure = None

    def start(role, **fields):
        require(role not in children, "duplicate child role")
        child = Child(executable, {"role": role, "root": str(root), **fields},
                      output, monitor if role in ("api", "worker") else None)
        children[role] = child
        return child

    def event(role, message):
        require(len(events) < 4096, "workload event bound exceeded")
        item = {"role": role, "elapsed_seconds": time.monotonic() - monitor.started,
                "message": message}
        events.append(item)
        return item

    def until(role, expected, watching=None, handler=None):
        watching = watching or {role: children[role]}
        while True:
            monitor.check()
            source, message = poll_message(watching)
            if source is None:
                continue
            event(source, message)
            if handler:
                handler(source, message)
            if source == role and message.get("event") == expected:
                return message

    def one(role, **fields):
        child = start(role, **fields)
        result = until(role, "result")
        child.wait()
        child.close()
        del children[role]
        return result

    def stop(role):
        child = children[role]
        if role in ("api", "worker"):
            monitor.expect_exit(role)
        child.send({"action": "stop"})
        result = until(role, "stopped" if role != "peers" else "result")
        if role in ("api", "worker"):
            monitor.retire(role, child)
        else:
            child.wait()
        child.close()
        del children[role]
        return result

    def worker_start(checkpoint=False):
        start("peers")
        ready = until("peers", "ready")
        start("worker", endpoints=ready["endpoints"], checkpoint=checkpoint)
        until("worker", "ready")

    def worker_stop():
        stop("worker")
        return stop("peers")

    def recover_scan(forced):
        worker_start(checkpoint=True)
        admitted = one("exercise", action="admit")
        progress = until("worker", "scan_progress")
        before = one("inspect")
        scan = before["locations"][0]["scans"][0]
        require(scan["id"] == progress["scan_id"] and scan["pending_directories"] > 0,
                "recovery checkpoint has no durable unfinished frontier")
        monitor.phase = "restart"
        if forced:
            monitor.expect_exit("worker")
            child = children["worker"]
            child.process.kill()
            monitor.retire("worker", child, forced=True)
            child.close()
            del children["worker"]
            stop("peers")
            saved = one("inspect")
            database = (root / "state" / "state.sqlite").as_uri() + "?mode=ro"
            with contextlib.closing(sqlite3.connect(database, uri=True)) as connection:
                row = connection.execute(
                    "SELECT lease_until FROM worker_requests WHERE id=?",
                    (admitted["request_id"],)).fetchone()
            require(row is not None, "interrupted worker request disappeared")
            expiry = max(saved["locations"][0]["scans"][0]["lease_until"],
                         row[0] or 0)
            event("controller", {"event": "waiting_for_real_lease_expiry", "lease_until": expiry})
            monitor.phase = "lease_wait"
            while time.time() <= expiry + 0.25:
                monitor.check()
                time.sleep(min(1, max(0.01, expiry + 0.25 - time.time())))
        else:
            worker_stop()
        stale = one("stale", scan_id=progress["scan_id"], scan_owner=progress["scan_owner"])
        worker_start()
        monitor.phase = "recovery"
        resumed = one("exercise", action="resume", request_id=admitted["request_id"])
        after = resumed["inspection"]["locations"][0]
        require(resumed["phases"][0]["request"]["state"] == "complete"
                and after["scans"][0]["id"] == progress["scan_id"]
                and after["present_files"] == settings["files"]
                and after["location"]["coverage"] == "complete",
                "restart did not resume the same scan to complete coverage")
        verification = one("verify")
        if forced:
            monitor.phase = "post_workload"
            end = time.monotonic() + 2
            while time.monotonic() < end:
                monitor.check()
                time.sleep(0.1)
        worker_stop()
        return {"forced_exit": forced, "scan_id": progress["scan_id"],
                "request_id": admitted["request_id"],
                "completed_directories_before": scan["complete_directories"],
                "stale_claim": stale, "resumed": resumed,
                "unchanged_files": verification["verified_files"]}

    def unavailable_root():
        worker_start()
        location = report["preparation"]["fixture"]["unavailable_location_id"]
        one("exercise", action="scan", location_id=location)
        worker_stop()
        one("fault", action="unavailable")
        worker_start()
        admitted = one("exercise", action="admit", location_id=location)
        failed = one("exercise", action="resume", location_id=location,
                     request_id=admitted["request_id"])
        cached = failed["inspection"]["locations"][1]
        require(failed["phases"][0]["request"]["state"] == "failed"
                and cached["present_files"] == 1
                and cached["location"]["coverage"] == "incomplete"
                and cached["location"]["last_error"],
                "unavailable root did not preserve cached presence and a durable error")
        worker_stop()
        one("fault", action="restore")
        worker_start()
        restored = one("exercise", action="retry", location_id=location,
                       request_id=admitted["request_id"])
        require(restored["phases"][0]["request"]["state"] == "complete"
                and restored["inspection"]["locations"][1]["location"]["coverage"] == "complete",
                "explicit root restoration and retry did not complete")
        worker_stop()
        return {"failed": failed, "restored": restored}

    try:
        prepare = {
            "file_count": settings["files"], "max_run_bytes": settings["max_run_bytes"],
            "profile": "small" if settings["profile"] == "small" else "azure_full",
            "integrated": True,
        }
        if settings["profile"] == "full":
            prepare.update(
                azure_admitted=True, baseline_regular_files=settings["files"] // 2,
                manifest_sha256=settings["baseline_sha256"])
        report["preparation"] = one("prepare", **prepare)
        require(storage(output, settings.get("max_generated_files"))
                < settings["max_run_bytes"], "prepared fixture exceeds storage admission")
        monitor.start()
        start("api")
        until("api", "ready")
        worker_start(checkpoint=True)
        start("integrated")
        monitor.phase = "workload"
        download_seen = False
        checkpoint_seen = False
        checkpoint_released = False
        listing_seen = False

        def integration_events(role, message):
            nonlocal download_seen, checkpoint_seen, checkpoint_released, listing_seen
            if role == "peers" and message.get("event") == "download_started":
                download_seen = True
            if role == "worker" and message.get("event") == "scan_progress":
                checkpoint_seen = True
            if role == "peers" and message.get("event") == "listing_complete":
                listing_seen = True
            if checkpoint_seen and download_seen and not checkpoint_released:
                children["worker"].send({"action": "release"})
                event("controller", {"event": "scan_released_during_download"})
                checkpoint_released = True
            if role == "integrated" and message.get("event") == "integration_ready":
                children["peers"].send({"action": "start"})

        report["integration"] = until(
            "integrated", "result",
            {name: children[name] for name in ("integrated", "peers", "worker")},
            integration_events)
        children["integrated"].wait()
        children["integrated"].close()
        del children["integrated"]
        children["peers"].send({"action": "list"})
        if not listing_seen:
            until("peers", "listing_complete",
                  {name: children[name] for name in ("peers", "worker")}, integration_events)
        report["repeat_scan"] = one("exercise", action="scan")
        monitor.phase = "verification"
        report["verification"] = one("verify")
        monitor.phase = "draining"
        report["peer_evidence"] = worker_stop()
        peers = report["peer_evidence"]
        expected_shared_files = settings["files"] - (settings["files"] + 12) // 16 + 1
        require(peers["account_logins"] == 1, "integration did not use one account login")
        require(peers["download_bytes"] > 0 and peers["upload_bytes"] > 0
                and peers["listed_files"] == expected_shared_files and not peers["peer_task_errors"],
                "integration transfer/listing evidence is incomplete")
        require(peers["download_upload_overlap"], "download and upload did not overlap")
        ready_time = next(
            item["elapsed_seconds"] for item in events
            if item["message"].get("event") == "integration_ready")
        release_time = next(
            item["elapsed_seconds"] for item in events
            if item["message"].get("event") == "scan_released_during_download")
        first_end = min(
            item["elapsed_seconds"] for item in events
            if item["message"].get("event") == "download_complete")
        report["scan_download_overlap"] = release_time < min(ready_time, first_end)
        require(report["scan_download_overlap"], "physical scan and download did not overlap")
        monitor.phase = "faults"
        if os.name == "nt" or settings.get("linux_permission_fixture_approved"):
            worker_start()
            with contextlib.ExitStack() as denial:
                if os.name == "nt":
                    start("lock")
                    locked = until("lock", "ready")
                    denial.callback(stop, "lock")
                else:
                    locked = denial.enter_context(linux_denied_file(root, True))
                denied = one("exercise", action="scan")
                before_issues = report["verification"]["locations"][0]["metadata_statuses"].get("unreadable", 0)
                issues = denied["inspection"]["locations"][0]["metadata_statuses"]["unreadable"]
                require(locked["actual_os_denial"] and issues == before_issues + 1,
                        "native unreadable fixture did not produce its extra diagnostic")
                report["native_denial"] = locked
            one("exercise", action="scan")
            worker_stop()
            report["actual_os_denial"] = True
        else:
            report["actual_os_denial"] = False
            report["os_denial_limit"] = "Linux permission fixture requires separate guest approval"
        report["unavailable_root"] = unavailable_root()
        report["graceful_recovery"] = recover_scan(False)
        report["forced_recovery"] = recover_scan(True)
        report["final_verification"] = one("verify")
        monitor.phase = "draining"
        stop("api")
        starts = [item["elapsed_seconds"] for item in events
                  if item["message"].get("event") == "download_started"]
        ends = [item["elapsed_seconds"] for item in events
                if item["message"].get("event") == "download_complete"]
        require(len(starts) == 3 and len(ends) == 3, "download phase timing is incomplete")
        memory = monitor.finish(True, (min(starts) + 1, max(ends)))
        monitor.check()
        require(memory["verdict"] == "passed_profile_only",
                "combined resident-memory evidence did not pass this profile")
        report["status"] = "evidence_collected"
        report["remaining_acceptance"] = [
            "native full-size Windows and Linux Azure runs",
            "full source byte-size and product 500000-file traversal are unverified",
        ]
    except (OSError, ValueError, RuntimeError, KeyError, TypeError, StopIteration,
            sqlite3.Error, subprocess.SubprocessError) as error:
        failure = f"{type(error).__name__}: {error}"
        report["status"] = "failed"
        report["error"] = failure
    finally:
        if memory is None:
            memory = monitor.finish(False)
        report["memory"] = memory
        report["events"] = events
        for child in list(children.values()):
            child.close()
        try:
            report["final_run_storage_bytes"] = storage(output, settings.get("max_generated_files"))
            require(report["final_run_storage_bytes"] <= settings["max_run_bytes"],
                    "final generated run storage exceeds admission")
        except (OSError, ValueError) as error:
            failure = f"final storage measurement failed: {error}"
            report["status"] = "failed"
            report["storage_error"] = failure
        if root.exists():
            try:
                remove_fixture(output, root)
                report["fixture_removed"] = True
            except (OSError, ValueError) as error:
                failure = f"generated fixture cleanup failed: {error}"
                report["status"] = "failed"
                report["cleanup_error"] = failure
        write_new(output / "report.json", report)
        print(json.dumps({"status": report["status"], "report": str(output / "report.json"),
                          "memory": memory["verdict"], "full_size_acceptance": False}))
    return 1 if failure else 0


def main(arguments):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    inventory = commands.add_parser("inventory")
    inventory.add_argument("--root", required=True)
    inventory.add_argument("--output", required=True)
    runner = commands.add_parser("run")
    runner.add_argument("--test-host", required=True)
    runner.add_argument("--profile", choices=("small", "full"), default="small")
    runner.add_argument("--output")
    runner.add_argument("--manifest")
    runner.add_argument("--admission")
    args = parser.parse_args(arguments)
    if args.action == "run":
        settings = admission(args.profile, args.manifest, args.admission)
        if settings["profile"] == "full":
            settings["azure_host"] = verify_azure_vm(settings["vm_id"])
        return run_profile(args, settings)
    if args.action == "inventory":
        started = time.monotonic()
        result = Inventory()
        root = Path(args.root)
        output = Path(args.output)
        require(".." not in root.parts, "parent traversal is not permitted")
        require(output.is_absolute(), "inventory output must be an absolute private path")
        require(not output.resolve().is_relative_to(root.resolve()),
                "inventory output cannot be inside the read-only source")
        try:
            if os.name == "nt":
                Windows().inventory(root, result)
            else:
                inventory_posix(root, result)
        except (OSError, ValueError) as error:
            result.error(error)
        report = result.report()
        report["elapsed_seconds"] = time.monotonic() - started
        write_new(args.output, report)
        print(json.dumps(report, sort_keys=True))
        return 0 if report["complete"] else 1
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except (OSError, ValueError) as failure:
        print(f"Scale evidence failed: {failure}", file=sys.stderr)
        sys.exit(1)
