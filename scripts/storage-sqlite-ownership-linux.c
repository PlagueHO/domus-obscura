/* Isolated native context owned by Rust, not a production callback registry. */
#define main domus_legacy_main
#include "probe.c"
#undef main
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <stddef.h>
#include <sys/prctl.h>

static Root *ownership_root;
static int ownership_released;
static int ownership_pins_closed;
static int ownership_quarantined;
static int ownership_refused_opens;
static int ownership_fault_descriptor = -1;
static struct stat ownership_fault_identity;

static int ownership_open(sqlite3_vfs *vfs, sqlite3_filename path,
    sqlite3_file *file, int flags, int *actual) {
    PROBE_LOCK();
    if (native_close_errors) {
        file->pMethods = NULL;
        ownership_refused_opens++;
        PROBE_UNLOCK();
        return SQLITE_IOERR_CLOSE;
    }
    PROBE_UNLOCK();
    return vfs_open(vfs, path, file, flags, actual);
}

void *domus_probe_root_new(const char *path) {
    char copy[4096];
    require(!ownership_root && !active && !native_close_errors &&
        path[0] == '/' && strlen(path) < sizeof(copy),
        "one bounded fresh Rust-owned native context");
    strcpy(copy, path);
    char *name = strrchr(copy, '/');
    require(name && name > copy && name[1], "generated native root has a bounded parent and leaf");
    *name++ = 0;
    int parent = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    require(parent >= 0, "native filesystem anchor");
    char *save = NULL;
    for (char *part = strtok_r(copy + 1, "/", &save); part;
        part = strtok_r(NULL, "/", &save)) {
        require(strcmp(part, ".") && strcmp(part, "..") && strlen(part) <= 255,
            "bounded no-follow parent component");
        int next = openat(parent, part, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        require(next >= 0 && close(parent) == 0, "native parent component traversal");
        parent = next;
    }
    ownership_root = malloc(sizeof(*ownership_root));
    require(ownership_root != NULL, "native context allocation");
    install(ownership_root, parent, name);
    ownership_root->vfs.xOpen = ownership_open;
    require(enable_temporary(ownership_root), "Rust-owned private native temporary root");
    ownership_pins_closed = 0;
    return ownership_root;
}

int domus_probe_root_live(void *root) {
    struct stat identity;
    PROBE_LOCK();
    int valid = root && root == ownership_root && active == ownership_root &&
        !fstat(ownership_root->directory, &identity) &&
        same(&identity, &ownership_root->identity) && temporary_binding(ownership_root);
    PROBE_UNLOCK();
    return valid;
}

int domus_probe_pins_live(void) {
    return domus_probe_root_live(ownership_root);
}

int domus_probe_pins_closed(void) {
    return ownership_pins_closed;
}

static void ownership_release(void) {
    int descriptors[] = {
        ownership_root->parent, ownership_root->directory, ownership_root->temporary
    };
    uninstall(ownership_root);
    require(close(descriptors[0]) == 0, "owned parent descriptor closes");
    for (unsigned int i = 0; i < sizeof(descriptors) / sizeof(descriptors[0]); i++) {
        errno = 0;
        require(fcntl(descriptors[i], F_GETFD) == -1 && errno == EBADF,
            "actual native authority descriptor is closed");
    }
    ownership_pins_closed = 1;
    free(ownership_root);
    ownership_root = NULL;
    ownership_released++;
}

int domus_probe_root_drop(void *root) {
    require(domus_probe_root_live(root), "native root remains valid through final cleanup");
    require(temp_native_opens == temp_closes,
        "all actual temporary file callbacks finish before native context destruction");
    if (native_close_errors) {
        ownership_quarantined = 1;
        return SQLITE_IOERR_CLOSE;
    }
    require(!temp_close_errors, "healthy file-method cleanup");
    temporary_empty(ownership_root);
    ownership_release();
    return SQLITE_OK;
}

sqlite3 *domus_probe_connection(void *root) {
    require(domus_probe_root_live(root), "connection borrows actual Rust-owned root");
    return open_db("state.sqlite", ownership_root->vfs.zName);
}

void domus_probe_seed(sqlite3 *db) {
    execute(db, "PRAGMA journal_mode=WAL;"
        "CREATE TABLE values_table(value INTEGER PRIMARY KEY);"
        "INSERT INTO values_table VALUES(1);");
    temporary_seed(db);
}

int domus_probe_temporary_live(void) {
    return temp_native_opens - temp_closes;
}

int domus_probe_roots_released(void) {
    return ownership_released;
}

int domus_probe_native_close_errors(void) {
    return native_close_errors;
}

void domus_probe_protect_temporary(sqlite3 *db) {
    sqlite3_file *file = NULL;
    require(ownership_fault_descriptor == -1 &&
        sqlite3_file_control(db, "temp", SQLITE_FCNTL_FILE_POINTER, &file) == SQLITE_OK &&
        file && !strcmp(((TemporaryFile *)file)->name, observed_temp_name) &&
        observed_temp_descriptor >= 0 &&
        !fstat(observed_temp_descriptor, &ownership_fault_identity) &&
        S_ISREG(ownership_fault_identity.st_mode) && ownership_fault_identity.st_size > 0,
        "select the exact native descriptor returned for this owned TEMP facade");
    ownership_fault_descriptor = observed_temp_descriptor;
    /* This fixture thread cannot remove its filter or admit another context. */
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_close, 0, 3),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, args[0])),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, (unsigned int)ownership_fault_descriptor, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)
    };
    struct sock_fprog program = { sizeof(filter) / sizeof(filter[0]), filter };
    require(prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0 &&
        prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) == 0,
        "restrict only this fixture thread's close syscall on the exact owned descriptor");
}

int domus_probe_verify_quarantine(void) {
    struct stat identity;
    require(ownership_quarantined && native_close_errors == 1 &&
        !temp_close_errors && domus_probe_root_live(ownership_root) &&
        !fstat(ownership_fault_descriptor, &identity) &&
        same(&identity, &ownership_fault_identity),
        "native bridge catches retained descriptor even when xClose returns success");
    sqlite3 *other = NULL;
    int refused = ownership_refused_opens, opened = native_bridge_calls;
    int result = sqlite3_open_v2("state.sqlite", &other,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW, ownership_root->vfs.zName);
    require(result != SQLITE_OK && other &&
        sqlite3_extended_errcode(other) == SQLITE_IOERR_CLOSE &&
        ownership_refused_opens > refused && native_bridge_calls == opened,
        "quarantined context refuses new SQLite admission before native open");
    require(sqlite3_close(other) == SQLITE_OK, "refused quarantine connection closes");
    return SQLITE_IOERR_CLOSE;
}

void domus_probe_recover_fixture(void) {
    struct stat identity;
    require(ownership_quarantined && ownership_fault_descriptor >= 0 &&
        !fstat(ownership_fault_descriptor, &identity) &&
        same(&identity, &ownership_fault_identity),
        "explicit recovery still owns the exact syscall-refused fixture descriptor");
    require(syscall(SYS_close_range, (unsigned int)ownership_fault_descriptor,
        (unsigned int)ownership_fault_descriptor, 0) == 0,
        "exact fixture close_range confirms native release without retrying close");
    errno = 0;
    require(fcntl(ownership_fault_descriptor, F_GETFD) == -1 && errno == EBADF,
        "failed fixture descriptor is actually closed");
    ownership_fault_descriptor = -1;
    ownership_release();
}

#ifdef DOMUS_SQLITE_CONCURRENT
#include "storage-sqlite-independent-native.h"
#endif
