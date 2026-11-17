/* Isolated SQLite 3.53.2 I/O experiment. Test hooks are not a production VFS. */
#define _GNU_SOURCE
#include "sqlite3.h"
#ifdef DOMUS_SQLITE_CONCURRENT
#include "storage-sqlite-probe-lock.h"
#else
#define PROBE_LOCK() ((void)0)
#define PROBE_UNLOCK() ((void)0)
#define PROBE_ASSERT_INCREMENT(value) ((value)++)
#endif
#include <errno.h>
#include <fcntl.h>
#include <dirent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
#include "storage-sqlite-native-io.h"
#endif

typedef struct {
    int parent;
    int directory;
    int invalid;
    struct stat identity;
    char name[64];
    char anchor[128];
    int temporary;
    int temporary_invalid;
    struct stat temporary_identity;
    char temporary_anchor[128];
    sqlite3_vfs vfs;
} Root;

static Root *active;
static sqlite3_vfs *base;
static int (*original_open)(const char *, int, mode_t);
static int (*original_unlink)(const char *);
static int (*original_close)(int) = close;
static int observed_original = -1;
static int observed_replacement = -1;
static int original_sidecars;
static int replacement_sidecars;
static int assertions;
static int native_bridge_calls;
static int native_close_errors;
static int observed_temp_descriptor = -1;
static char observed_temp_name[4096];

#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
static int plain_open(const char *path, int flags, mode_t mode) {
    return open(path, flags, mode);
}
#endif

static void fail(const char *operation) {
    fprintf(stderr, "%s failed: errno=%d (%s)\n", operation, errno, strerror(errno));
    exit(1);
}

static void require(int condition, const char *label) {
    if (!condition) {
        fprintf(stderr, "assertion failed: %s\n", label);
        exit(1);
    }
    PROBE_ASSERT_INCREMENT(assertions);
}

static void sql_ok(sqlite3 *db, int result, const char *operation) {
    if (result != SQLITE_OK) {
        fprintf(stderr, "%s: sqlite=%d; %s\n", operation, result, sqlite3_errmsg(db));
        exit(1);
    }
}

static void execute(sqlite3 *db, const char *sql) {
    sql_ok(db, sqlite3_exec(db, sql, NULL, NULL, NULL), sql);
}

#include "storage-sqlite-temp-workload.h"

static int same(const struct stat *a, const struct stat *b) {
    return a->st_dev == b->st_dev && a->st_ino == b->st_ino;
}

static int leaf(const char *name) {
    return name && (!strcmp(name, "state.sqlite") ||
        !strcmp(name, "state.sqlite-wal") || !strcmp(name, "state.sqlite-shm") ||
        !strcmp(name, "state.sqlite-journal"));
}

static const char *relative(const Root *root, const char *path) {
    size_t size = strlen(root->anchor);
    if (strncmp(path, root->anchor, size) || path[size] != '/') return NULL;
    return leaf(path + size + 1) ? path + size + 1 : NULL;
}

static const char *temporary_relative(const Root *root, const char *path) {
    if (root->temporary < 0) return NULL;
    size_t length = strlen(root->temporary_anchor);
    if (strncmp(path, root->temporary_anchor, length) || path[length] != '/') return NULL;
    return temporary_leaf(path + length + 1) ? path + length + 1 : NULL;
}

static int temporary_binding(Root *root) {
    struct stat state, current;
    if (root->invalid ||
        fstatat(root->parent, root->name, &state, AT_SYMLINK_NOFOLLOW) ||
        !same(&state, &root->identity) || !S_ISDIR(state.st_mode)) {
        root->invalid = root->temporary_invalid = 1;
        return 0;
    }
    if (root->temporary_invalid || root->temporary < 0 ||
        fstatat(root->directory, "sqlite-tmp", &current, AT_SYMLINK_NOFOLLOW) ||
        !same(&current, &root->temporary_identity) || !S_ISDIR(current.st_mode) ||
        current.st_uid != geteuid() || (current.st_mode & 0777) != 0700) {
        root->temporary_invalid = 1;
        return 0;
    }
    return 1;
}

static void observe(int file, const char *path) {
    const char *name = strrchr(path, '/');
    if (!name || (!strstr(name, "-wal") && !strstr(name, "-shm") &&
        !strstr(name, "-journal"))) return;
    struct stat actual, expected;
    if (fstat(file, &actual)) fail("opened sidecar metadata");
    if (observed_original >= 0 &&
        !fstatat(observed_original, name + 1, &expected, AT_SYMLINK_NOFOLLOW) &&
        same(&actual, &expected)) original_sidecars++;
    if (observed_replacement >= 0 &&
        !fstatat(observed_replacement, name + 1, &expected, AT_SYMLINK_NOFOLLOW) &&
        same(&actual, &expected)) replacement_sidecars++;
}

static int probe_open(const char *path, int flags, mode_t mode) {
    int file;
    if (active) {
        const char *name = relative(active, path);
        const char *temp_name = temporary_relative(active, path);
        if (!strcmp(path, active->anchor)) {
            file = fcntl(active->directory, F_DUPFD_CLOEXEC, 3);
        } else if (temp_name) {
            if (!temporary_binding(active) ||
                (flags & (O_CREAT | O_EXCL | O_RDWR)) != (O_CREAT | O_EXCL | O_RDWR)) {
                errno = EACCES;
                return -1;
            }
            file = openat(active->temporary, temp_name,
                flags | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, mode);
            if (file >= 0) {
                struct stat actual, named;
                if (fstat(file, &actual) || !S_ISREG(actual.st_mode) ||
                    fstatat(active->temporary, temp_name, &named, AT_SYMLINK_NOFOLLOW) ||
                    !same(&actual, &named)) {
                    close(file);
                    errno = EINVAL;
                    return -1;
                }
                temp_native_opens++;
                require(strlen(path) < sizeof(observed_temp_name), "bounded observed temporary name");
                strcpy(observed_temp_name, path);
                observed_temp_descriptor = file;
            }
        } else if (name) {
            file = openat(active->directory, name,
                flags | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, mode);
            if (file >= 0) {
                struct stat metadata;
                if (fstat(file, &metadata) || !S_ISREG(metadata.st_mode)) {
                    close(file);
                    errno = EINVAL;
                    return -1;
                }
            }
        } else if (!strcmp(path, "/dev/urandom") && !(flags & (O_WRONLY | O_RDWR))) {
            file = original_open(path, flags, mode);
        } else {
            namespace_native_refusals++;
            errno = EACCES;
            return -1;
        }
    } else {
        file = original_open(path, flags, mode);
    }
    if (file >= 0) observe(file, path);
    return file;
}

static int probe_unlink(const char *path) {
    if (!active) return original_unlink(path);
    const char *name = temporary_relative(active, path);
    int directory = active->temporary;
    if (!name) {
        name = relative(active, path);
        directory = active->directory;
    }
    struct stat metadata;
    if (!name || fstatat(directory, name, &metadata, AT_SYMLINK_NOFOLLOW) ||
        !S_ISREG(metadata.st_mode)) {
        errno = EACCES;
        return -1;
    }
    return unlinkat(directory, name, 0);
}

static int probe_close(int file) {
    int result = original_close(file);
    int error = errno;
    if (result != 0) native_close_errors++;
    errno = error;
    return result;
}

#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
int sqlite3_domus_native_open_v1(const char *path, int flags, int mode) {
    PROBE_LOCK();
#ifdef DOMUS_SQLITE_CONCURRENT
    if (native_close_errors) {
        errno = EIO;
        PROBE_UNLOCK();
        return -1;
    }
#endif
    native_bridge_calls++;
    int result = probe_open(path, flags, (mode_t)mode);
    int error = errno;
    PROBE_UNLOCK();
    errno = error;
    return result;
}
int sqlite3_domus_native_unlink_v1(const char *path) {
    PROBE_LOCK();
    int result = probe_unlink(path);
    int error = errno;
    PROBE_UNLOCK();
    errno = error;
    return result;
}
int sqlite3_domus_native_close_v1(int file) {
    PROBE_LOCK();
    int result = probe_close(file);
    int error = errno;
    PROBE_UNLOCK();
    errno = error;
    return result;
}
#endif

static int full_path_inner(sqlite3_vfs *vfs, const char *name, int capacity, char *out) {
    Root *root = vfs->pAppData;
    struct stat current;
    if (root->invalid || strcmp(name, "state.sqlite")) return SQLITE_CANTOPEN;
    if (fstatat(root->parent, root->name, &current, AT_SYMLINK_NOFOLLOW) ||
        !S_ISDIR(current.st_mode) || !same(&current, &root->identity)) {
        root->invalid = 1;
        return SQLITE_CANTOPEN;
    }
    int size = snprintf(out, (size_t)capacity, "%s/state.sqlite", root->anchor);
    return size >= 0 && size < capacity ? SQLITE_OK : SQLITE_CANTOPEN;
}

static int full_path(sqlite3_vfs *vfs, const char *name, int capacity, char *out) {
    PROBE_LOCK();
    int result = full_path_inner(vfs, name, capacity, out);
    PROBE_UNLOCK();
    return result;
}

static int vfs_open(sqlite3_vfs *vfs, sqlite3_filename path, sqlite3_file *file,
    int flags, int *actual) {
    Root *root = vfs->pAppData;
    if (!entry_filename(path, flags)) return SQLITE_CANTOPEN;
    if (!path) {
        PROBE_LOCK();
        int valid = temporary_binding(root);
        PROBE_UNLOCK();
        if (!valid) return SQLITE_CANTOPEN;
        return temporary_open(base, root->temporary_anchor, '/', file, flags, actual);
    }
    if (!relative(root, path)) return SQLITE_CANTOPEN;
    return base->xOpen(base, path, file, flags | SQLITE_OPEN_NOFOLLOW, actual);
}

static int vfs_access(sqlite3_vfs *vfs, const char *path, int flags, int *out) {
    Root *root = vfs->pAppData;
    const char *name = relative(root, path);
    *out = 0;
    if (!name) return SQLITE_IOERR_ACCESS;
    struct stat metadata;
    if (fstatat(root->directory, name, &metadata, AT_SYMLINK_NOFOLLOW)) {
        return errno == ENOENT ? SQLITE_OK : SQLITE_IOERR_ACCESS;
    }
    if (!S_ISREG(metadata.st_mode)) return SQLITE_IOERR_ACCESS;
    if (flags == SQLITE_ACCESS_EXISTS) {
        *out = 1;
        return SQLITE_OK;
    }
    if (flags != SQLITE_ACCESS_READWRITE && flags != SQLITE_ACCESS_READ)
        return SQLITE_IOERR_ACCESS;
    /* Opening and closing this inode could release SQLite's POSIX locks. */
    if (!syscall(SYS_faccessat2, root->directory, name,
        flags == SQLITE_ACCESS_READWRITE ? R_OK | W_OK : R_OK,
        AT_EACCESS | AT_SYMLINK_NOFOLLOW)) {
        *out = 1;
        return SQLITE_OK;
    }
    return errno == EACCES || errno == EPERM || errno == EROFS || errno == ENOENT
        ? SQLITE_OK : SQLITE_IOERR_ACCESS;
}

static int vfs_delete(sqlite3_vfs *vfs, const char *path, int sync) {
    Root *root = vfs->pAppData;
    const char *name = relative(root, path);
    if (!name) return SQLITE_IOERR_DELETE;
    struct stat metadata;
    if (fstatat(root->directory, name, &metadata, AT_SYMLINK_NOFOLLOW)) {
        return errno == ENOENT ? SQLITE_OK : SQLITE_IOERR_DELETE;
    }
    if (!S_ISREG(metadata.st_mode) || unlinkat(root->directory, name, 0))
        return SQLITE_IOERR_DELETE;
    return sync && fsync(root->directory) ? SQLITE_IOERR_DIR_FSYNC : SQLITE_OK;
}

static void install(Root *root, int parent, const char *name) {
    memset(root, 0, sizeof(*root));
    root->temporary = -1;
    root->parent = parent;
    require(strlen(name) < sizeof(root->name), "bounded root name");
    strcpy(root->name, name);
    root->directory = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (root->directory < 0 || fstat(root->directory, &root->identity)) fail("root open");
    struct statfs filesystem;
    if (fstatfs(root->directory, &filesystem)) fail("root filesystem query");
    require(filesystem.f_type == 0xef53, "actual ext4 fixture");
    snprintf(root->anchor, sizeof(root->anchor), "/proc/self/fd/%d", root->directory);
    base = sqlite3_vfs_find("unix");
#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base && base->iVersion >= 2, "upstream Unix VFS with WAL methods");
    original_open = plain_open;
    original_unlink = unlink;
#else
    require(base && base->iVersion >= 3 && base->xSetSystemCall &&
        base->xGetSystemCall, "pinned Unix test hooks available");
    original_open = (int (*)(const char *, int, mode_t))base->xGetSystemCall(base, "open");
    original_unlink = (int (*)(const char *))base->xGetSystemCall(base, "unlink");
    original_close = (int (*)(int))base->xGetSystemCall(base, "close");
    require(original_open != NULL, "actual original open hook");
    require(original_unlink != NULL, "actual original unlink hook");
    require(original_close != NULL, "actual original close hook");
#endif
    root->vfs = *base;
    root->vfs.zName = "domus_root_probe";
    root->vfs.pNext = NULL;
    root->vfs.pAppData = root;
    root->vfs.xFullPathname = full_path;
    root->vfs.xOpen = vfs_open;
    root->vfs.xAccess = vfs_access;
    root->vfs.xDelete = vfs_delete;
    if (root->vfs.szOsFile < (int)sizeof(TemporaryFile))
        root->vfs.szOsFile = sizeof(TemporaryFile);
    active = root;
#ifndef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base->xSetSystemCall(base, "open", (sqlite3_syscall_ptr)probe_open) == SQLITE_OK,
        "test open hook installed");
    require(base->xSetSystemCall(base, "unlink", (sqlite3_syscall_ptr)probe_unlink) == SQLITE_OK,
        "test temporary unlink hook installed");
    require(base->xSetSystemCall(base, "close", (sqlite3_syscall_ptr)probe_close) == SQLITE_OK,
        "test native close observation installed");
#endif
    require(sqlite3_vfs_register(&root->vfs, 0) == SQLITE_OK, "scoped test VFS registered");
}

static void uninstall(Root *root) {
    require(sqlite3_vfs_unregister(&root->vfs) == SQLITE_OK, "test VFS unregistered");
#ifndef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base->xSetSystemCall(base, "open", (sqlite3_syscall_ptr)original_open) == SQLITE_OK,
        "original open hook restored");
    require(base->xSetSystemCall(base, "unlink", (sqlite3_syscall_ptr)original_unlink) == SQLITE_OK,
        "original unlink hook restored");
    require(base->xSetSystemCall(base, "close", (sqlite3_syscall_ptr)original_close) == SQLITE_OK,
        "original close hook restored");
#endif
    active = NULL;
    if (root->temporary >= 0) close(root->temporary);
    close(root->directory);
}

static sqlite3 *open_db(const char *path, const char *vfs) {
    sqlite3 *db = NULL;
    int flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOFOLLOW;
    int result = active ? entry_open(path, &db, flags, vfs)
        : sqlite3_open_v2(path, &db, flags, vfs);
    sql_ok(db, result, "database open");
    sql_ok(db, sqlite3_busy_timeout(db, 5000), "busy timeout");
    return db;
}

static void close_db(sqlite3 *db) { sql_ok(db, sqlite3_close(db), "database close"); }

static int count_rows(sqlite3 *db) {
    sqlite3_stmt *statement = NULL;
    sql_ok(db, sqlite3_prepare_v2(db, "SELECT count(*) FROM values_table", -1,
        &statement, NULL), "row count prepare");
    require(sqlite3_step(statement) == SQLITE_ROW, "row count returned");
    int count = sqlite3_column_int(statement, 0);
    sql_ok(db, sqlite3_finalize(statement), "row count finalize");
    return count;
}

static void seed(sqlite3 *db) {
    execute(db, "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;");
    execute(db, "CREATE TABLE values_table(value INTEGER PRIMARY KEY);");
    execute(db, "INSERT INTO values_table VALUES(1);");
}

static int make_directory(int parent, const char *name) {
    if (mkdirat(parent, name, 0700)) fail("fresh directory creation");
    int result = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (result < 0) fail("fresh directory open");
    return result;
}

static int enable_temporary(Root *root) {
    if (mkdirat(root->directory, "sqlite-tmp", 0700) && errno != EEXIST) return 0;
    root->temporary = openat(root->directory, "sqlite-tmp",
        O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (root->temporary < 0) return 0;
    if (fstat(root->temporary, &root->temporary_identity)) return 0;
    snprintf(root->temporary_anchor, sizeof(root->temporary_anchor),
        "/proc/self/fd/%d", root->temporary);
    return temporary_binding(root);
}

static void temporary_empty(Root *root) {
    DIR *entries = fdopendir(openat(root->temporary, ".", O_RDONLY | O_DIRECTORY | O_CLOEXEC));
    require(entries != NULL, "temporary cleanup observation");
    int remaining = 0;
    struct dirent *entry;
    errno = 0;
    while ((entry = readdir(entries)))
        if (strcmp(entry->d_name, ".") && strcmp(entry->d_name, "..")) remaining++;
    int error = errno;
    require(closedir(entries) == 0 && !error && remaining == 0,
        "no temporary files remain after close or process interruption");
}

static void refuse_reopen(void) {
    sqlite3 *db = NULL;
    int result = sqlite3_open_v2("state.sqlite", &db,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOFOLLOW, "domus_root_probe");
    require((result & 0xff) == SQLITE_CANTOPEN, "stale root refuses a new connection");
    if (db) close_db(db);
}

static void baseline(int parent, const char *directory) {
    int original = make_directory(parent, "baseline");
    char path[4096];
    require(snprintf(path, sizeof(path), "%s/baseline/state.sqlite", directory) <
        (int)sizeof(path), "bounded baseline path");
    sqlite3 *db = open_db(path, NULL);
    seed(db);
    close_db(db);
    db = open_db(path, NULL);
    if (renameat(parent, "baseline", parent, "baseline-held")) fail("baseline parent rename");
    int replacement = make_directory(parent, "baseline");
    if (linkat(original, "state.sqlite", replacement, "state.sqlite", 0))
        fail("generated baseline file alias");
    observed_original = original;
    observed_replacement = replacement;
    original_sidecars = replacement_sidecars = 0;
    base = sqlite3_vfs_find("unix");
#ifndef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base && base->iVersion >= 3 && base->xGetSystemCall &&
        base->xSetSystemCall, "baseline trace hooks available");
    original_open = (int (*)(const char *, int, mode_t))base->xGetSystemCall(base, "open");
    require(original_open != NULL, "baseline original open hook");
    require(base->xSetSystemCall(base, "open", (sqlite3_syscall_ptr)probe_open) == SQLITE_OK,
        "baseline trace installed");
#endif
    execute(db, "INSERT INTO values_table VALUES(2);");
    require(replacement_sidecars >= 2 && original_sidecars == 0,
        "baseline WAL and SHM open in the replacement parent");
    close_db(db);
#ifndef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base->xSetSystemCall(base, "open", (sqlite3_syscall_ptr)original_open) == SQLITE_OK,
        "baseline trace restored");
#endif
    observed_original = observed_replacement = -1;
    close(original);
    close(replacement);
}

static void guarded(int parent) {
    int original = make_directory(parent, "guarded");
    Root root;
    install(&root, parent, "guarded");
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    seed(db);
    close_db(db);
    db = open_db("state.sqlite", root.vfs.zName);
    if (renameat(parent, "guarded", parent, "guarded-held")) fail("guarded parent rename");
    int replacement = make_directory(parent, "guarded");
    observed_original = original;
    observed_replacement = replacement;
    original_sidecars = replacement_sidecars = 0;
    execute(db, "INSERT INTO values_table VALUES(2);");
    require(original_sidecars >= 2 && replacement_sidecars == 0,
        "guarded WAL and SHM remain in the retained parent");
    require(count_rows(db) == 2, "guarded exact committed rows");
    close_db(db);
    refuse_reopen();
    struct stat metadata;
    errno = 0;
    require(fstatat(replacement, "state.sqlite", &metadata, AT_SYMLINK_NOFOLLOW) < 0 &&
        errno == ENOENT, "replacement database was not created");
    if (unlinkat(parent, "guarded", AT_REMOVEDIR) ||
        renameat(parent, "guarded-held", parent, "guarded")) fail("guarded restoration");
    refuse_reopen();
    observed_original = observed_replacement = -1;
    close(replacement);
    close(original);
    uninstall(&root);
    install(&root, parent, "guarded");
    db = open_db("state.sqlite", root.vfs.zName);
    require(count_rows(db) == 2, "fresh authority reads restored original");
    close_db(db);
    uninstall(&root);
}

static void symlink_case(int parent, const char *root_name, const char *file_name) {
    int directory = make_directory(parent, root_name);
    Root root;
    install(&root, parent, root_name);
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    seed(db);
    if (!strcmp(file_name, "state.sqlite-journal"))
        execute(db, "PRAGMA journal_mode=DELETE;");
    close_db(db);
    if (!strcmp(file_name, "state.sqlite") &&
        renameat(directory, "state.sqlite", directory, "original.sqlite"))
        fail("generated main-file relocation");
    char sentinel_name[128], target[132];
    snprintf(sentinel_name, sizeof(sentinel_name), "sentinel-%s", root_name);
    snprintf(target, sizeof(target), "../%s", sentinel_name);
    int sentinel = openat(parent, sentinel_name, O_CREAT | O_EXCL | O_RDWR | O_NOFOLLOW, 0600);
    if (sentinel < 0 || write(sentinel, "unchanged", 9) != 9) fail("sentinel creation");
    if (symlinkat(target, directory, file_name)) fail("generated file symlink");
    db = NULL;
    int result = sqlite3_open_v2("state.sqlite", &db,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW, root.vfs.zName);
    if (result == SQLITE_OK) result = sqlite3_exec(db,
        "INSERT INTO values_table VALUES(3)", NULL, NULL, NULL);
    require(result != SQLITE_OK, "actual main/sidecar symlink refuses database work");
    if (db) close_db(db);
    char data[10] = {0};
    require(pread(sentinel, data, 10, 0) == 9 && !strcmp(data, "unchanged"),
        "symlink target bytes unchanged");
    struct stat metadata;
    require(!fstatat(directory, file_name, &metadata, AT_SYMLINK_NOFOLLOW) &&
        S_ISLNK(metadata.st_mode), "refused file symlink was not removed");
    close(sentinel);
    close(directory);
    uninstall(&root);
}

static void invalid_types(int parent) {
    if (symlinkat("link-wal", parent, "root-link")) fail("generated root link");
    errno = 0;
    int file = openat(parent, "root-link", O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    require(file < 0 && (errno == ELOOP || errno == ENOTDIR), "root symlink refused");
    int directory = make_directory(parent, "fifo");
    if (mkfifoat(directory, "state.sqlite", 0600)) fail("generated FIFO");
    Root root;
    install(&root, parent, "fifo");
    sqlite3 *db = NULL;
    int result = sqlite3_open_v2("state.sqlite", &db,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW, root.vfs.zName);
    require((result & 0xff) == SQLITE_CANTOPEN, "FIFO refused without blocking or stream I/O");
    if (db) close_db(db);
    struct stat metadata;
    require(!fstatat(directory, "state.sqlite", &metadata, AT_SYMLINK_NOFOLLOW) &&
        S_ISFIFO(metadata.st_mode) && metadata.st_size == 0, "refused FIFO unchanged");
    uninstall(&root);
    close(directory);
}

static void worker(int parent, int ready, int go, int identity) {
    Root root;
    install(&root, parent, "guarded");
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    if (write(ready, "R", 1) != 1) fail("worker ready");
    char signal;
    if (read(go, &signal, 1) != 1) fail("worker start");
    sqlite3_stmt *statement = NULL;
    sql_ok(db, sqlite3_prepare_v2(db, "INSERT INTO values_table VALUES(?1)", -1,
        &statement, NULL), "worker prepare");
    for (int i = 0; i < 50; i++) {
        sql_ok(db, sqlite3_bind_int(statement, 1, identity * 1000 + i), "worker bind");
        require(sqlite3_step(statement) == SQLITE_DONE, "worker insert committed");
        sql_ok(db, sqlite3_reset(statement), "worker reset");
    }
    sql_ok(db, sqlite3_finalize(statement), "worker finalize");
    close_db(db);
    uninstall(&root);
    _exit(0);
}

static void concurrency(int parent) {
    require(sqlite3_shutdown() == SQLITE_OK, "SQLite shut down before fork");
    int ready[2], go[2];
    if (pipe(ready) || pipe(go)) fail("worker pipes");
    pid_t children[2];
    for (int i = 0; i < 2; i++) {
        children[i] = fork();
        if (children[i] < 0) fail("worker fork");
        if (!children[i]) worker(parent, ready[1], go[0], i + 1);
    }
    close(ready[1]);
    close(go[0]);
    for (int i = 0; i < 2; i++) {
        char signal;
        if (read(ready[0], &signal, 1) != 1 || signal != 'R') fail("worker readiness");
    }
    if (renameat(parent, "guarded", parent, "concurrent-held")) fail("concurrent parent rename");
    int replacement = make_directory(parent, "guarded");
    if (write(go[1], "GG", 2) != 2) fail("worker release");
    close(go[1]);
    close(ready[0]);
    for (int i = 0; i < 2; i++) {
        int status;
        if (waitpid(children[i], &status, 0) != children[i]) fail("worker wait");
        require(WIFEXITED(status) && WEXITSTATUS(status) == 0, "concurrent worker succeeded");
    }
    Root root;
    install(&root, parent, "concurrent-held");
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    require(count_rows(db) == 102, "all 100 concurrent rows committed with prior rows");
    sqlite3_stmt *query = NULL;
    sql_ok(db, sqlite3_prepare_v2(db, "SELECT count(*) FROM values_table WHERE "
        "value IN(1,2) OR value BETWEEN 1000 AND 1049 OR value BETWEEN 2000 AND 2049",
        -1, &query, NULL), "exact concurrent values prepare");
    require(sqlite3_step(query) == SQLITE_ROW && sqlite3_column_int(query, 0) == 102,
        "every expected concurrent primary key is present");
    sql_ok(db, sqlite3_finalize(query), "exact concurrent values finalize");
    struct stat metadata;
    errno = 0;
    require(fstatat(replacement, "state.sqlite", &metadata, AT_SYMLINK_NOFOLLOW) < 0 &&
        errno == ENOENT, "concurrent replacement remains empty");
    close_db(db);
    uninstall(&root);
    close(replacement);
}

static void rollback(int parent) {
    int original = make_directory(parent, "rollback");
    Root root;
    install(&root, parent, "rollback");
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    seed(db);
    execute(db, "PRAGMA journal_mode=DELETE;");
    if (renameat(parent, "rollback", parent, "rollback-held")) fail("rollback parent rename");
    int replacement = make_directory(parent, "rollback");
    observed_original = original;
    observed_replacement = replacement;
    original_sidecars = replacement_sidecars = 0;
    execute(db, "INSERT INTO values_table VALUES(2);");
    require(original_sidecars >= 1 && replacement_sidecars == 0,
        "rollback journal opens in retained parent only");
    require(count_rows(db) == 2, "rollback exact committed rows");
    close_db(db);
    struct stat metadata;
    errno = 0;
    require(fstatat(original, "state.sqlite-journal", &metadata, AT_SYMLINK_NOFOLLOW) < 0 &&
        errno == ENOENT, "normal journal cleanup uses original root");
    observed_original = observed_replacement = -1;
    uninstall(&root);
    close(original);
    close(replacement);
}

static void interrupted(int parent) {
    int directory = make_directory(parent, "interrupted");
    require(sqlite3_shutdown() == SQLITE_OK, "SQLite shut down before interrupted writer");
    pid_t child = fork();
    if (child < 0) fail("interrupted writer fork");
    if (!child) {
        Root root;
        install(&root, parent, "interrupted");
        sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
        seed(db);
        require(enable_temporary(&root), "interrupted writer native temporary authority");
        temporary_seed(db);
        sqlite3_int64 before = temp_other_bytes;
        sql_ok(db, temporary_sort(db), "interrupted writer actual file-backed sort");
        require(temp_other_bytes > before, "interrupted writer sort disk writes");
        execute(db, "INSERT INTO values_table VALUES(888);");
        execute(db, "BEGIN IMMEDIATE; INSERT INTO values_table VALUES(999);");
        sql_ok(db, sqlite3_db_cacheflush(db), "uncommitted WAL flush");
        _exit(23);
    }
    int status;
    if (waitpid(child, &status, 0) != child) fail("interrupted writer wait");
    require(WIFEXITED(status) && WEXITSTATUS(status) == 23,
        "writer exited without connection cleanup");
    struct stat metadata;
    require(!fstatat(directory, "state.sqlite-wal", &metadata, AT_SYMLINK_NOFOLLOW) &&
        S_ISREG(metadata.st_mode) && metadata.st_size > 0, "interrupted WAL remains");
    Root root;
    install(&root, parent, "interrupted");
    require(enable_temporary(&root), "fresh interrupted temporary authority");
    temporary_empty(&root);
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    require(count_rows(db) == 2, "recovery preserves only committed rows");
    sqlite3_stmt *statement = NULL;
    sql_ok(db, sqlite3_prepare_v2(db,
        "SELECT sum(value), max(value) FROM values_table", -1, &statement, NULL),
        "recovery values prepare");
    require(sqlite3_step(statement) == SQLITE_ROW &&
        sqlite3_column_int(statement, 0) == 889 &&
        sqlite3_column_int(statement, 1) == 888, "uncommitted row is absent after recovery");
    sql_ok(db, sqlite3_finalize(statement), "recovery values finalize");
    close_db(db);
    errno = 0;
    require(fstatat(directory, "state.sqlite-wal", &metadata, AT_SYMLINK_NOFOLLOW) < 0 &&
        errno == ENOENT, "recovery WAL cleanup remains in bound root");
    uninstall(&root);
    close(directory);
}

static void permission_lock(int parent) {
    int directory = make_directory(parent, "permission-lock");
    Root root;
    install(&root, parent, "permission-lock");
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    seed(db);
    execute(db, "PRAGMA journal_mode=DELETE;");
    close_db(db);
    uninstall(&root);
    require(sqlite3_shutdown() == SQLITE_OK, "SQLite shut down before lock contender");
    int go[2];
    if (pipe(go)) fail("lock contender pipe");
    pid_t child = fork();
    if (child < 0) fail("lock contender fork");
    if (!child) {
        close(go[1]);
        char signal;
        if (read(go[0], &signal, 1) != 1) fail("lock contender start");
        close(go[0]);
        Root contender;
        install(&contender, parent, "permission-lock");
        sqlite3 *second = open_db("state.sqlite", contender.vfs.zName);
        sql_ok(second, sqlite3_busy_timeout(second, 0), "immediate lock contender");
        int result = sqlite3_exec(second, "BEGIN IMMEDIATE", NULL, NULL, NULL);
        require((result & 0xff) == SQLITE_BUSY,
            "permission check preserves the other process's reserved lock");
        close_db(second);
        uninstall(&contender);
        _exit(0);
    }
    close(go[0]);
    install(&root, parent, "permission-lock");
    db = open_db("state.sqlite", root.vfs.zName);
    execute(db, "BEGIN IMMEDIATE;");
    char path[256];
    snprintf(path, sizeof(path), "%s/state.sqlite", root.anchor);
    int allowed = 0;
    require(root.vfs.xAccess(&root.vfs, path, SQLITE_ACCESS_READWRITE, &allowed) == SQLITE_OK &&
        allowed, "actual read/write permission check while transaction is locked");
    sqlite3 *sibling = open_db("state.sqlite", root.vfs.zName);
    require(count_rows(sibling) == 1, "same-process sibling reads committed rollback data");
    close_db(sibling);
    if (write(go[1], "G", 1) != 1) fail("lock contender release");
    close(go[1]);
    int status;
    if (waitpid(child, &status, 0) != child) fail("lock contender wait");
    require(WIFEXITED(status) && WEXITSTATUS(status) == 0,
        "permission query and SQLite sibling close preserve the reserved lock");
    execute(db, "ROLLBACK;");
    close_db(db);
    uninstall(&root);
    close(directory);
}

static void private_temporary(int parent) {
    int directory = make_directory(parent, "temporary");
    Root root;
    install(&root, parent, "temporary");
    require(enable_temporary(&root), "private native temporary root");
    sqlite3 *db = open_db("state.sqlite", root.vfs.zName);
    seed(db);
    temporary_seed(db);
    sqlite3_int64 before = temp_other_bytes;
    sql_ok(db, temporary_sort(db), "private file-backed sort");
    require(temp_other_bytes > before, "sort writes actual native temporary files");
    temporary_auxiliary(db);
    namespace_refusals(db, root.vfs.zName);
    temporary_busy_close(db);

    const char *collision = "etilqs_00000000000000000000000000000000";
    int sentinel = openat(directory, "sentinel", O_CREAT | O_EXCL | O_RDWR | O_NOFOLLOW, 0600);
    require(sentinel >= 0 && write(sentinel, "unchanged", 9) == 9, "temporary sentinel");
    if (symlinkat("../sentinel", root.temporary, collision)) fail("temporary file link");
    forced_temp_leaf = collision;
    require(temporary_sort(db) != SQLITE_OK, "actual sort refuses temporary-file symlink");
    forced_temp_leaf = NULL;
    char bytes[10] = {0};
    struct stat metadata;
    require(pread(sentinel, bytes, 10, 0) == 9 && !strcmp(bytes, "unchanged") &&
        !fstatat(root.temporary, collision, &metadata, AT_SYMLINK_NOFOLLOW) &&
        S_ISLNK(metadata.st_mode), "refused temporary link and target unchanged");
    require(!unlinkat(root.temporary, collision, 0), "remove exact generated test link");
    close(sentinel);
    sql_ok(db, temporary_sort(db), "sort recovers after refused new file");

    if (renameat(directory, "sqlite-tmp", directory, "sqlite-tmp-held"))
        fail("temporary namespace replacement");
    int replacement = make_directory(directory, "sqlite-tmp");
    before = temp_other_bytes;
    require(temporary_sort(db) != SQLITE_OK && temp_other_bytes == before,
        "stale temporary authority refuses new sorting files");
    if (unlinkat(directory, "sqlite-tmp", AT_REMOVEDIR) ||
        renameat(directory, "sqlite-tmp-held", directory, "sqlite-tmp"))
        fail("temporary namespace restore");
    require(temporary_sort(db) != SQLITE_OK, "restoration does not revive stale temporary authority");
    close(replacement);
    require(count_rows(db) == 1, "temporary failures preserve committed main database");
    temporary_deferred_close(db);
    db = NULL;
    require(temp_native_opens == temp_closes, "all created temporary handles close through SQLite");
    temporary_empty(&root);
    uninstall(&root);
    install(&root, parent, "temporary");
    require(enable_temporary(&root), "fresh authority admits restored temporary root");
    db = open_db("state.sqlite", root.vfs.zName);
    temporary_seed(db);
    before = temp_other_bytes;
    sql_ok(db, temporary_sort(db), "fresh-authority file-backed sort");
    require(temp_other_bytes > before, "fresh authority writes actual spill files");
    close_db(db);
    require(temp_native_opens == temp_closes, "fresh temporary handles close through SQLite");
    temporary_empty(&root);
    uninstall(&root);
    close(directory);

    directory = make_directory(parent, "temporary-link");
    if (symlinkat("../temporary/sqlite-tmp", directory, "sqlite-tmp"))
        fail("temporary root link creation");
    install(&root, parent, "temporary-link");
    require(!enable_temporary(&root), "native temporary root symlink refused");
    db = open_db("state.sqlite", root.vfs.zName);
    require(sqlite3_exec(db, temporary_seed_sql, NULL, NULL, NULL) != SQLITE_OK,
        "actual TEMP work fails without admitted temporary authority");
    require(!fstatat(directory, "sqlite-tmp", &metadata, AT_SYMLINK_NOFOLLOW) &&
        S_ISLNK(metadata.st_mode), "refused temporary root link preserved");
    close_db(db);
    uninstall(&root);
    close(directory);
}

static void same_process_wal(int parent) {
    int directory = make_directory(parent, "same-process-wal");
    Root root;
    install(&root, parent, "same-process-wal");
    sqlite3 *writer = open_db("state.sqlite", root.vfs.zName);
    seed(writer);
    sqlite3 *reader = readonly_reader(root.vfs.zName);
    wal_snapshot(reader, writer);
    close_db(writer);
    require(count_rows(reader) == 1, "reader remains valid after same-process writer close");
    close_db(reader);
    uninstall(&root);
    close(directory);
}

int main(int argc, char **argv) {
#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
    original_open = plain_open;
    original_unlink = unlink;
#endif
    require(argc == 2 && sqlite3_libversion_number() == 3053002 &&
        SQLITE_VERSION_NUMBER == 3053002, "exact pinned SQLite 3.53.2");
    if (mkdir(argv[1], 0700)) fail("fresh case root");
    int parent = open(argv[1], O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (parent < 0) fail("case root open");
    baseline(parent, argv[1]);
    guarded(parent);
    symlink_case(parent, "link-main", "state.sqlite");
    symlink_case(parent, "link-wal", "state.sqlite-wal");
    symlink_case(parent, "link-shm", "state.sqlite-shm");
    symlink_case(parent, "link-journal", "state.sqlite-journal");
    invalid_types(parent);
    concurrency(parent);
    rollback(parent);
    interrupted(parent);
    permission_lock(parent);
    same_process_wal(parent);
    private_temporary(parent);
    close(parent);
#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
    require(native_bridge_calls > 0, "compile-time native bridge actually used");
#endif
    require(native_close_errors == 0, "normal workflows have no actual native close errors");
    printf("{\"result\":\"linux_sqlite_root_prototype_passed\","
        "\"sqlite\":\"%s\",\"parent_assertions\":%d,"
        "\"concurrent_committed_rows\":100,"
        "\"native_bridge_calls\":%d,"
        "\"temp_database_written_bytes\":%lld,\"temp_spill_written_bytes\":%lld,"
        "\"temp_native_opens\":%d,\"temp_closed_files\":%d,"
        "\"temp_close_errors\":%d,"
        "\"native_close_errors\":%d,"
        "\"temp_interrupted_cleanup\":true,"
        "\"wal_snapshot_checks\":%d,\"busy_close_checks\":%d,\"deferred_close_checks\":%d,"
        "\"readonly_wal_snapshot_checks\":%d,"
        "\"auxiliary_checks\":%d,\"blob_lifetime_checks\":%d,\"backup_lifetime_checks\":%d,"
        "\"namespace_refusal_checks\":%d,"
        "\"entry_policy_checks\":%d,\"entry_uri_refusals\":%d,"
        "\"subjournal_written_bytes\":%lld,\"transient_written_bytes\":%lld,"
        "\"sqlite_sibling_close_preserves_reserved_lock\":true,"
        "\"scope\":\"generated ext4; isolated native I/O candidate\"}\n",
        sqlite3_libversion(), assertions, native_bridge_calls,
        (long long)temp_database_bytes, (long long)temp_other_bytes,
        temp_native_opens, temp_closes, temp_close_errors, native_close_errors,
        wal_snapshot_checks, busy_close_checks,
        deferred_close_checks, wal_snapshot_checks, auxiliary_checks, blob_lifetime_checks,
        backup_lifetime_checks, namespace_refusal_checks, entry_policy_checks, entry_uri_refusals,
        (long long)subjournal_bytes, (long long)transient_bytes);
    return 0;
}
