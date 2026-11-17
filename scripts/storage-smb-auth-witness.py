"""Isolated authenticated Samba fixture; never use live accounts or paths."""

import hashlib
import errno
import importlib.util
import json
import os
import re
import stat
import struct
import subprocess
import sys
import time
import uuid

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

REQUEST = struct.Struct("<4sHH16s32s32s32sIQ16s")
CONTROL = 0x00096408
# smbprotocol 1.17.0 omits NT_STATUS_FILE_INVALID.
FILE_INVALID = 0xC0000098
BODY = 840
PURPOSE = b"domus.native-witness.v2"
ROLES = ("media", "media-alias", "staging", "private-alias")
READ_BOUND = 1024 * 1024
READ_IDENTITY = struct.Struct("<7Q")


def setup_read_files():
    payload = hashlib.shake_256(b"domus-read-proof-v1").digest(READ_BOUND + 1)
    for name, length in (("read-empty", 0), ("read-tail", 32785),
                         ("read-full", READ_BOUND), ("read-over", READ_BOUND + 1),
                         ("read-denied", 8), ("read-hard-source", 8)):
        path = f"/fixture/payload/{name}"
        generated_file(path, payload[:length])
        os.chmod(path, 0 if name == "read-denied" else 0o644)
    os.link("/fixture/payload/read-hard-source", "/fixture/payload/read-hard-link")
    os.symlink("read-full", "/fixture/payload/read-link")
    os.mkfifo("/fixture/payload/read-fifo", 0o644)
    for directory in ("/fixture/payload/staging", "/fixture/private"):
        for index in range(127):
            path = f"{directory}/read-entry-{index:03}"
            generated_file(path, b"")
            os.chmod(path, 0o644)
    print("Generated bounded read-only operation fixtures.")


def generated_file(path, data):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)

def protected_client_file(name, bound):
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    root = os.open("/", flags)
    try:
        secrets = os.open("secrets", flags, dir_fd=root)
        try:
            directory = os.open("client", flags, dir_fd=secrets)
            try:
                identity = os.fstat(directory)
                if identity.st_uid != 0 or stat.S_IMODE(identity.st_mode) != 0o700:
                    raise ValueError("generated client secret directory is not protected")
                descriptor = os.open(
                    name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK,
                    dir_fd=directory,
                )
                with os.fdopen(descriptor, "rb") as stream:
                    identity = os.fstat(stream.fileno())
                    if (not stat.S_ISREG(identity.st_mode) or identity.st_uid != 0
                            or stat.S_IMODE(identity.st_mode) != 0o600
                            or identity.st_nlink != 1 or not 0 < identity.st_size <= bound):
                        raise ValueError("generated client secret file is not protected")
                    data = stream.read(bound + 1)
                    if len(data) != identity.st_size:
                        raise ValueError("generated client secret length changed")
                    return data
            finally:
                os.close(directory)
        finally:
            os.close(secrets)
    finally:
        os.close(root)


def setup():
    for directory in ("/secrets/witness", "/secrets/client-a", "/secrets/client-b"):
        os.chmod(directory, 0o700)
    key = Ed25519PrivateKey.generate()
    generated_file("/secrets/witness/key.pem", key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption(),
    ))
    pin = key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw,
    )
    with open("/etc/samba/smb.conf", "rb") as stream:
        configuration_bytes = stream.read(16384)
    if len(configuration_bytes) >= 16384:
        raise ValueError("generated server configuration exceeds its bound")
    configuration = configuration_bytes.decode("utf-8")
    configuration = configuration.replace(
        "[global]", "[global]\n    private dir = /secrets/witness\n"
        "    lock directory = /run/samba\n    state directory = /var/lib/samba\n"
        "    cache directory = /var/cache/samba\n",
        1,
    )
    generated_file("/run/samba/auth-setup.conf", configuration.encode())
    for suffix in ("a", "b"):
        name = f"domus_{suffix}"
        password = uuid.uuid4().hex + uuid.uuid4().hex
        command = ["smbpasswd", "-c", "/run/samba/auth-setup.conf", "-s", "-a", name]
        subprocess.run(command, input=f"{password}\n{password}\n".encode(),
                       check=True, stdout=subprocess.DEVNULL)
        generated_file(f"/secrets/client-{suffix}/credential", f"{name}\n{password}\n".encode())
        generated_file(f"/secrets/client-{suffix}/public.key", pin)
        generated_file(
            f"/secrets/client-{suffix}/server-config.sha256",
            hashlib.sha256(configuration_bytes).digest(),
        )
    print(json.dumps({"phase": "generated_credentials_ready", "public_pin": pin.hex()}))


def decode(data, request, pin, uid, sid, tid, file_id, server_config):
    if len(request) < REQUEST.size:
        raise ValueError("authenticated request is truncated")
    action = struct.unpack_from("<H", request, 6)[0]
    if action not in (0, 1, 2, 3) or (action == 0 and len(request) != REQUEST.size):
        raise ValueError("authenticated request operation differs")
    if (len(data) < BODY + 64 or len(data) > BODY + 49152 + 64
            or (not action and len(data) != BODY + 64)):
        raise ValueError("authenticated witness response length differs")
    body_length = len(data) - 64
    Ed25519PublicKey.from_public_bytes(pin).verify(data[body_length:], PURPOSE + data[:body_length])
    if (data[:8] != b"DWS2\x02\x00\x00\x00" or data[16:164] != request[:REQUEST.size]
            or not any(data[164:180]) or not any(data[180:196])
            or struct.unpack_from("<IQI", data, 196) != (uid, sid, tid)
            or data[212:228] != file_id or data[244:276] != server_config
            or (not action and any(data[740:BODY]))
            or (action and (data[740:772] != hashlib.sha256(request).digest()
                            or struct.unpack_from("<I", data, 772)[0] != body_length - BODY
                            or any(data[776:BODY])))):
        raise ValueError("signed response does not match the actual request/open")
    result, error = struct.unpack_from("<II", data, 8)
    if result not in (0, 1) or (result == 0 and error) or (result == 1 and not error):
        raise ValueError("invalid signed result")
    mask = struct.unpack_from("<I", request, 120)[0]
    roots = {}
    matrix = list(data[276:292])
    if result:
        if any(data[276:740]) or body_length != BODY:
            raise ValueError("refused signed result contains native evidence")
    else:
        parent = struct.unpack_from("<QQ", data, 228)
        for index, role in enumerate(ROLES):
            record = data[292 + index * 112:404 + index * 112]
            if not mask & (1 << index):
                if any(record):
                    raise ValueError("signed reply disclosed an unauthorized role")
                continue
            device, inode = struct.unpack_from("<QQ", record)
            mount, count, reserved = struct.unpack_from("<QII", record, 32)
            if not device or not inode or not any(record[16:32]) or not mount or not 1 <= count <= 4 or reserved:
                raise ValueError("signed native evidence is incomplete")
            chain = [struct.unpack_from("<QQ", record, 48 + part * 16) for part in range(count)]
            if chain[-1] != (device, inode) or any(record[48 + count * 16:]):
                raise ValueError("signed ancestry is incomplete or malformed")
            roots[role] = {
                "identity": f"unix:{device:016x}:{inode:016x}",
                "filesystem_uuid": record[16:32].hex(),
                "ancestry": [f"unix:{dev:016x}:{ino:016x}" for dev, ino in chain],
            }
        if parent not in [struct.unpack_from("<QQ", data, 292 + part * 112)
                          for part in range(4) if mask & (1 << part)]:
            raise ValueError("signed parent is outside the requested roles")
        for left in range(4):
            for right in range(4):
                value = matrix[left * 4 + right]
                included = mask & (1 << left) and mask & (1 << right)
                if (included and value not in range(4)) or (not included and value != 255):
                    raise ValueError("signed relationship matrix is incomplete")
                if included and (matrix[right * 4 + left] != value or (left == right and value != 0)):
                    raise ValueError("signed relationship matrix is inconsistent")
    result = {
        "result": "signed_native_refusal" if result else "signed_native_evidence",
        "errno": error, "roots": roots, "relationships": matrix,
        "context": data[164:180].hex(), "instance": data[180:196].hex(),
        "server_config": data[244:276].hex(),
        "parent": "unix:%016x:%016x" % struct.unpack_from("<QQ", data, 228),
    }
    if action:
        result["operation_data"] = data[BODY:body_length]
    return result


def authentication_rejections(name, password):
    from smbprotocol import Dialects
    from smbprotocol.connection import Connection
    from smbprotocol.exceptions import SMBResponseException
    from smbprotocol.header import NtStatus
    from smbprotocol.session import Session

    results = []
    for label, user, secret, compatibility, dialect in (
        ("bad_password", name, password + "wrong", "3", Dialects.SMB_3_1_1),
        ("guest_account", "nobody", "", "3", Dialects.SMB_3_1_1),
        ("null_account", "\\", "", "3", Dialects.SMB_3_1_1),
        ("ntlmv1", name, password, "0", Dialects.SMB_3_1_1),
        ("dialect_downgrade", name, password, "3", Dialects.SMB_3_0_2),
    ):
        connection = Connection(uuid.uuid4(), "storage-server", require_signing=True)
        sent, response_lengths = [], []
        old_level = os.environ.get("LM_COMPAT_LEVEL")
        os.environ["LM_COMPAT_LEVEL"] = compatibility
        try:
            connection.connect(dialect=dialect, timeout=5)
            send = connection.transport.send

            def observed_auth(message):
                wire = message if isinstance(message, bytes) else message.pack()
                sent.append(len(wire))
                offset = wire.find(b"NTLMSSP\0\x03\0\0\0")
                if offset >= 0 and len(wire) >= offset + 24:
                    response_lengths.append(struct.unpack_from("<H", wire, offset + 20)[0])
                return send(message)

            connection.transport.send = observed_auth
            Session(connection, user, secret, require_encryption=True, auth_protocol="ntlm").connect()
        except SMBResponseException as error:
            expected = NtStatus.STATUS_NOT_SUPPORTED if label == "dialect_downgrade" else NtStatus.STATUS_LOGON_FAILURE
            if error.status != expected or (label != "dialect_downgrade" and not sent):
                raise
            if label == "ntlmv1" and response_lengths != [24]:
                raise ValueError("the refused request did not carry an actual NTLMv1 response")
            results.append({"case": label, "status": error.status, "ntlm_response_lengths": response_lengths})
        else:
            raise ValueError(f"server accepted {label}")
        finally:
            if old_level is None:
                os.environ.pop("LM_COMPAT_LEVEL", None)
            else:
                os.environ["LM_COMPAT_LEVEL"] = old_level
            connection.disconnect(False)
    return results


def decoder_tests():
    key = Ed25519PrivateKey.generate()
    pin = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    request = REQUEST.pack(b"DWQ2", 2, 0, bytes([1]) * 16, bytes([2]) * 32,
                           bytes([3]) * 32, bytes([4]) * 32, 3, 1, bytes(16))
    configuration = hashlib.sha256(b"generated-config").digest()
    file_id = struct.pack("<QQ", 11, 12)
    body = bytearray(BODY)
    body[:8] = b"DWS2\x02\x00\x00\x00"
    body[16:164], body[164:180], body[180:196] = request, bytes([5]) * 16, bytes([6]) * 16
    struct.pack_into("<IQI", body, 196, 10001, 7, 8)
    body[212:228] = file_id
    struct.pack_into("<QQ", body, 228, 1, 2)
    body[244:276], body[276:292] = configuration, bytes([255]) * 16
    for left in range(2):
        for right in range(2):
            body[276 + left * 4 + right] = 0
        start = 292 + left * 112
        struct.pack_into("<QQ", body, start, 1, 2)
        body[start + 16:start + 32] = bytes([7]) * 16
        struct.pack_into("<QIIQQ", body, start + 32, 1, 1, 0, 1, 2)

    def check(value, expected_configuration=configuration):
        signed = bytes(value) + key.sign(PURPOSE + bytes(value))
        return decode(signed, request, pin, 10001, 7, 8, file_id, expected_configuration)

    check(body)
    malformed = []
    for offset, value in ((740, 1), (516, 1), (332, 0), (340, 2), (277, 1)):
        changed = bytearray(body)
        changed[offset] = value
        malformed.append(changed)
    refused = 0
    for value in malformed:
        try:
            check(value)
        except ValueError:
            refused += 1
        else:
            raise ValueError("decoder accepted malformed evidence with a valid signature")
    try:
        check(body, bytes(32))
    except ValueError:
        refused += 1
    else:
        raise ValueError("decoder accepted a different actual server configuration")
    print(json.dumps({"authenticated_decoder_cases": refused + 1}))


def remote(suffix, held=False):
    from smbprotocol import Dialects
    from smbprotocol.connection import Connection
    from smbprotocol.exceptions import SMBResponseException
    from smbprotocol.header import NtStatus
    from smbprotocol.ioctl import SMB2IOCTLRequest, SMB2IOCTLResponse, IOCTLFlags
    from smbprotocol.open import Open, ImpersonationLevel, DirectoryAccessMask, FileAttributes, ShareAccess, CreateDisposition, CreateOptions
    from smbprotocol.session import Session
    from smbprotocol.tree import TreeConnect

    class WitnessControl:
        READ = CONTROL

    name, password = protected_client_file("credential", 256).decode("ascii").splitlines()
    if name != f"domus_{suffix}":
        raise ValueError("the generated credential has the wrong principal")
    pin = protected_client_file("public.key", 32)
    server_config = protected_client_file("server-config.sha256", 32)
    if len(pin) != 32 or len(server_config) != 32:
        raise ValueError("generated pin or configuration digest is incomplete")
    authentication = authentication_rejections(name, password) if suffix == "a" and not held else []
    uid, mask = (10001, 15) if suffix == "a" else (10002, 3)
    connection = Connection(uuid.uuid4(), "storage-server", require_signing=True)
    connection.connect(dialect=Dialects.SMB_3_1_1, timeout=5)
    try:
        session = Session(connection, name, password, require_encryption=True, auth_protocol="ntlm")
        session.connect()
        if connection.dialect != Dialects.SMB_3_1_1 or not session.encrypt_data or session.session_key is None:
            raise ValueError("authenticated encrypted transport is unavailable")
        counts = {"encrypted_sent": 0, "verified_decrypted": 0}
        plaintext_sent = 0
        send = connection.transport.send
        decrypt = connection._decrypt

        def observed_send(message):
            nonlocal plaintext_sent
            wire = message if isinstance(message, bytes) else message.pack()
            if wire[:4] == b"\xfdSMB":
                counts["encrypted_sent"] += 1
            elif wire[:4] == b"\xfeSMB":
                plaintext_sent += 1
            return send(message)

        def observed_decrypt(*args, **kwargs):
            result = decrypt(*args, **kwargs)
            counts["verified_decrypted"] += 1
            return result

        connection.transport.send = observed_send
        connection._decrypt = observed_decrypt
        def open_root(active_session, role, list_directory=True):
            tree = TreeConnect(active_session, rf"\\storage-server\{role}")
            tree.connect()
            handle = Open(tree, "")
            handle.create(
                ImpersonationLevel.Impersonation,
                DirectoryAccessMask.FILE_READ_ATTRIBUTES |
                (DirectoryAccessMask.FILE_LIST_DIRECTORY if list_directory else 0),
                FileAttributes.FILE_ATTRIBUTE_DIRECTORY,
                ShareAccess.FILE_SHARE_READ | ShareAccess.FILE_SHARE_WRITE | ShareAccess.FILE_SHARE_DELETE,
                CreateDisposition.FILE_OPEN,
                CreateOptions.FILE_DIRECTORY_FILE | CreateOptions.FILE_OPEN_REPARSE_POINT,
            )
            return tree, handle

        opened = {}
        for index, role in enumerate(ROLES):
            if not mask & (1 << index):
                continue
            opened[role] = open_root(session, role)
        sequences, contexts, instances, previous = {}, {}, {}, {}
        configuration = hashlib.sha256(b"generated-application-config").digest()
        inventory = hashlib.sha256(b"generated-binding-inventory").digest()

        def query(role, override_mask=None, malformed=None, advance=True, file_id=None,
                  action=0, arguments=b"", output_bound=65536):
            tree, handle = opened[role]
            active_session = tree.session
            seq = sequences.get(role, 0) + 1
            request = REQUEST.pack(
                b"DWQ2", 2, action, bytes.fromhex(os.environ["DOMUS_SMB_LAB_RUN"]),
                os.urandom(32), configuration, inventory,
                mask if override_mask is None else override_mask, seq,
                contexts.get(role, bytes(16)),
            ) + arguments
            if malformed is not None:
                request = malformed(request)
            packet = SMB2IOCTLRequest()
            packet["ctl_code"].enum_type = WitnessControl
            packet["ctl_code"] = CONTROL
            packet["file_id"] = handle.file_id if file_id is None else file_id
            packet["flags"] = IOCTLFlags.SMB2_0_IOCTL_IS_FSCTL
            packet["max_output_response"] = output_bound
            packet["buffer"] = request
            sent_before, received_before = counts.values()
            deadline = time.monotonic() + 5
            pending = connection.send(packet, sid=active_session.session_id, tid=tree.tree_connect_id)
            response = connection.receive(pending, timeout=5)
            if time.monotonic() > deadline:
                raise TimeoutError("authenticated witness request expired")
            if (response["session_id"].get_value() != active_session.session_id
                    or response["tree_id"].get_value() != tree.tree_connect_id
                    or counts["encrypted_sent"] <= sent_before
                    or counts["verified_decrypted"] <= received_before):
                raise ValueError("witness response lacks the exact protected transport")
            decoded = SMB2IOCTLResponse()
            decoded["ctl_code"].enum_type = WitnessControl
            if decoded.unpack(response["data"].get_value()) or decoded["file_id"].get_value() != handle.file_id:
                raise ValueError("authenticated response open differs")
            raw = decoded["buffer"].get_value()
            result = decode(raw, request, pin, uid, active_session.session_id, tree.tree_connect_id, handle.file_id, server_config)
            context, instance = bytes.fromhex(result["context"]), bytes.fromhex(result["instance"])
            if role in instances and instance != instances[role]:
                raise ValueError("witness instance changed in a live context")
            if role in contexts and context != contexts[role]:
                raise ValueError("witness context changed in a live connection")
            if advance:
                contexts[role], instances[role], sequences[role] = context, instance, seq
                previous[role] = request
            return result, raw, request, tree, handle

        def read_operation_proof():
            def named(name):
                encoded = name.encode("ascii")
                return struct.pack("<H", len(encoded)) + encoded

            def local_identity(name):
                value = os.stat(f"/fixture/payload/{name}", follow_symlinks=False)
                return READ_IDENTITY.pack(
                    value.st_dev, value.st_ino, value.st_size,
                    value.st_mtime_ns // 10**9, value.st_mtime_ns % 10**9,
                    value.st_ctime_ns // 10**9, value.st_ctime_ns % 10**9,
                )

            def listing(data):
                if len(data) < 2:
                    raise ValueError("directory reply lacks its entry count")
                count, = struct.unpack_from("<H", data)
                position, names = 2, []
                if count > 128:
                    raise ValueError("directory reply exceeds its entry bound")
                for _ in range(count):
                    if position + 2 > len(data):
                        raise ValueError("directory entry length is truncated")
                    size, = struct.unpack_from("<H", data, position)
                    position += 2
                    if not 1 <= size <= 255 or position + size > len(data):
                        raise ValueError("directory entry name is truncated")
                    name = data[position:position + size].decode("ascii")
                    if name in (".", "..") or any(value in name for value in ("/", "\\", ":", "\0")):
                        raise ValueError("directory entry is not one component")
                    names.append(name)
                    position += size
                if position != len(data) or len(set(names)) != len(names):
                    raise ValueError("directory reply has trailing or repeated entries")
                return names

            def isolated(action, arguments=b"", share="media", **options):
                key = "read-operation-case"
                opened[key] = open_root(session, share, options.pop("list_directory", True))
                try:
                    return query(key, action=action, arguments=arguments, **options)
                finally:
                    temporary_tree, temporary_open = opened.pop(key)
                    temporary_open.close()
                    temporary_tree.disconnect()
                    for values in (contexts, sequences, instances, previous):
                        values.pop(key, None)

            listed = query(first, action=1)[0]
            if (listed["result"] != "signed_native_evidence" or
                    sorted(listing(listed["operation_data"])) != sorted(os.listdir("/fixture/payload"))):
                raise ValueError("native directory listing differs from the generated fixture")
            complete, chunks = {}, 0
            for name, length in (("read-empty", 0), ("read-tail", 32785), ("read-full", READ_BOUND)):
                expected_identity = local_identity(name)
                metadata = query(first, action=2, arguments=named(name))[0]
                if metadata["operation_data"] != expected_identity:
                    raise ValueError("signed child metadata differs from independent native metadata")
                content = bytearray()
                while len(content) < length:
                    count = min(32768, length - len(content))
                    arguments = named(name) + expected_identity + struct.pack("<QI", len(content), count)
                    result, raw, request, active_tree, active_open = query(
                        first, action=3, arguments=arguments,
                    )
                    data = result["operation_data"]
                    if (result["result"] != "signed_native_evidence" or
                            len(data) != 56 + count or data[:56] != expected_identity):
                        raise ValueError("read range has a different identity or length")
                    content.extend(data[56:])
                    chunks += 1
                expected = hashlib.shake_256(b"domus-read-proof-v1").digest(length)
                if bytes(content) != expected:
                    raise ValueError("complete generated file bytes differ")
                complete[name] = {"bytes": length, "sha256": hashlib.sha256(content).hexdigest()}
            altered = raw[:BODY + 56] + bytes([raw[BODY + 56] ^ 1]) + raw[BODY + 57:]
            for candidate, requested in (
                (altered, request),
                (raw, request[:-1] + bytes([request[-1] ^ 1])),
            ):
                try:
                    decode(candidate, requested, pin, uid, session.session_id,
                           active_tree.tree_connect_id, active_open.file_id, server_config)
                except (ValueError, InvalidSignature):
                    pass
                else:
                    raise ValueError("read decoder accepted changed bytes or range parameters")
            identity = local_identity("read-full")
            refused = []
            for label, action, arguments, expected_error in (
                ("oversized_file", 2, named("read-over"), errno.EMSGSIZE),
                ("symlink", 3, named("read-link") + identity + struct.pack("<QI", 0, 1), errno.ELOOP),
                ("fifo", 2, named("read-fifo"), errno.EINVAL),
                ("directory", 2, named("staging"), errno.EINVAL),
                ("hardlink", 2, named("read-hard-link"), errno.EINVAL),
                ("missing_child", 2, named("missing-read-child"), errno.ENOENT),
                ("denied_content", 3, named("read-denied") + local_identity("read-denied") + struct.pack("<QI", 0, 1), errno.EACCES),
                ("changed_observation", 3, named("read-full") + bytes(56) + struct.pack("<QI", 0, 1), errno.ESTALE),
                ("oversized_range", 3, named("read-full") + identity + struct.pack("<QI", 0, 32769), errno.EINVAL),
                ("empty_range", 3, named("read-full") + identity + struct.pack("<QI", 0, 0), errno.EINVAL),
                ("past_eof", 3, named("read-full") + identity + struct.pack("<QI", READ_BOUND + 1, 1), errno.EINVAL),
            ):
                result = isolated(action, arguments)[0]
                if (result["result"] != "signed_native_refusal" or result["errno"] != expected_error
                        or result["operation_data"]):
                    raise ValueError(f"{label} returned bytes or the wrong native refusal")
                refused.append(label)
            for label, action, arguments, options, status in (
                ("traversal", 2, named("../read-full"), {}, NtStatus.STATUS_INVALID_PARAMETER),
                ("unknown_action", 4, b"", {}, NtStatus.STATUS_INVALID_PARAMETER),
                ("oversized_request", 2, bytes(4096), {}, NtStatus.STATUS_INVALID_PARAMETER),
                ("metadata_only_parent", 1, b"", {"list_directory": False}, NtStatus.STATUS_ACCESS_DENIED),
                ("small_reply_bound", 1, b"", {"output_bound": BODY + 64}, NtStatus.STATUS_BUFFER_TOO_SMALL),
            ):
                try:
                    isolated(action, arguments, **options)
                except SMBResponseException as error:
                    if error.status != status:
                        raise
                    refused.append(label)
                else:
                    raise ValueError(f"server accepted {label}")
            exact_directory_bound = False
            oversized_directory_refused = False
            if suffix == "a":
                result = isolated(1, share="staging")[0]
                if len(listing(result["operation_data"])) != 128:
                    raise ValueError("the exact directory entry bound did not pass")
                exact_directory_bound = True
                roots = observations["private-alias"]["roots"]
                if roots["private-alias"]["identity"] != roots["staging"]["identity"]:
                    result = isolated(1, share="private-alias")[0]
                    if (result["result"] != "signed_native_refusal" or
                            result["errno"] != errno.EMSGSIZE or result["operation_data"]):
                        raise ValueError("129 directory entries did not refuse without bytes")
                    oversized_directory_refused = True
            query(first)
            return {
                "complete_files": complete, "read_chunks": chunks,
                "native_refusals": refused, "tampered_read_refusals": 2,
                "exact_128_entry_directory": exact_directory_bound,
                "over_128_entries_refused": oversized_directory_refused,
                "fixture_file_bound": READ_BOUND,
            }

        observations = {role: query(role)[0] for role in opened}
        if held:
            specification = importlib.util.spec_from_file_location(
                "domus_fixture_witness", "/fixture/tools/storage-smb-witness.py",
            )
            witness = importlib.util.module_from_spec(specification)
            specification.loader.exec_module(witness)
            commands, emit = witness.commands, witness.emit
            held_read = None
            if os.environ.get("DOMUS_SMB_READ_PROOF") == "1":
                arguments = struct.pack("<H", 8) + b"sentinel"
                held_read = query("private-alias", action=2, arguments=arguments)[0]["operation_data"]
            emit({"phase": "authenticated_opened", "observation": observations["private-alias"]})
            for command in commands():
                if command == "exit":
                    return
                if command == "fresh":
                    if "fresh-private" in opened:
                        previous_tree, previous_open = opened.pop("fresh-private")
                        previous_open.close()
                        previous_tree.disconnect()
                    for values in (contexts, sequences, instances):
                        values.pop("fresh-private", None)
                    opened["fresh-private"] = open_root(session, "private-alias")
                    emit({"phase": "authenticated_fresh", "observation": query("fresh-private")[0]})
                elif command == "observe":
                    try:
                        result = query("private-alias")[0]
                    except SMBResponseException as error:
                        if error.status != FILE_INVALID:
                            raise
                        emit({"phase": "authenticated_replaced_root_refused", "status": error.status})
                    else:
                        emit({"phase": "authenticated_observed", "observation": result})
                elif command in ("read-observed", "read-mount") and held_read is not None:
                    action = 2 if command == "read-mount" else 3
                    arguments = (struct.pack("<H", 7) + b"mounted" if action == 2 else
                                 struct.pack("<H", 8) + b"sentinel" + held_read + struct.pack("<QI", 0, 1))
                    try:
                        result = query("private-alias", action=action, arguments=arguments)[0]
                    except SMBResponseException as error:
                        if error.status != FILE_INVALID:
                            raise
                        emit({"phase": "authenticated_read_root_refused", "status": error.status, "bytes": 0})
                    else:
                        emit({
                            "phase": "authenticated_read_observed",
                            "result": result["result"], "errno": result["errno"],
                            "bytes": len(result["operation_data"]),
                        })
                else:
                    raise ValueError("unsupported authenticated fixture command")
            return
        first = next(iter(opened))
        result, raw, request, tree, handle = query(first)
        rejected = 0
        for invalid, invalid_pin in (
            (raw, os.urandom(32)), (raw[:-1], pin),
            (raw[:64] + bytes([raw[64] ^ 1]) + raw[65:], pin),
            (raw[:-1] + bytes([raw[-1] ^ 1]), pin),
        ):
            try:
                decode(invalid, request, invalid_pin, uid, session.session_id, tree.tree_connect_id, handle.file_id, server_config)
            except (ValueError, InvalidSignature):
                rejected += 1
            else:
                raise ValueError("decoder accepted altered signed evidence")
        for wrong_request, wrong_uid, wrong_sid, wrong_tid, wrong_open in (
            (request[:24] + os.urandom(32) + request[56:], uid, session.session_id, tree.tree_connect_id, handle.file_id),
            (request, uid + 1, session.session_id, tree.tree_connect_id, handle.file_id),
            (request, uid, session.session_id + 1, tree.tree_connect_id, handle.file_id),
            (request, uid, session.session_id, tree.tree_connect_id + 1, handle.file_id),
            (request, uid, session.session_id, tree.tree_connect_id, bytes(16)),
        ):
            try:
                decode(raw, wrong_request, pin, wrong_uid, wrong_sid, wrong_tid, wrong_open, server_config)
            except ValueError:
                rejected += 1
            else:
                raise ValueError("decoder accepted a foreign signed association")
        for malformed in (
            lambda value: previous[first],
            lambda value: value[:132] + bytes(16),
            lambda value: value[:4] + b"\x01\x00" + value[6:],
            lambda value: value[:8] + bytes(16) + value[24:],
            lambda value: value[:24] + bytes(32) + value[56:],
            lambda value: value[:56] + bytes(32) + value[88:],
            lambda value: value[:88] + bytes(32) + value[120:],
            lambda value: value[:-1],
            lambda value: value[:56] + bytes([value[56] ^ 1]) + value[57:],
            lambda value: value[:88] + bytes([value[88] ^ 1]) + value[89:],
        ):
            try:
                query(first, malformed=malformed, advance=False)
            except SMBResponseException as error:
                if error.status != NtStatus.STATUS_INVALID_PARAMETER:
                    raise
                rejected += 1
            else:
                raise ValueError("server accepted stale or malformed witness context")
            query(first)
        wire_refusals = []

        def require_wire_refusal(label, operation, status):
            sent = counts["encrypted_sent"] + plaintext_sent
            try:
                operation()
            except SMBResponseException as error:
                if error.status != status:
                    raise
            else:
                raise ValueError(f"server accepted {label}")
            if counts["encrypted_sent"] + plaintext_sent <= sent:
                raise ValueError(f"{label} failed before a request reached the wire")
            wire_refusals.append(label)

        duplicate = Open(tree, "")
        duplicate.create(
            ImpersonationLevel.Impersonation, DirectoryAccessMask.FILE_READ_ATTRIBUTES,
            FileAttributes.FILE_ATTRIBUTE_DIRECTORY, ShareAccess.FILE_SHARE_READ,
            CreateDisposition.FILE_OPEN, CreateOptions.FILE_DIRECTORY_FILE,
        )
        require_wire_refusal("foreign_open_context",
                             lambda: query(first, file_id=duplicate.file_id, advance=False),
                             NtStatus.STATUS_INVALID_PARAMETER)
        duplicate.close()
        for label, active_session in (("foreign_tree_context", session),
                                     ("foreign_session_context",
                                      Session(connection, name, password, require_encryption=True, auth_protocol="ntlm"))):
            if active_session is not session:
                active_session.connect()
            opened[label] = open_root(active_session, first)
            try:
                require_wire_refusal(
                    label, lambda: query(label, malformed=lambda value: value[:132] + contexts[first], advance=False),
                    NtStatus.STATUS_INVALID_PARAMETER,
                )
            finally:
                temporary_tree, temporary_open = opened.pop(label)
                temporary_open.close()
                temporary_tree.disconnect()
            query(first)

        encrypted, signing, tree_encrypted = session.encrypt_data, session.signing_required, tree.encrypt_data
        before_plaintext = plaintext_sent
        try:
            session.encrypt_data = tree.encrypt_data = False
            session.signing_required = True
            require_wire_refusal("actual_signed_plaintext_witness",
                                 lambda: query(first, advance=False), NtStatus.STATUS_ACCESS_DENIED)
        finally:
            session.encrypt_data, session.signing_required, tree.encrypt_data = encrypted, signing, tree_encrypted
        if plaintext_sent != before_plaintext + 1:
            raise ValueError("the plaintext witness did not send exactly one actual SMB2 request")
        query(first)
        denied = []
        if suffix == "b":
            for role in ROLES[2:]:
                try:
                    TreeConnect(session, rf"\\storage-server\{role}").connect()
                except SMBResponseException as error:
                    if error.status != NtStatus.STATUS_ACCESS_DENIED:
                        raise
                    denied.append(role)
                else:
                    raise ValueError("restricted user connected to an unauthorized role")
            try:
                query(first, override_mask=15, advance=False)
            except SMBResponseException as error:
                if error.status != NtStatus.STATUS_ACCESS_DENIED:
                    raise
                denied.append("private_inventory")
            else:
                raise ValueError("witness disclosed unauthorized private metadata")
            query(first)
        old_context = contexts[first]
        handle.close()
        require_wire_refusal("closed_open", lambda: query(first, advance=False), NtStatus.STATUS_FILE_CLOSED)
        tree.disconnect()

        def fresh_context():
            opened[first] = open_root(session, first)
            for values in (contexts, sequences, instances):
                values.pop(first, None)
            return query(first)[0]

        fresh = fresh_context()
        if bytes.fromhex(fresh["context"]) == old_context:
            raise ValueError("closing an open did not discard its witness context")
        require_wire_refusal("disconnected_context",
                             lambda: query(first, malformed=lambda value: value[:132] + old_context, advance=False),
                             NtStatus.STATUS_INVALID_PARAMETER)
        query(first)
        restarted = os.environ.get("DOMUS_SMB_OLD_CONTEXT")
        if restarted is not None:
            if not re.fullmatch(r"[a-f0-9]{32}", restarted) or bytes.fromhex(restarted) == contexts[first]:
                raise ValueError("the old context did not belong to a different server generation")
            require_wire_refusal(
                "restarted_context",
                lambda: query(first, malformed=lambda value: value[:132] + bytes.fromhex(restarted), advance=False),
                NtStatus.STATUS_INVALID_PARAMETER,
            )
            query(first)
        old_pin = os.environ.get("DOMUS_SMB_OLD_PIN")
        if old_pin is not None:
            if not re.fullmatch(r"[a-f0-9]{64}", old_pin) or bytes.fromhex(old_pin) == pin:
                raise ValueError("the old operator pin was not rotated")
            _, rotated_raw, rotated_request, rotated_tree, rotated_open = query(first)
            try:
                decode(rotated_raw, rotated_request, bytes.fromhex(old_pin), uid,
                       session.session_id, rotated_tree.tree_connect_id, rotated_open.file_id, server_config)
            except InvalidSignature:
                pass
            else:
                raise ValueError("the old operator pin accepted the new server signature")
        read_operations = read_operation_proof() if os.environ.get("DOMUS_SMB_READ_PROOF") == "1" else None
        expired = False
        if suffix == "a":
            time.sleep(31)
            require_wire_refusal("expired_context", lambda: query(first, advance=False), NtStatus.STATUS_IO_TIMEOUT)
            expired_context = contexts[first]
            expired_tree, expired_open = opened[first]
            expired_open.close()
            expired_tree.disconnect()
            fresh_context()
            if contexts[first] == expired_context:
                raise ValueError("an expired context was retained after a fresh tree/open")
            expired = True
        print(json.dumps({
            "result": "authenticated_signed_native_evidence_measured",
            "principal": name, "observations": observations,
            "decoder_and_server_refusals": rejected, "denied_roles": denied,
            "transport": counts, "valid_after_refusals": query(first)[0],
            "wire_refusals": wire_refusals, "actual_plaintext_requests": plaintext_sent - before_plaintext,
            "expiry_and_fresh_context": expired,
            "authentication_refusals": authentication,
            "restarted_context_checked": restarted is not None,
            "old_pin_refused": old_pin is not None,
            "read_operations": read_operations,
            "application_revision_scope": "Generated frozen request hashes only; not production SQLite acceptance.",
        }, sort_keys=True))
    finally:
        connection.disconnect(False)


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        decoder_tests()
        sys.exit(0)
    if not re.fullmatch(r"[a-f0-9]{32}", os.environ.get("DOMUS_SMB_LAB_RUN", "")):
        raise ValueError("authenticated proof requires the generated lab harness")
    if sys.argv[1:] == ["--setup"]:
        setup()
    elif sys.argv[1:] == ["--setup-read-files"]:
        setup_read_files()
    elif sys.argv[1:] in (["--client-a"], ["--client-b"]):
        remote(sys.argv[1][-1])
    elif sys.argv[1:] == ["--held-a"]:
        remote("a", held=True)
    elif sys.argv[1:] == ["--missing-key"]:
        from smbprotocol.exceptions import SMBResponseException
        from smbprotocol.header import NtStatus
        try:
            remote("a", held=True)
        except SMBResponseException as error:
            if error.status != NtStatus.STATUS_BAD_NETWORK_NAME:
                raise
            print(json.dumps({"result": "missing_witness_key_refused", "status": error.status}))
        else:
            raise ValueError("a missing signing key did not stop native evidence")
    else:
        raise ValueError("select the generated setup or client")
