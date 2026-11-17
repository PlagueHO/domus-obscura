"""Generated Samba 4.17.12 experiment, not a production filesystem adapter.

That version's CREATE_OR_GET_OBJECT_ID hook returns native fields without
creating an object ID. Never send this operation to an arbitrary server.
"""

import contextlib
import errno
import hashlib
import json
import os
import re
import select
import struct
import sys
import time
import unittest
import uuid


ROOTS = {
    "media": "/fixture/payload",
    "media-alias": "/fixture/payload",
    "staging": "/fixture/payload/staging",
    "private-alias": "/fixture/private",
}
PROBE_CONTROL = 0x00096404
PROBE_REQUEST = struct.Struct("<4sHH16s16sQHQQ")
PROBE_RESPONSE = struct.Struct("<4sHHII16s16sQQQQQH6s")


def decode_probe(data, revision, challenge, sequence):
    if not PROBE_RESPONSE.size <= len(data) <= PROBE_RESPONSE.size + 32:
        raise ValueError("invalid native-parent response length")
    (
        magic, version, reserved, result, error, actual_revision,
        actual_challenge, actual_sequence, parent_device, parent_inode,
        child_device, child_inode, marker_length, padding,
    ) = PROBE_RESPONSE.unpack_from(data)
    if (
        magic != b"DRO1" or version != 1 or reserved or padding != bytes(6)
        or actual_revision != revision or actual_challenge != challenge
        or actual_sequence != sequence or result not in (0, 1)
        or len(data) != PROBE_RESPONSE.size + marker_length
        or (result == 0 and (error or not all(
            (parent_device, parent_inode, child_device, child_inode)
        )))
        or (result == 1 and (not error or marker_length))
    ):
        raise ValueError("native-parent response did not match its request or result")
    return {
        "result": "native_refusal" if result else "native_child_read",
        "errno": error,
        "parent_native_identity": (
            f"unix:{parent_device:016x}:{parent_inode:016x}"
            if parent_device and parent_inode else None
        ),
        "child_native_identity": (
            f"unix:{child_device:016x}:{child_inode:016x}"
            if child_device and child_inode else None
        ),
        "marker": None if result else data[PROBE_RESPONSE.size:].decode("ascii"),
        "sequence": sequence,
    }


def decode_native(data):
    if len(data) != 64:
        raise ValueError("expected the exact Samba 64-byte response")
    if data[:16] != data[32:48] or data[48:] != bytes(16):
        raise ValueError("inconsistent Samba native identity fields")
    device, inode = struct.unpack("<QQ", data[:16])
    if not device or not inode:
        raise ValueError("missing native identity")
    return f"unix:{device:016x}:{inode:016x}"


def emit(record):
    print(json.dumps(record, sort_keys=True), flush=True)


def commands():
    deadline = time.monotonic() + 60
    pending = bytearray()
    while time.monotonic() < deadline:
        ready, _, _ = select.select(
            [sys.stdin], [], [], max(0, deadline - time.monotonic())
        )
        if not ready:
            raise TimeoutError("generated witness context expired")
        data = os.read(sys.stdin.fileno(), 129)
        if not data:
            return
        pending.extend(data)
        if len(pending) > 128:
            raise ValueError("generated command exceeded its bound")
        if b"\n" in pending:
            command, remainder = pending.split(b"\n", 1)
            if remainder:
                raise ValueError("only one generated command may be outstanding")
            pending.clear()
            yield command.decode("ascii").strip()


@contextlib.contextmanager
def open_native(path):
    with contextlib.ExitStack() as stack:
        flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
        handle = os.open("/", flags)
        stack.callback(os.close, handle)
        metadata = os.fstat(handle)
        chain = [(metadata.st_dev, metadata.st_ino)]
        for component in path.split("/")[1:]:
            if component in ("", ".", ".."):
                raise ValueError("invalid generated component")
            handle = os.open(component, flags, dir_fd=handle)
            stack.callback(os.close, handle)
            metadata = os.fstat(handle)
            chain.append((metadata.st_dev, metadata.st_ino))
        yield handle, chain


def native_record(path, retained, chain):
    import fcntl

    metadata = os.fstat(retained)
    filesystem = bytearray(17)
    # Linux FS_IOC_GETFSUUID reads the retained inode's superblock UUID.
    fcntl.ioctl(retained, 0x80111500, filesystem, True)
    if filesystem[0] != 16 or not any(filesystem[1:]):
        raise ValueError("missing full native filesystem UUID")
    attributes = [
        (name, hashlib.sha256(os.getxattr(retained, name)).hexdigest())
        for name in sorted(os.listxattr(retained))
    ]
    state = (metadata.st_mtime_ns, metadata.st_ctime_ns, attributes)
    with open_native(path) as (_, current_chain):
        matches = current_chain == chain
    sentinel = None
    if path in (ROOTS["staging"], ROOTS["private-alias"]):
        descriptor = os.open(
            "sentinel", os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=retained
        )
        try:
            child = os.fstat(descriptor)
            sentinel = f"unix:{child.st_dev:016x}:{child.st_ino:016x}"
        finally:
            os.close(descriptor)
    return {
        "native_identity": f"unix:{metadata.st_dev:016x}:{metadata.st_ino:016x}",
        "filesystem_uuid": filesystem[1:].hex(),
        "ancestry": [f"unix:{device:016x}:{inode:016x}" for device, inode in chain],
        "path_binding_matches": matches,
        "metadata_digest": hashlib.sha256(json.dumps(state).encode()).hexdigest(),
        "sentinel_identity": sentinel,
    }


def native_context():
    with contextlib.ExitStack() as stack:
        roots = {
            role: stack.enter_context(open_native(path))
            for role, path in ROOTS.items()
        }

        def observe():
            return {
                role: native_record(ROOTS[role], handle, chain)
                for role, (handle, chain) in roots.items()
            }

        emit({"phase": "native_opened", "roots": observe()})
        for command in commands():
            if command == "observe":
                emit({"phase": "native_observed", "roots": observe()})
            elif command == "exit":
                return
            else:
                raise ValueError("unsupported generated native command")


def cifs_parent_context():
    with open_native("/mounts/media/handle-parent") as (parent, _):
        def observe():
            with contextlib.ExitStack() as stack:
                child = os.open(
                    "sentinel", os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent
                )
                stack.callback(os.close, child)
                data = os.read(child, 33)
                if len(data) > 32:
                    raise ValueError("generated marker exceeded its bound")
                return {
                    "parent_inode": os.fstat(parent).st_ino,
                    "child_inode": os.fstat(child).st_ino,
                    "marker": data.decode("ascii"),
                }

        emit(observe())
        for command in commands():
            if command == "observe":
                try:
                    emit(observe())
                except OSError as error:
                    if error.errno not in (
                        errno.ESTALE, errno.ENOENT, errno.EACCES, errno.ENOTDIR
                    ):
                        raise
                    emit({"rejected": True, "errno": error.errno})
            elif command == "exit":
                return
            else:
                raise ValueError("unsupported generated CIFS command")


def remote_context(server_hook=False):
    from smbprotocol import Dialects
    from smbprotocol.connection import Connection
    from smbprotocol.exceptions import SMBResponseException
    from smbprotocol.header import NtStatus
    from smbprotocol.ioctl import CtlCode, IOCTLFlags, SMB2IOCTLRequest, SMB2IOCTLResponse
    from smbprotocol.open import (
        CreateDisposition,
        CreateOptions,
        DirectoryAccessMask,
        FileAttributes,
        ImpersonationLevel,
        Open,
        ShareAccess,
    )
    from smbprotocol.session import Session
    from smbprotocol.tree import TreeConnect

    class ProbeCtlCode:
        DOMUS_NATIVE_READ = PROBE_CONTROL

    connection = Connection(uuid.uuid4(), "storage-server", 445, require_signing=False)
    connection.connect(dialect=Dialects.SMB_3_1_1, timeout=3)
    try:
        # These deliberately unknown, generated credentials select the lab's
        # guest mapping. No host credential store or production account is used.
        session = Session(
            connection,
            username=f"domus-fixture-{uuid.uuid4().hex}",
            password=uuid.uuid4().hex,
            require_encryption=False,
            auth_protocol="ntlm",
        )
        session.connect()
        roots = {}
        for role in ROOTS:
            tree = TreeConnect(session, rf"\\storage-server\{role}")
            tree.connect(require_secure_negotiate=False)
            opened = Open(tree, "")
            opened.create(
                ImpersonationLevel.Impersonation,
                DirectoryAccessMask.FILE_READ_ATTRIBUTES | (
                    DirectoryAccessMask.FILE_LIST_DIRECTORY if server_hook else 0
                ),
                FileAttributes.FILE_ATTRIBUTE_DIRECTORY,
                ShareAccess.FILE_SHARE_READ
                | ShareAccess.FILE_SHARE_WRITE
                | ShareAccess.FILE_SHARE_DELETE,
                CreateDisposition.FILE_OPEN,
                CreateOptions.FILE_DIRECTORY_FILE | CreateOptions.FILE_OPEN_REPARSE_POINT,
            )
            roots[role] = (tree, opened)

        def receive_control(request, tree, file_id, control):
            response = connection.receive(request, timeout=10 if server_hook else 3)
            result = SMB2IOCTLResponse()
            if control == PROBE_CONTROL:
                result["ctl_code"].enum_type = ProbeCtlCode
            remaining = result.unpack(response["data"].get_value())
            if (
                remaining
                or result["file_id"].get_value() != file_id
                or result["ctl_code"].get_value() != control
                or response["session_id"].get_value() != session.session_id
                or response["tree_id"].get_value() != tree.tree_connect_id
            ):
                raise ValueError("response did not match the existing remote open")
            return result["buffer"].get_value()

        sequences = {}
        previous_requests = {}
        observed_children = {}
        revision = bytes.fromhex(os.environ["DOMUS_SMB_LAB_RUN"])

        def query_hook(tree, file_id, name=b"sentinel", action=1,
                       malformed=None, advance=True, announce=False):
            sequence = sequences.get(tree.tree_connect_id, 0) + 1
            challenge = uuid.uuid4().bytes
            observed = observed_children.get((tree.tree_connect_id, name), (0, 0))
            body = PROBE_REQUEST.pack(
                b"DRP1", 1, action, revision, challenge, sequence, len(name), *observed
            ) + name
            if malformed is not None:
                body = malformed(body)
            packet = SMB2IOCTLRequest()
            packet["ctl_code"].enum_type = ProbeCtlCode
            packet["ctl_code"] = PROBE_CONTROL
            packet["file_id"] = file_id
            packet["flags"] = IOCTLFlags.SMB2_0_IOCTL_IS_FSCTL
            packet["max_output_response"] = 128
            packet["buffer"] = body
            request = connection.send(
                packet, sid=session.session_id, tid=tree.tree_connect_id
            )
            if advance:
                sequences[tree.tree_connect_id] = sequence
                previous_requests[tree.tree_connect_id] = body
            if announce:
                emit({"phase": "native_parent_request_submitted"})
            result = decode_probe(
                receive_control(request, tree, file_id, PROBE_CONTROL),
                revision, challenge, sequence,
            )
            if result["result"] == "native_child_read":
                observed_children[(tree.tree_connect_id, name)] = tuple(
                    int(field, 16) for field in result["child_native_identity"].split(":")[1:]
                )
            return result

        def query(tree, file_id):
            packet = SMB2IOCTLRequest()
            packet["ctl_code"] = CtlCode.FSCTL_CREATE_OR_GET_OBJECT_ID
            packet["file_id"] = file_id
            packet["flags"] = IOCTLFlags.SMB2_0_IOCTL_IS_FSCTL
            packet["max_output_response"] = 64
            request = connection.send(
                packet, sid=session.session_id, tid=tree.tree_connect_id
            )
            data = receive_control(request, tree, file_id, packet["ctl_code"].get_value())
            return {
                "native_identity": decode_native(data),
                "remote_open": file_id.hex(),
                "export_object_id": data[16:32].hex(),
            }

        def observe():
            return {
                role: (
                    query_hook(tree, opened.file_id) if server_hook
                    else query(tree, opened.file_id)
                )
                for role, (tree, opened) in roots.items()
            }

        def require_rejection(tree, file_id):
            try:
                if server_hook:
                    query_hook(tree, file_id, advance=False)
                else:
                    query(tree, file_id)
            except SMBResponseException as error:
                if error.status not in (
                    NtStatus.STATUS_INVALID_HANDLE,
                    NtStatus.STATUS_FILE_CLOSED,
                    NtStatus.STATUS_ACCESS_DENIED,
                ):
                    raise
                return {"rejected": True, "status": f"0x{error.status:08x}"}
            raise ValueError("server accepted an invalid remote-open association")

        def malformed_requests():
            tree, opened = roots["private-alias"]
            cases = [
                (b"", None), (b".", None), (b"..", None),
                (b"../sentinel", None), (b"folder\\sentinel", None),
                (b"sentinel:stream", None), (b"\xff", None),
                (b"a" * 256, None), (b"sentinel", lambda data: data[:-1]),
                (b"sentinel", lambda data: data + bytes(4096)),
                (b"sentinel", lambda data: data[:4] + b"\x02\x00" + data[6:]),
                (b"sentinel", lambda data: data[:8] + bytes(16) + data[24:]),
                (b"sentinel", lambda data: data[:40] + bytes(8) + data[48:]),
                (b"sentinel", lambda data: previous_requests[tree.tree_connect_id]),
            ]
            for name, malformed in cases:
                try:
                    query_hook(tree, opened.file_id, name, malformed=malformed, advance=False)
                except SMBResponseException as error:
                    if error.status != NtStatus.STATUS_INVALID_PARAMETER:
                        raise
                else:
                    raise ValueError("server accepted a malformed native-parent request")
            return {"rejected": len(cases), "valid_after": query_hook(tree, opened.file_id)}

        def metadata_only():
            tree, _ = roots["private-alias"]
            opened = Open(tree, "")
            opened.create(
                ImpersonationLevel.Impersonation, DirectoryAccessMask.FILE_READ_ATTRIBUTES,
                FileAttributes.FILE_ATTRIBUTE_DIRECTORY, ShareAccess.FILE_SHARE_READ
                | ShareAccess.FILE_SHARE_WRITE | ShareAccess.FILE_SHARE_DELETE,
                CreateDisposition.FILE_OPEN, CreateOptions.FILE_DIRECTORY_FILE
                | CreateOptions.FILE_OPEN_REPARSE_POINT,
            )
            try:
                try:
                    query_hook(tree, opened.file_id)
                except SMBResponseException as error:
                    if error.status != NtStatus.STATUS_ACCESS_DENIED:
                        raise
                    return {"rejected": True, "status": f"0x{error.status:08x}"}
                raise ValueError("metadata-only access authorized a native child read")
            finally:
                opened.close()

        emit({
            "phase": "server_hook_opened" if server_hook else "remote_opened",
            "roots": observe(),
        })
        for command in commands():
            if command == "observe":
                emit({"phase": "remote_observed", "roots": observe()})
            elif server_hook and command == "malformed":
                emit(malformed_requests())
            elif server_hook and command == "metadata-only":
                emit(metadata_only())
            elif server_hook and command == "unrelated-parent":
                tree, _ = roots["media"]
                opened = Open(tree, "staging")
                opened.create(
                    ImpersonationLevel.Impersonation,
                    DirectoryAccessMask.FILE_READ_ATTRIBUTES | DirectoryAccessMask.FILE_LIST_DIRECTORY,
                    FileAttributes.FILE_ATTRIBUTE_DIRECTORY,
                    ShareAccess.FILE_SHARE_READ | ShareAccess.FILE_SHARE_WRITE | ShareAccess.FILE_SHARE_DELETE,
                    CreateDisposition.FILE_OPEN,
                    CreateOptions.FILE_DIRECTORY_FILE | CreateOptions.FILE_OPEN_REPARSE_POINT,
                )
                try:
                    emit(query_hook(tree, opened.file_id))
                finally:
                    opened.close()
            elif server_hook and command == "file-fixtures":
                tree, opened = roots["private-alias"]
                emit({
                    name: query_hook(tree, opened.file_id, name.encode("ascii"))
                    for name in ("denied", "link", "fifo", "large")
                })
            elif server_hook and command == "mount-case":
                tree, opened = roots["private-alias"]
                emit(query_hook(tree, opened.file_id, b"mounted"))
            elif server_hook and command == "hook-pause":
                tree, opened = roots["private-alias"]
                emit(query_hook(tree, opened.file_id, action=2, announce=True))
            elif command == "foreign-tree":
                emit(require_rejection(roots["media"][0], roots["private-alias"][1].file_id))
            elif command == "closed":
                tree, opened = roots["private-alias"]
                file_id = opened.file_id
                opened.close()
                emit(require_rejection(tree, file_id))
            elif command == "exit":
                return
            else:
                raise ValueError("unsupported generated remote command")
    finally:
        connection.disconnect(True)


class DecoderTests(unittest.TestCase):
    def test_preserves_both_full_native_fields(self):
        identity = struct.pack("<QQ", 0x123456789ABCDEF0, 0xFEDCBA9876543210)
        data = identity + bytes(16) + identity + bytes(16)
        self.assertEqual(
            decode_native(data), "unix:123456789abcdef0:fedcba9876543210"
        )

    def test_rejects_truncated_conflicting_and_missing_evidence(self):
        identity = struct.pack("<QQ", 1, 2)
        data = identity + bytes(16) + identity + bytes(16)
        for invalid in (
            data[:-1],
            data + b"\0",
            data[:32] + bytes(32),
            bytes(64),
        ):
            with self.subTest(length=len(invalid)):
                with self.assertRaises(ValueError):
                    decode_native(invalid)

    def test_native_parent_response_requires_exact_request_and_complete_result(self):
        revision, challenge = bytes(range(16)), bytes(range(16, 32))
        header = PROBE_RESPONSE.pack(
            b"DRO1", 1, 0, 0, 0, revision, challenge, 7,
            0x123456789ABCDEF0, 2, 3, 0xFEDCBA9876543210, 2, bytes(6),
        )
        response = header + b"ok"
        result = decode_probe(response, revision, challenge, 7)
        self.assertEqual(result["marker"], "ok")
        self.assertEqual(result["parent_native_identity"], "unix:123456789abcdef0:0000000000000002")
        self.assertEqual(result["child_native_identity"], "unix:0000000000000003:fedcba9876543210")
        for invalid, expected_revision, expected_challenge, expected_sequence in (
            (response[:-1], revision, challenge, 7),
            (response + b"x", revision, challenge, 7),
            (response, bytes(16), challenge, 7),
            (response, revision, bytes(16), 7),
            (response, revision, challenge, 8),
        ):
            with self.subTest(sequence=expected_sequence, length=len(invalid)):
                with self.assertRaises(ValueError):
                    decode_probe(invalid, expected_revision, expected_challenge, expected_sequence)

    def test_native_parent_refusal_cannot_contain_marker_or_missing_error(self):
        revision, challenge = bytes(16), bytes(range(16))
        refusal = PROBE_RESPONSE.pack(
            b"DRO1", 1, 0, 1, errno.ESTALE, revision, challenge,
            1, 2, 3, 4, 5, 0, bytes(6),
        )
        result = decode_probe(refusal, revision, challenge, 1)
        self.assertEqual(result["result"], "native_refusal")
        self.assertIsNone(result["marker"])
        for invalid in (
            refusal + b"x",
            refusal[:12] + bytes(4) + refusal[16:],
            refusal[:88] + b"\x01\x00" + refusal[90:] + b"x",
        ):
            with self.assertRaises(ValueError):
                decode_probe(invalid, revision, challenge, 1)


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        unittest.main(argv=[sys.argv[0]])
    else:
        if not re.fullmatch(r"[a-f0-9]{32}", os.environ.get("DOMUS_SMB_LAB_RUN", "")):
            raise ValueError("this collector requires the generated lab harness")
        if sys.argv[1:] == ["--native"]:
            native_context()
        elif sys.argv[1:] == ["--remote"]:
            remote_context()
        elif sys.argv[1:] == ["--cifs-parent"]:
            cifs_parent_context()
        elif sys.argv[1:] == ["--server-hook"]:
            remote_context(server_hook=True)
        else:
            raise ValueError("select one generated witness context")
