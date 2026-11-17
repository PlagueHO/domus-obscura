/* SPDX-License-Identifier: GPL-3.0-or-later
 * Original fixed-fixture authenticated witness, not a production provider.
 */
#include "librpc/gen_ndr/smbXsrv.h"
#include "auth.h"
#include <gnutls/abstract.h>
#include <gnutls/crypto.h>
#include <sys/ioctl.h>
#include <dirent.h>

#define DOMUS_AUTH_CONTROL 0x00096408
#define AUTH_REQUEST 148U
#define AUTH_BODY 840U
#define AUTH_SIGNATURE 64U
#define AUTH_DATA_BOUND 49152U
#define AUTH_FILE_BOUND (1024U * 1024U)

struct auth_state {
    struct probe_root roots[4];
    uint8_t context[16];
    struct timespec started;
    uint64_t sequence;
    uint64_t persistent_open, volatile_open;
    bool invalidated;
    uint32_t mask;
    uint8_t configuration[32], inventory[32];
};

struct auth_observation {
    int descriptor;
    uint64_t device, inode, mount_id;
};

int domus_probe_relationships(const struct auth_observation *observations,
                              uint32_t mask, uint8_t *matrix);

static gnutls_privkey_t witness_signer;
static uint8_t witness_instance[16];
static uint8_t witness_config[32];
static const char witness_purpose[] = "domus.native-witness.v2";
static const char *auth_paths[4] = {
    "/fixture/payload", "/fixture/payload", "/fixture/payload/staging",
    "/fixture/private"
};

static bool auth_enabled(void)
{
    const char *enabled = getenv("DOMUS_SMB_AUTH_PROOF");
    return enabled != NULL && strcmp(enabled, "1") == 0;
}

static int read_startup_file(const char *first, const char *second,
                             const char *name, uint8_t *bytes, size_t bound,
                             bool secret)
{
    int root = -1, parent = -1, directory = -1, file = -1, result = -1;
    int flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
    struct stat identity;
    ssize_t length;
    root = open("/", flags);
    if (root < 0) {
        goto done;
    }
    parent = openat(root, first, flags);
    if (parent < 0) {
        goto done;
    }
    directory = openat(parent, second, flags);
    if (directory < 0) {
        goto done;
    }
    file = openat(directory, name, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (file < 0 || fstat(file, &identity) != 0) {
        goto done;
    }
    if (!S_ISREG(identity.st_mode) || identity.st_size <= 0 ||
        (uint64_t)identity.st_size >= bound ||
        (secret && (identity.st_uid != 0 || (identity.st_mode & 0777) != 0600 ||
                    identity.st_nlink != 1))) {
        errno = EACCES;
        goto done;
    }
    length = read(file, bytes, bound);
    if (length != identity.st_size) {
        errno = EIO;
        goto done;
    }
    result = length;
done:
    if (file >= 0) close(file);
    if (directory >= 0) close(directory);
    if (parent >= 0) close(parent);
    if (root >= 0) close(root);
    return result;
}

static int initialize_signer(void)
{
    uint8_t bytes[16384];
    gnutls_datum_t key;
    int length, result;
    if (witness_signer != NULL) {
        return 0;
    }
    length = read_startup_file("secrets", "witness", "key.pem",
                               bytes, sizeof(bytes), true);
    if (length < 0) {
        DEBUG(0, ("Authenticated witness refused startup key: %d\n", errno));
        return -1;
    }
    key.data = bytes;
    key.size = length;
    result = gnutls_privkey_init(&witness_signer);
    if (result >= 0) {
        result = gnutls_privkey_import_x509_raw(witness_signer, &key,
                                               GNUTLS_X509_FMT_PEM, NULL, 0);
    }
    gnutls_memset(bytes, 0, sizeof(bytes));
    if (result < 0 ||
        gnutls_privkey_get_pk_algorithm(witness_signer, NULL) != GNUTLS_PK_EDDSA_ED25519) {
        DEBUG(0, ("Authenticated witness refused signing algorithm: %d\n", result));
        if (witness_signer != NULL) {
            gnutls_privkey_deinit(witness_signer);
            witness_signer = NULL;
        }
        return -1;
    }
    length = read_startup_file("etc", "samba", "smb.conf",
                               bytes, sizeof(bytes), false);
    if (length < 0 ||
        gnutls_hash_fast(GNUTLS_DIG_SHA256, bytes, length, witness_config) < 0 ||
        gnutls_rnd(GNUTLS_RND_RANDOM, witness_instance, sizeof(witness_instance)) < 0) {
        DEBUG(0, ("Authenticated witness refused startup evidence\n"));
        gnutls_privkey_deinit(witness_signer);
        witness_signer = NULL;
        return -1;
    }
    return 0;
}

static void close_auth(void **data)
{
    struct auth_state *state = *data;
    size_t index;
    if (state == NULL) {
        return;
    }
    for (index = 0; index < 4; index++) {
        close_chain(&state->roots[index]);
    }
    TALLOC_FREE(state);
    *data = NULL;
}

static int initialize_auth(struct probe_root *root, struct files_struct *fsp,
                            const uint8_t *input)
{
    struct auth_state *state;
    size_t index;
    if (root->auth != NULL) {
        return 0;
    }
    state = talloc_zero(root, struct auth_state);
    if (state == NULL) {
        errno = ENOMEM;
        return -1;
    }
    for (index = 0; index < 4; index++) {
        state->roots[index].control = -1;
    }
    root->auth = state;
    state->persistent_open = fsp->op->global->open_persistent_id;
    state->volatile_open = fsp->op->global->open_volatile_id;
    state->mask = IVAL(input, 120);
    memcpy(state->configuration, input + 56, 32);
    memcpy(state->inventory, input + 88, 32);
    if (clock_gettime(CLOCK_MONOTONIC, &state->started) != 0 ||
        gnutls_rnd(GNUTLS_RND_RANDOM, state->context, sizeof(state->context)) < 0) {
        close_auth(&root->auth);
        return -1;
    }
    for (index = 0; index < 4; index++) {
        if (!(state->mask & (1U << index))) {
            continue;
        }
        state->roots[index].path = auth_paths[index];
        if (open_chain(&state->roots[index]) != 0) {
            int saved_error = errno;
            close_auth(&root->auth);
            errno = saved_error;
            return -1;
        }
    }
    return 0;
}

static bool read_proof_enabled(void)
{
    const char *enabled = getenv("DOMUS_SMB_READ_PROOF");
    return enabled != NULL && strcmp(enabled, "1") == 0;
}

static bool read_request_valid(const uint8_t *input, uint32_t length)
{
    uint16_t action = SVAL(input, 6), names;
    size_t index;
    if (action == 0 || action == 1) {
        return length == AUTH_REQUEST && (action == 0 || read_proof_enabled());
    }
    if (!read_proof_enabled() || (action != 2 && action != 3) ||
        length < AUTH_REQUEST + 2) {
        return false;
    }
    names = SVAL(input, AUTH_REQUEST);
    if (names == 0 || names > 255 ||
        length != AUTH_REQUEST + 2 + names + (action == 3 ? 68U : 0U)) {
        return false;
    }
    for (index = 0; index < names; index++) {
        uint8_t value = input[AUTH_REQUEST + 2 + index];
        if (value < 32 || value > 126 || value == '/' || value == '\\' ||
            value == ':') {
            return false;
        }
    }
    return !(names == 1 && input[AUTH_REQUEST + 2] == '.') &&
           !(names == 2 && input[AUTH_REQUEST + 2] == '.' &&
             input[AUTH_REQUEST + 3] == '.');
}

static void read_identity(uint8_t *record, const struct stat *identity)
{
    SBVAL(record, 0, identity->st_dev);
    SBVAL(record, 8, identity->st_ino);
    SBVAL(record, 16, identity->st_size);
    SBVAL(record, 24, identity->st_mtim.tv_sec);
    SBVAL(record, 32, identity->st_mtim.tv_nsec);
    SBVAL(record, 40, identity->st_ctim.tv_sec);
    SBVAL(record, 48, identity->st_ctim.tv_nsec);
}

static bool unchanged_read(const struct stat *left, const struct stat *right)
{
    uint8_t first[56], second[56];
    read_identity(first, left);
    read_identity(second, right);
    return memcmp(first, second, sizeof(first)) == 0;
}

static int read_operation(int parent, uint64_t parent_mount,
                           const uint8_t *input, uint8_t *data,
                           uint32_t *output_length)
{
    uint16_t action = SVAL(input, 6), names;
    struct stat before, after;
    uint64_t mount;
    int descriptor = -1, result = -1, saved_error;
    char name[256];
    *output_length = 0;
    if (action == 1) {
        DIR *directory;
        struct dirent *entry;
        uint16_t count = 0;
        uint32_t length = 2;
        descriptor = openat(parent, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        if (descriptor < 0 || fstat(descriptor, &before) != 0) {
            goto done;
        }
        directory = fdopendir(descriptor);
        if (directory == NULL) {
            goto done;
        }
        descriptor = -1;
        errno = 0;
        while ((entry = readdir(directory)) != NULL) {
            size_t size;
            if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) {
                continue;
            }
            size = strlen(entry->d_name);
            if (size == 0 || size > 255 || count == 128 ||
                length + 2 + size > AUTH_DATA_BOUND) {
                errno = EMSGSIZE;
                break;
            }
            SSVAL(data, length, size);
            memcpy(data + length + 2, entry->d_name, size);
            length += 2 + size;
            count++;
        }
        saved_error = errno;
        if (saved_error == 0 &&
            (fstat(dirfd(directory), &after) != 0 || !unchanged_read(&before, &after))) {
            saved_error = ESTALE;
        }
        if (closedir(directory) != 0 && saved_error == 0) {
            saved_error = errno;
        }
        if (saved_error != 0) {
            errno = saved_error;
            return -1;
        }
        SSVAL(data, 0, count);
        *output_length = length;
        return 0;
    }
    names = SVAL(input, AUTH_REQUEST);
    memcpy(name, input + AUTH_REQUEST + 2, names);
    name[names] = '\0';
    descriptor = openat(parent, name,
                         (action == 2 ? O_PATH : O_RDONLY | O_NONBLOCK) |
                         O_NOFOLLOW | O_CLOEXEC);
    if (descriptor < 0 || fstat(descriptor, &before) != 0 ||
        inspect_mount(descriptor, &before, &mount) != 0) {
        goto done;
    }
    if (mount != parent_mount) {
        errno = EXDEV;
        goto done;
    }
    if (!S_ISREG(before.st_mode) || before.st_nlink != 1) {
        errno = EINVAL;
        goto done;
    }
    if (before.st_size < 0 || (uint64_t)before.st_size > AUTH_FILE_BOUND) {
        errno = EMSGSIZE;
        goto done;
    }
    read_identity(data, &before);
    *output_length = 56;
    if (action == 3) {
        const uint8_t *parameters = input + AUTH_REQUEST + 2 + names;
        uint64_t offset = BVAL(parameters, 56);
        uint32_t requested = IVAL(parameters, 64), completed = 0;
        if (memcmp(data, parameters, 56) != 0) {
            errno = ESTALE;
            goto done;
        }
        if (requested == 0 || requested > 32768 || offset > (uint64_t)before.st_size ||
            requested > (uint64_t)before.st_size - offset) {
            errno = EINVAL;
            goto done;
        }
        while (completed < requested) {
            ssize_t count = pread(descriptor, data + 56 + completed,
                                   requested - completed, offset + completed);
            if (count < 0 && errno == EINTR) {
                continue;
            }
            if (count <= 0) {
                if (count == 0) errno = EIO;
                goto done;
            }
            completed += count;
        }
        *output_length += completed;
    }
    if (fstat(descriptor, &after) != 0 || !unchanged_read(&before, &after)) {
        errno = ESTALE;
        goto done;
    }
    result = 0;
done:
    saved_error = errno;
    if (descriptor >= 0 && close(descriptor) != 0 && result == 0) {
        saved_error = errno;
        result = -1;
    }
    errno = saved_error;
    return result;
}

static NTSTATUS auth_fsctl(struct vfs_handle_struct *handle,
                           struct files_struct *fsp, TALLOC_CTX *context,
                           const uint8_t *input, uint32_t input_length,
                           uint8_t **output, uint32_t output_bound,
                           uint32_t *output_length)
{
    struct probe_root *root;
    struct auth_state *state;
    struct auth_observation observations[4] = {0};
    struct stat parent;
    struct timespec now;
    uint8_t *reply, *signed_bytes;
    gnutls_datum_t message, signature = {0};
    uint32_t mask, allowed, uid, payload_length = 0, body_length = AUTH_BODY;
    size_t index, component;
    int descriptor, error = 0, result;
    if (!auth_enabled() || witness_signer == NULL) {
        return NT_STATUS_NOT_SUPPORTED;
    }
    SMB_VFS_HANDLE_GET_DATA(handle, root, struct probe_root,
                            return NT_STATUS_INVALID_HANDLE);
    if (input_length < AUTH_REQUEST || input_length > 4096 ||
        output_bound < AUTH_BODY + AUTH_SIGNATURE ||
        memcmp(input, "DWQ2", 4) != 0 || SVAL(input, 4) != 2 ||
        !read_request_valid(input, input_length) ||
        memcmp(input + 8, root->revision, 16) != 0 ||
        memcmp(input + 56, (uint8_t[32]){0}, 32) == 0 ||
        memcmp(input + 88, (uint8_t[32]){0}, 32) == 0) {
        return NT_STATUS_INVALID_PARAMETER;
    }
    if (fsp->conn == NULL || fsp->op == NULL || fsp->conn->session_info == NULL ||
        fsp->conn->session_info->unix_token == NULL ||
        fsp->conn->tcon == NULL || fsp->conn->tcon->global == NULL ||
        !(fsp->conn->tcon->global->encryption_flags & SMBXSRV_ENCRYPTION_REQUIRED) ||
        !fsp->conn->read_only ||
        !(fsp->access_mask & SEC_FILE_READ_ATTRIBUTE)) {
        return NT_STATUS_ACCESS_DENIED;
    }
    if (SVAL(input, 6) != 0 && !(fsp->access_mask & SEC_DIR_LIST)) {
        return NT_STATUS_ACCESS_DENIED;
    }
    if (SVAL(input, 6) != 0 &&
        output_bound < AUTH_BODY + AUTH_DATA_BOUND + AUTH_SIGNATURE) {
        return NT_STATUS_BUFFER_TOO_SMALL;
    }
    uid = fsp->conn->session_info->unix_token->uid;
    allowed = uid == 10001 ? 15 : (uid == 10002 ? 3 : 0);
    mask = IVAL(input, 120);
    if (mask == 0 || (mask & ~allowed) != 0 ||
        !(mask & (1U << root->role))) {
        return NT_STATUS_ACCESS_DENIED;
    }
    if (initialize_auth(root, fsp, input) != 0) {
        DEBUG(0, ("Authenticated witness refused context creation: %d\n", errno));
        return NT_STATUS_INTERNAL_ERROR;
    }
    state = root->auth;
    if (state->invalidated) {
        return NT_STATUS_FILE_INVALID;
    }
    if (state->persistent_open != fsp->op->global->open_persistent_id ||
        state->volatile_open != fsp->op->global->open_volatile_id ||
        state->mask != mask || memcmp(state->configuration, input + 56, 32) != 0 ||
        memcmp(state->inventory, input + 88, 32) != 0) {
        return NT_STATUS_INVALID_PARAMETER;
    }
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return NT_STATUS_INTERNAL_ERROR;
    }
    if (now.tv_sec - state->started.tv_sec >= 30) {
        return NT_STATUS_IO_TIMEOUT;
    }
    if (state->sequence == UINT64_MAX || BVAL(input, 124) != state->sequence + 1 ||
        (state->sequence != 0 && memcmp(input + 132, state->context, 16) != 0) ||
        (state->sequence == 0 && memcmp(input + 132, (uint8_t[16]){0}, 16) != 0) ||
        memcmp(input + 24, (uint8_t[32]){0}, 32) == 0) {
        return NT_STATUS_INVALID_PARAMETER;
    }
    descriptor = fsp_get_pathref_fd(fsp);
    if (fstat(descriptor, &parent) != 0 ||
        !same_identity(&parent, &root->identities[root->count - 1]) ||
        !S_ISDIR(parent.st_mode) || binding_matches(root) != 0) {
        state->invalidated = true;
        return NT_STATUS_FILE_INVALID;
    }
    state->sequence++;
    reply = talloc_zero_array(context, uint8_t,
                              AUTH_BODY + AUTH_DATA_BOUND + AUTH_SIGNATURE);
    if (reply == NULL) {
        return NT_STATUS_NO_MEMORY;
    }
    memcpy(reply, "DWS2", 4);
    SSVAL(reply, 4, 2);
    memcpy(reply + 16, input, AUTH_REQUEST);
    memcpy(reply + 164, state->context, 16);
    memcpy(reply + 180, witness_instance, 16);
    SIVAL(reply, 196, uid);
    SBVAL(reply, 200, fsp->vuid);
    SIVAL(reply, 208, fsp->conn->cnum);
    SBVAL(reply, 212, fsp->op->global->open_persistent_id);
    SBVAL(reply, 220, fsp->op->global->open_volatile_id);
    SBVAL(reply, 228, parent.st_dev);
    SBVAL(reply, 236, parent.st_ino);
    memcpy(reply + 244, witness_config, 32);
    memset(reply + 276, 255, 16);
    for (index = 0; index < 4; index++) {
        struct probe_root *retained = &state->roots[index];
        struct stat identity;
        uint8_t filesystem[17] = {0};
        uint8_t *record = reply + 292 + index * 112;
        uint64_t mount;
        observations[index].descriptor = -1;
        if (!(mask & (1U << index))) {
            continue;
        }
        observations[index].descriptor =
            same_identity(&parent, &retained->identities[retained->count - 1])
            ? descriptor : retained->descriptors[retained->count - 1];
        if (binding_matches(retained) != 0 ||
            fstat(observations[index].descriptor, &identity) != 0 ||
            inspect_mount(observations[index].descriptor, &identity, &mount) != 0 ||
            ioctl(observations[index].descriptor, 0x80111500, filesystem) != 0) {
            error = errno;
            goto refused;
        }
        if (filesystem[0] != 16 || memcmp(filesystem + 1, (uint8_t[16]){0}, 16) == 0) {
            error = ENOTSUP;
            goto refused;
        }
        observations[index].device = identity.st_dev;
        observations[index].inode = identity.st_ino;
        observations[index].mount_id = mount;
        SBVAL(record, 0, identity.st_dev);
        SBVAL(record, 8, identity.st_ino);
        memcpy(record + 16, filesystem + 1, 16);
        SBVAL(record, 32, mount);
        SIVAL(record, 40, retained->count);
        for (component = 0; component < retained->count; component++) {
            SBVAL(record, 48 + component * 16, retained->identities[component].st_dev);
            SBVAL(record, 56 + component * 16, retained->identities[component].st_ino);
        }
    }
    if (SVAL(input, 6) != 0) {
        uint64_t parent_mount;
        if (inspect_mount(descriptor, &parent, &parent_mount) != 0 ||
            read_operation(descriptor, parent_mount, input,
                            reply + AUTH_BODY, &payload_length) != 0) {
            error = errno;
            goto refused;
        }
        body_length += payload_length;
        for (index = 0; index < 4; index++) {
            if ((mask & (1U << index)) && binding_matches(&state->roots[index]) != 0) {
                error = errno;
                goto refused;
            }
        }
    }
    if (domus_probe_relationships(observations, mask, reply + 276) != 0) {
        error = EIO;
        goto refused;
    }
    for (index = 0; index < 4; index++) {
        if ((mask & (1U << index)) && binding_matches(&state->roots[index]) != 0) {
            error = errno;
            goto refused;
        }
    }
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return NT_STATUS_INTERNAL_ERROR;
    }
    if (now.tv_sec - state->started.tv_sec >= 30) {
        return NT_STATUS_IO_TIMEOUT;
    }
    goto sign;
refused:
    state->invalidated = true;
    SIVAL(reply, 8, 1);
    SIVAL(reply, 12, error);
    memset(reply + 276, 0, AUTH_BODY + AUTH_DATA_BOUND - 276);
    body_length = AUTH_BODY;
    payload_length = 0;
sign:
    if (SVAL(input, 6) != 0) {
        if (gnutls_hash_fast(GNUTLS_DIG_SHA256, input, input_length, reply + 740) < 0) {
            return NT_STATUS_INTERNAL_ERROR;
        }
        SIVAL(reply, 772, payload_length);
    }
    signed_bytes = talloc_array(context, uint8_t,
                                sizeof(witness_purpose) - 1 + body_length);
    if (signed_bytes == NULL) {
        return NT_STATUS_NO_MEMORY;
    }
    memcpy(signed_bytes, witness_purpose, sizeof(witness_purpose) - 1);
    memcpy(signed_bytes + sizeof(witness_purpose) - 1, reply, body_length);
    message.data = signed_bytes;
    message.size = sizeof(witness_purpose) - 1 + body_length;
    result = gnutls_privkey_sign_data2(witness_signer, GNUTLS_SIGN_EDDSA_ED25519,
                                      0, &message, &signature);
    if (result < 0 || signature.size != AUTH_SIGNATURE) {
        DEBUG(0, ("Authenticated witness signing failed: %d\n", result));
        gnutls_free(signature.data);
        return NT_STATUS_INTERNAL_ERROR;
    }
    memcpy(reply + body_length, signature.data, signature.size);
    gnutls_free(signature.data);
    *output = reply;
    *output_length = body_length + AUTH_SIGNATURE;
    return NT_STATUS_OK;
}
