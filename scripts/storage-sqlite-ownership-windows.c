/* Isolated native context owned by the Rust lifetime proof, not production ABI. */
#define wmain domus_legacy_wmain
#include "probe.c"
#undef wmain

static void *ownership_root;
static int ownership_released;
static int ownership_close_errors;
static int ownership_native_close_errors;
static int ownership_quarantined;
static int ownership_refused_opens;
static HANDLE ownership_fault_handle = INVALID_HANDLE_VALUE;

static int ownership_open(sqlite3_vfs *vfs, sqlite3_filename path,
    sqlite3_file *file, int flags, int *actual) {
    PROBE_LOCK();
    if (ownership_quarantined || temp_close_errors != ownership_close_errors ||
        native_close_errors != ownership_native_close_errors) {
        file->pMethods = NULL;
        ownership_refused_opens++;
        PROBE_UNLOCK();
        return SQLITE_IOERR_CLOSE;
    }
    PROBE_UNLOCK();
    return open_file(vfs, path, file, flags, actual);
}

void *domus_probe_root_new(const char *path) {
    wchar_t wide[4096];
    require(!ownership_root && !directory_count &&
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, wide, 4096) > 0,
        "one bounded fresh Rust-owned native context");
    ownership_root = malloc(1);
    require(ownership_root != NULL, "native context token allocation");
    ownership_close_errors = temp_close_errors;
    ownership_native_close_errors = native_close_errors;
    ownership_quarantined = 0;
    install(wide);
    scoped.xOpen = ownership_open;
    require(enable_temporary(), "Rust-owned retained native temporary root");
    return ownership_root;
}

int domus_probe_root_live(void *root) {
    FILE_STANDARD_INFO info;
    return root && root == ownership_root && directory_count &&
        GetFileInformationByHandleEx(directories[directory_count - 1],
            FileStandardInfo, &info, sizeof(info)) && info.Directory;
}

static void ownership_release(void) {
    require(sqlite3_vfs_unregister(&scoped) == SQLITE_OK, "owned VFS unregisters after SQLite cleanup");
    release_root();
    free(ownership_root);
    ownership_root = NULL;
    ownership_released++;
}

int domus_probe_root_drop(void *root) {
    require(domus_probe_root_live(root), "owned native context remains valid through final cleanup");
    require(temp_native_opens == temp_closes,
        "all actual temporary file callbacks finish before native context destruction");
    if (temp_close_errors != ownership_close_errors ||
        native_close_errors != ownership_native_close_errors) {
        ownership_quarantined = 1;
        return SQLITE_IOERR_CLOSE;
    }
    ownership_release();
    return SQLITE_OK;
}

sqlite3 *domus_probe_connection(void *root) {
    require(domus_probe_root_live(root), "connection borrows actual Rust-owned root");
    return open_db();
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
    require(ownership_fault_handle == INVALID_HANDLE_VALUE,
        "only one fixture handle can be protected");
    require(sqlite3_file_control(db, "temp", SQLITE_FCNTL_WIN32_GET_HANDLE,
        &ownership_fault_handle) == SQLITE_OK &&
        ownership_fault_handle != INVALID_HANDLE_VALUE && ownership_fault_handle != NULL,
        "public SQLite control returns actual temporary database handle");
    require(SetHandleInformation(ownership_fault_handle,
        HANDLE_FLAG_PROTECT_FROM_CLOSE, HANDLE_FLAG_PROTECT_FROM_CLOSE),
        "protect only the generated temporary database handle from native close");
}

int domus_probe_verify_quarantine(void) {
    FILE_STANDARD_INFO info;
    DWORD flags;
    require(ownership_quarantined && temp_close_errors == ownership_close_errors + 1 &&
        native_close_errors > ownership_native_close_errors &&
        domus_probe_root_live(ownership_root),
        "native close error retains the actual root context");
    require(GetHandleInformation(ownership_fault_handle, &flags) &&
        (flags & HANDLE_FLAG_PROTECT_FROM_CLOSE) &&
        GetFileInformationByHandleEx(ownership_fault_handle, FileStandardInfo,
            &info, sizeof(info)) && !info.Directory && info.EndOfFile.QuadPart > 0,
        "actual native temporary database handle remains open after SQLite close");
    sqlite3 *other = NULL;
    int refused = ownership_refused_opens, opened = native_bridge_calls;
    int result = sqlite3_open_v2("state.sqlite", &other,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW, scoped.zName);
    require(result != SQLITE_OK && other &&
        sqlite3_extended_errcode(other) == SQLITE_IOERR_CLOSE &&
        ownership_refused_opens > refused && native_bridge_calls == opened,
        "quarantined context refuses new SQLite admission before native open");
    require(sqlite3_close(other) == SQLITE_OK, "refused quarantine connection closes");
    return SQLITE_IOERR_CLOSE;
}

void domus_probe_recover_fixture(void) {
    require(ownership_quarantined &&
        ownership_fault_handle != INVALID_HANDLE_VALUE &&
        temp_close_errors == ownership_close_errors + 1,
        "recovery is limited to the exact protected fixture handle");
    require(SetHandleInformation(ownership_fault_handle, HANDLE_FLAG_PROTECT_FROM_CLOSE, 0) &&
        CloseHandle(ownership_fault_handle),
        "explicit fixture recovery confirms the actual native close");
    ownership_fault_handle = INVALID_HANDLE_VALUE;
    ownership_release();
}

#ifdef DOMUS_SQLITE_CONCURRENT
#include "storage-sqlite-independent-native.h"
#endif
