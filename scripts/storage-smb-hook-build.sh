#!/bin/sh
set -eu

test "${DOMUS_SMB_HOOK_BUILD_APPROVED:-}" = 1 || {
    echo "Separate generated-lab source, build-package and module approval is required." >&2
    exit 1
}
smbd --version | grep -E '^Version 4\.17\.12(-|$)'

source=/opt/domus-samba-source
if ! test -f "$source/bin/default/include/config.h"; then
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
        samba-dev libpopt-dev libgnutls28-dev libtalloc-dev libtdb-dev \
        libtevent-dev libldb-dev python3-setuptools python3-dev libacl1-dev \
        libattr1-dev pkg-config gnupg flex bison libparse-yapp-perl libjson-perl
    mkdir -p "$source" /tmp/domus-samba-key
    chmod 700 /tmp/domus-samba-key
    curl --fail --silent --show-error --max-time 120 --proto '=https' \
        https://download.samba.org/pub/samba/stable/samba-4.17.12.tar.gz \
        -o /tmp/samba.tar.gz
    curl --fail --silent --show-error --max-time 120 --proto '=https' \
        https://download.samba.org/pub/samba/stable/samba-4.17.12.tar.asc \
        -o /tmp/samba.tar.asc
    curl --fail --silent --show-error --max-time 120 --proto '=https' \
        https://download.samba.org/pub/samba/samba-pubkey.asc \
        -o /tmp/samba-key
    echo '6129a7f967b822b308c57152326e7302711c2d0b9830a82d0c21832b648741f4  /tmp/samba.tar.gz' | sha256sum -c -
    gpg --batch --homedir /tmp/domus-samba-key --import /tmp/samba-key
    gzip -dc /tmp/samba.tar.gz > /tmp/samba.tar
    gpg --batch --homedir /tmp/domus-samba-key --status-fd 1 \
        --verify /tmp/samba.tar.asc /tmp/samba.tar > /tmp/samba-verification
    grep -E '^\[GNUPG:\] VALIDSIG 81F5E2832BD2545A1897B713AA99442FB680B620 ' /tmp/samba-verification
    tar -xf /tmp/samba.tar -C "$source" --strip-components=1
    cd "$source"
    ./configure --without-ad-dc --without-ldap --without-ads --without-pam \
        --without-systemd --without-json --disable-python --without-ldb-lmdb \
        --without-gpgme --without-libarchive --without-regedit --without-winbind \
        --without-gettext --without-libunwind --with-shared-modules='!vfs_snapper' \
        > /fixture/tools/samba-configure.log 2>&1 || {
        tail -60 /fixture/tools/samba-configure.log >&2
        exit 1
    }
fi

echo '6129a7f967b822b308c57152326e7302711c2d0b9830a82d0c21832b648741f4  /tmp/samba.tar.gz' | sha256sum -c -
cp /tmp/storage-smb-native-probe.c "$source/source3/modules/vfs_domus_probe.c"
cp /tmp/storage-smb-auth-probe.h "$source/source3/modules/domus_auth_probe.h"
mkdir -p /fixture/tools/scripts /fixture/tools/crates/domus/src/infrastructure/filesystem
cp /tmp/storage-smb-auth-policy.rs /fixture/tools/scripts/storage-smb-auth-policy.rs
cp /tmp/domus-topology-prototype.rs /fixture/tools/crates/domus/src/infrastructure/filesystem/topology_prototype.rs
rustc --edition 2024 --deny warnings --crate-type staticlib -C relocation-model=pic \
    /fixture/tools/scripts/storage-smb-auth-policy.rs \
    -o "$source/bin/libdomus_witness.a"
rustc --edition 2024 --deny warnings --test \
    /fixture/tools/scripts/storage-smb-auth-policy.rs \
    -o /fixture/tools/auth-policy-tests
/fixture/tools/auth-policy-tests
cd "$source"
python3 -c '
from pathlib import Path
path = Path("source3/modules/wscript_build")
text = path.read_text()
previous = """
bld.SAMBA3_MODULE("vfs_domus_probe",
                 subsystem="vfs",
                 source="vfs_domus_probe.c",
                 deps="samba-util",
                 cflags="-Wall -Wextra -Werror",
                 init_function="domus_probe_init",
                 internal_module=False,
                 enabled=True)
"""
target = previous.replace(
    "deps=\"samba-util\",",
    "deps=\"samba-util gnutls\","
) + """
bld.get_tgen_by_name("vfs_domus_probe").samba_ldflags.extend([
    "/opt/domus-samba-source/bin/libdomus_witness.a",
    "-lgcc_s", "-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc"])
"""
if "vfs_domus_probe" in text:
    if text.endswith(previous):
        path.write_text(text[:-len(previous)] + target)
    elif not text.endswith(target):
        raise RuntimeError("the source contains a different generated target")
else:
    path.write_text(text + target)
'
PYTHONHASHSEED=1 ./buildtools/bin/waf build --targets=vfs_domus_probe,smbd/smbd --jobs=1 \
    > /fixture/tools/samba-hook-build.log 2>&1 || {
    tail -80 /fixture/tools/samba-hook-build.log >&2
    exit 1
}
module="$source/bin/default/source3/modules/libvfs_module_domus_probe.so"
test -f "$module"
server="$source/bin/default/source3/smbd/smbd"
test -x "$server"
"$server" --version | grep -E '^Version 4\.17\.12$'
directory=$("$server" -b | sed -n 's/^[[:space:]]*MODULESDIR: //p')
test -n "$directory"
install -d "$directory/vfs"
install -m 755 "$module" "$directory/vfs/domus_probe.so"
sha256sum /tmp/storage-smb-native-probe.c "$directory/vfs/domus_probe.so" "$server"
ldd "$directory/vfs/domus_probe.so" > /fixture/tools/samba-hook-linkage.log
ldd "$server" > /fixture/tools/samba-server-linkage.log
if grep 'not found' /fixture/tools/samba-hook-linkage.log /fixture/tools/samba-server-linkage.log; then
    echo "A generated server or hook dependency is missing." >&2
    exit 1
fi
echo "Generated read-only hook and server built from the same Samba 4.17.12 source."
