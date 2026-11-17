/* SPDX-License-Identifier: GPL-3.0-or-later
 * Original generated-fixture diagnostic for Samba 4.17.12, not a runtime adapter.
 */
#include "includes.h"
#include "smbd/smbd.h"
#include "libcli/security/security.h"
#include <sys/statfs.h>
#include <sys/sysmacros.h>

#define DOMUS_PROBE_CONTROL 0x00096404
#define DOMUS_PROBE_HEADER 96
#define DOMUS_PROBE_REQUEST 66U
#define DOMUS_PROBE_MARKER 32
#define DOMUS_PROBE_COMPONENTS 4

struct probe_root {
    int descriptors[DOMUS_PROBE_COMPONENTS];
    struct stat identities[DOMUS_PROBE_COMPONENTS];
    size_t count;
    uint64_t mount_id;
    int control;
    const char *path;
    uint8_t revision[16];
    uint64_t sequence;
    uint32_t role;
    void *auth;
};

static void close_auth(void **data);

static bool same_identity(const struct stat *left, const struct stat *right)
{
    return left->st_dev == right->st_dev && left->st_ino == right->st_ino;
}

static int inspect_mount(int descriptor, const struct stat *identity,
                         uint64_t *mount_id)
{
    struct statx extended;
    struct statfs filesystem;
    if (statx(descriptor, "", AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW,
              STATX_BASIC_STATS | STATX_MNT_ID, &extended) != 0 ||
        fstatfs(descriptor, &filesystem) != 0) {
        return -1;
    }
    if (!(extended.stx_mask & STATX_MNT_ID) ||
        extended.stx_ino != identity->st_ino ||
        extended.stx_dev_major != major(identity->st_dev) ||
        extended.stx_dev_minor != minor(identity->st_dev) ||
        filesystem.f_type != 0xef53 || !identity->st_ino || !identity->st_dev) {
        errno = ENOTSUP;
        return -1;
    }
    *mount_id = extended.stx_mnt_id;
    return 0;
}

static void close_chain(struct probe_root *root)
{
    size_t index;
    for (index = 0; index < root->count; index++) {
        close(root->descriptors[index]);
    }
    if (root->control >= 0) {
        close(root->control);
    }
}

static void close_root(void **data)
{
    struct probe_root *root = *data;
    if (root == NULL) {
        return;
    }
    close_auth(&root->auth);
    close_chain(root);
    TALLOC_FREE(root);
    *data = NULL;
}

static int open_chain(struct probe_root *root)
{
    char path[64];
    char *component;
    char *position = NULL;
    int flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
    if (strlen(root->path) >= sizeof(path)) {
        errno = EINVAL;
        return -1;
    }
    memcpy(path, root->path, strlen(root->path) + 1);
    root->descriptors[0] = open("/", flags);
    if (root->descriptors[0] < 0) {
        return -1;
    }
    root->count = 1;
    if (fstat(root->descriptors[0], &root->identities[0]) != 0) {
        return -1;
    }
    for (component = strtok_r(path + 1, "/", &position); component != NULL;
         component = strtok_r(NULL, "/", &position)) {
        size_t index = root->count;
        if (index == DOMUS_PROBE_COMPONENTS) {
            errno = EINVAL;
            return -1;
        }
        root->descriptors[index] = openat(root->descriptors[index - 1],
                                          component, flags);
        if (root->descriptors[index] < 0) {
            return -1;
        }
        root->count++;
        if (fstat(root->descriptors[index], &root->identities[index]) != 0) {
            return -1;
        }
    }
    return inspect_mount(root->descriptors[root->count - 1],
                         &root->identities[root->count - 1], &root->mount_id);
}

static int binding_matches(const struct probe_root *retained)
{
    struct probe_root current = {.control = -1, .path = retained->path};
    size_t index;
    int result = -1;
    int saved_error;
    if (open_chain(&current) != 0) {
        goto done;
    }
    if (current.count != retained->count ||
        current.mount_id != retained->mount_id) {
        errno = ESTALE;
        goto done;
    }
    for (index = 0; index < current.count; index++) {
        struct stat live;
        if (fstat(retained->descriptors[index], &live) != 0) {
            goto done;
        }
        if (!same_identity(&live, &retained->identities[index]) ||
            !same_identity(&live, &current.identities[index])) {
            errno = ESTALE;
            goto done;
        }
    }
    result = 0;
done:
    saved_error = errno;
    for (index = 0; index < current.count; index++) {
        close(current.descriptors[index]);
    }
    errno = saved_error;
    return result;
}

#include "domus_auth_probe.h"

static int probe_connect(struct vfs_handle_struct *handle,
                         const char *service, const char *user)
{
    struct probe_root *root;
    const char *run = getenv("DOMUS_SMB_LAB_RUN");
    const char *path = NULL;
    size_t index;
    void *owned;
    if (strcmp(service, "IPC$") == 0) {
        return SMB_VFS_NEXT_CONNECT(handle, service, user);
    }
    if (run == NULL || strlen(run) != 32) {
        errno = EINVAL;
        return -1;
    }
    if (strcmp(service, "media") == 0 || strcmp(service, "media-alias") == 0) {
        path = "/fixture/payload";
    } else if (strcmp(service, "staging") == 0) {
        path = "/fixture/payload/staging";
    } else if (strcmp(service, "private-alias") == 0) {
        path = "/fixture/private";
    } else {
        errno = EINVAL;
        return -1;
    }
    root = talloc_zero(handle, struct probe_root);
    if (root == NULL) {
        errno = ENOMEM;
        return -1;
    }
    root->path = path;
    root->role = strcmp(service, "media") == 0 ? 0 :
                 strcmp(service, "media-alias") == 0 ? 1 :
                 strcmp(service, "staging") == 0 ? 2 : 3;
    root->control = -1;
    for (index = 0; index < 16; index++) {
        unsigned int pair;
        if (!isxdigit((unsigned char)run[index * 2]) ||
            !isxdigit((unsigned char)run[index * 2 + 1]) ||
            sscanf(run + index * 2, "%2x", &pair) != 1) {
            TALLOC_FREE(root);
            errno = EINVAL;
            return -1;
        }
        root->revision[index] = pair;
    }
    if (open_chain(root) != 0) {
        owned = root;
        close_root(&owned);
        return -1;
    }
    root->control = openat(root->descriptors[1], "tools",
                           O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (root->control < 0) {
        owned = root;
        close_root(&owned);
        return -1;
    }
    SMB_VFS_HANDLE_SET_DATA(handle, root, close_root, struct probe_root,
                            return -1);
    {
        int result = SMB_VFS_NEXT_CONNECT(handle, service, user);
        if (result != 0) {
            SMB_VFS_HANDLE_FREE_DATA(handle);
        }
        return result;
    }
}

static int pause_before_child(const struct probe_root *root)
{
    struct timespec interval = {.tv_nsec = 100000000};
    unsigned int attempt;
    DEBUG(0, ("DOMUS_NATIVE_PROBE_PAUSED\n"));
    for (attempt = 0; attempt < 50; attempt++) {
        struct stat release;
        if (fstatat(root->control, "release-parent-probe", &release,
                    AT_SYMLINK_NOFOLLOW) == 0) {
            if (!S_ISREG(release.st_mode)) {
                errno = EINVAL;
                return -1;
            }
            return 0;
        }
        if (errno != ENOENT || nanosleep(&interval, NULL) != 0) {
            return -1;
        }
    }
    errno = ETIMEDOUT;
    return -1;
}

static NTSTATUS probe_fsctl(struct vfs_handle_struct *handle,
                            struct files_struct *fsp, TALLOC_CTX *context,
                            uint32_t function, uint16_t flags,
                            const uint8_t *input, uint32_t input_length,
                            uint8_t **output, uint32_t output_bound,
                            uint32_t *output_length)
{
    struct probe_root *root;
    struct stat parent, child;
    uint64_t parent_mount, child_mount;
    uint16_t name_length, action;
    char name[256];
    uint8_t *reply;
    size_t index;
    int parent_descriptor, child_descriptor = -1, error = 0;
    uint32_t result = 0;
    ssize_t length = 0;
    if (function != DOMUS_PROBE_CONTROL) {
        if (function == DOMUS_AUTH_CONTROL) {
            return auth_fsctl(handle, fsp, context, input, input_length,
                               output, output_bound, output_length);
        }
        return SMB_VFS_NEXT_FSCTL(handle, fsp, context, function, flags,
                                  input, input_length, output, output_bound,
                                  output_length);
    }
    if (auth_enabled()) {
        return NT_STATUS_NOT_SUPPORTED;
    }
    SMB_VFS_HANDLE_GET_DATA(handle, root, struct probe_root,
                            return NT_STATUS_INVALID_HANDLE);
    if (input_length < DOMUS_PROBE_REQUEST || input_length > 4096 ||
        output_bound < DOMUS_PROBE_HEADER + DOMUS_PROBE_MARKER ||
        memcmp(input, "DRP1", 4) != 0 || SVAL(input, 4) != 1 ||
        memcmp(input + 8, root->revision, 16) != 0) {
        return NT_STATUS_INVALID_PARAMETER;
    }
    action = SVAL(input, 6);
    name_length = SVAL(input, 48);
    if ((action != 1 && action != 2) || name_length == 0 ||
        name_length > 255 || input_length != DOMUS_PROBE_REQUEST + name_length ||
        ((BVAL(input, 50) == 0) != (BVAL(input, 58) == 0)) ||
        root->sequence == UINT64_MAX || BVAL(input, 40) != root->sequence + 1) {
        return NT_STATUS_INVALID_PARAMETER;
    }
    for (index = 0; index < name_length; index++) {
        uint8_t value = input[DOMUS_PROBE_REQUEST + index];
        if (value < 32 || value > 126 || value == '/' || value == '\\' ||
            value == ':') {
            return NT_STATUS_INVALID_PARAMETER;
        }
        name[index] = value;
    }
    name[name_length] = '\0';
    if (strcmp(name, ".") == 0 || strcmp(name, "..") == 0) {
        return NT_STATUS_INVALID_PARAMETER;
    }
    root->sequence++;
    if (!(fsp->access_mask & SEC_DIR_LIST)) {
        return NT_STATUS_ACCESS_DENIED;
    }
    reply = talloc_zero_array(context, uint8_t,
                              DOMUS_PROBE_HEADER + DOMUS_PROBE_MARKER + 1);
    if (reply == NULL) {
        return NT_STATUS_NO_MEMORY;
    }
    memcpy(reply, "DRO1", 4);
    SSVAL(reply, 4, 1);
    memcpy(reply + 16, root->revision, 16);
    memcpy(reply + 32, input + 24, 16);
    SBVAL(reply, 48, root->sequence);
    /* This borrowed native parent is the I/O authority, not its stored name. */
    parent_descriptor = fsp_get_pathref_fd(fsp);
    if (fstat(parent_descriptor, &parent) != 0) {
        error = errno;
        goto refused;
    }
    SBVAL(reply, 56, parent.st_dev);
    SBVAL(reply, 64, parent.st_ino);
    if (!S_ISDIR(parent.st_mode) ||
        !same_identity(&parent, &root->identities[root->count - 1])) {
        error = ESTALE;
        goto refused;
    }
    if (inspect_mount(parent_descriptor, &parent, &parent_mount) != 0 ||
        binding_matches(root) != 0) {
        error = errno;
        goto refused;
    }
    if (parent_mount != root->mount_id) {
        error = ESTALE;
        goto refused;
    }
    if (action == 2 && pause_before_child(root) != 0) {
        error = errno;
        goto refused;
    }
    child_descriptor = openat(parent_descriptor, name,
                               O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (child_descriptor < 0 || fstat(child_descriptor, &child) != 0) {
        error = errno;
        goto refused;
    }
    SBVAL(reply, 72, child.st_dev);
    SBVAL(reply, 80, child.st_ino);
    if (inspect_mount(child_descriptor, &child, &child_mount) != 0) {
        error = errno;
        goto refused;
    }
    if (child_mount != parent_mount) {
        error = EXDEV;
        goto refused;
    }
    if (BVAL(input, 50) != 0 &&
        (BVAL(input, 50) != child.st_dev || BVAL(input, 58) != child.st_ino)) {
        error = ESTALE;
        goto refused;
    }
    if (!S_ISREG(child.st_mode)) {
        error = EINVAL;
        goto refused;
    }
    if (child.st_size < 0 || child.st_size > DOMUS_PROBE_MARKER) {
        error = EMSGSIZE;
        goto refused;
    }
    length = read(child_descriptor, reply + DOMUS_PROBE_HEADER,
                   DOMUS_PROBE_MARKER + 1);
    if (length < 0) {
        error = errno;
        goto refused;
    }
    if (length != child.st_size) {
        error = EIO;
        goto refused;
    }
    if (binding_matches(root) != 0) {
        error = errno;
        goto refused;
    }
    SSVAL(reply, 88, length);
    goto done;
refused:
    result = 1;
    length = 0;
    memset(reply + DOMUS_PROBE_HEADER, 0, DOMUS_PROBE_MARKER + 1);
done:
    if (child_descriptor >= 0) {
        close(child_descriptor);
    }
    SIVAL(reply, 8, result);
    SIVAL(reply, 12, error);
    *output = reply;
    *output_length = DOMUS_PROBE_HEADER + length;
    return NT_STATUS_OK;
}

static struct vfs_fn_pointers probe_functions = {
    .connect_fn = probe_connect,
    .fsctl_fn = probe_fsctl,
};

NTSTATUS domus_probe_init(TALLOC_CTX *context);

NTSTATUS domus_probe_init(TALLOC_CTX *context)
{
    (void)context;
    if (auth_enabled() && initialize_signer() != 0) {
        return NT_STATUS_ACCESS_DENIED;
    }
    return smb_register_vfs(SMB_VFS_INTERFACE_VERSION, "domus_probe",
                            &probe_functions);
}
