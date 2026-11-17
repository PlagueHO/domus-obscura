/* Generated NTFS experiment. Optional SQLite hooks are not a production adapter. */
#define _WIN32_WINNT 0x0a00
#include <windows.h>
#include <winternl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include "sqlite3.h"
#ifdef DOMUS_SQLITE_CONCURRENT
#include "storage-sqlite-probe-lock.h"
#else
#define PROBE_LOCK() ((void)0)
#define PROBE_UNLOCK() ((void)0)
#define PROBE_ASSERT_INCREMENT(value) ((value)++)
#endif
#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
#include "storage-sqlite-native-io.h"
#endif

#pragma warning(disable: 4191)

static HANDLE directories[256];
static int directory_count;
static wchar_t root_path[4096];
static char database_path[4096];
static sqlite3_vfs *base;
static sqlite3_vfs scoped;
static int assertions;
static int wal_opens, shm_opens, journal_opens;
static int native_bridge_calls;
static int native_close_errors;
static HANDLE temporary_directory = INVALID_HANDLE_VALUE;
static wchar_t temporary_path[4096];
static char temporary_anchor[4096];
static void execute(sqlite3 *db, const char *sql);
static void require(int condition, const char *label);
#include "storage-sqlite-temp-workload.h"

static void require(int condition, const char *label) {
    if (!condition) {
        fprintf(stderr, "%s failed; Win32=%lu\n", label, GetLastError());
        exit(1);
    }
    PROBE_ASSERT_INCREMENT(assertions);
}

static HANDLE native_open_options(HANDLE parent, const wchar_t *name, DWORD access,
    DWORD share, ULONG disposition, int directory, DWORD file_attributes, ULONG options) {
    UNICODE_STRING text;
    OBJECT_ATTRIBUTES attributes;
    IO_STATUS_BLOCK status;
    HANDLE file = INVALID_HANDLE_VALUE;
    size_t length = wcslen(name);
    require(length > 0 && length <= 255, "bounded native component");
    memset(&attributes, 0, sizeof(attributes));
    text.Length = (USHORT)(length * sizeof(wchar_t));
    text.MaximumLength = text.Length;
    text.Buffer = (PWSTR)name;
    attributes.Length = sizeof(attributes);
    attributes.RootDirectory = parent;
    attributes.ObjectName = &text;
    attributes.Attributes = 0x1040; /* Case insensitive, no intermediate reparse. */
    NTSTATUS result = NtCreateFile(&file, access | SYNCHRONIZE, &attributes,
        &status, NULL, file_attributes, share, disposition,
        0x200020 | (directory ? 1 : 0x40) | options, NULL, 0);
    if (result < 0) {
        SetLastError(RtlNtStatusToDosError(result));
        return INVALID_HANDLE_VALUE;
    }
    FILE_ATTRIBUTE_TAG_INFO tag;
    if (!GetFileInformationByHandleEx(file, FileAttributeTagInfo, &tag, sizeof(tag)) ||
        (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) ||
        (!!(tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != !!directory)) {
        CloseHandle(file);
        SetLastError(ERROR_ACCESS_DENIED);
        return INVALID_HANDLE_VALUE;
    }
    return file;
}

static HANDLE native_open(HANDLE parent, const wchar_t *name, DWORD access,
    DWORD share, ULONG disposition, int directory) {
    return native_open_options(parent, name, access, share, disposition, directory,
        FILE_ATTRIBUTE_NORMAL, 0);
}

static void retain_root(const wchar_t *path) {
    require(wcslen(path) < 4096 && path[1] == L':' && path[2] == L'\\',
        "generated absolute drive path");
    require(wcscpy_s(root_path, 4096, path) == 0, "root path copy");
    wchar_t anchor[4] = { path[0], L':', L'\\', 0 };
    directories[0] = CreateFileW(anchor, FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, NULL);
    require(directories[0] != INVALID_HANDLE_VALUE, "native drive root");
    directory_count = 1;
    const wchar_t *start = path + 3;
    while (*start) {
        const wchar_t *end = wcschr(start, L'\\');
        size_t length = end ? (size_t)(end - start) : wcslen(start);
        wchar_t component[256];
        require(length > 0 && length < 256 && directory_count < 256,
            "bounded root component");
        require(wcsncpy_s(component, 256, start, length) == 0 &&
            wcscmp(component, L".") && wcscmp(component, L"..") &&
            !wcschr(component, L':'), "safe native root component");
        HANDLE next = native_open(directories[directory_count - 1], component,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE, 1, 1);
        require(next != INVALID_HANDLE_VALUE, "retained no-follow parent component");
        directories[directory_count++] = next;
        if (!end) break;
        start = end + 1;
    }
    wchar_t filesystem[64];
    require(GetVolumeInformationByHandleW(directories[directory_count - 1],
        NULL, 0, NULL, NULL, NULL, filesystem, 64) && !wcscmp(filesystem, L"NTFS"),
        "actual NTFS root handle");
    typedef LONG (NTAPI *QueryVolume)(HANDLE, PIO_STATUS_BLOCK, PVOID, ULONG, ULONG);
    QueryVolume query = (QueryVolume)GetProcAddress(GetModuleHandleW(L"ntdll.dll"),
        "NtQueryVolumeInformationFile");
    ULONG device[2];
    IO_STATUS_BLOCK status;
    require(query && query(directories[directory_count - 1], &status,
        device, sizeof(device), 4) == 0 && status.Information == sizeof(device) &&
        !(device[1] & 0x10), "actual non-remote device");
}

static const wchar_t *leaf(const wchar_t *path) {
    size_t size = wcslen(root_path);
    if (wcsncmp(path, root_path, size) || path[size] != L'\\') return NULL;
    const wchar_t *name = path + size + 1;
    return (!wcscmp(name, L"state.sqlite") || !wcscmp(name, L"state.sqlite-wal") ||
        !wcscmp(name, L"state.sqlite-shm") || !wcscmp(name, L"state.sqlite-journal"))
        ? name : NULL;
}

static const wchar_t *temporary_relative(const wchar_t *path) {
    if (temporary_directory == INVALID_HANDLE_VALUE) return NULL;
    size_t length = wcslen(temporary_path);
    if (wcsncmp(path, temporary_path, length) || path[length] != L'\\') return NULL;
    const wchar_t *name = path + length + 1;
    char narrow[40];
    return WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, name, -1,
        narrow, sizeof(narrow), NULL, NULL) && temporary_leaf(narrow) ? name : NULL;
}

static int enable_temporary(void) {
    require(swprintf_s(temporary_path, 4096, L"%s\\sqlite-tmp", root_path) > 0 &&
        WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, temporary_path, -1,
            temporary_anchor, sizeof(temporary_anchor), NULL, NULL) > 0,
        "bounded temporary label under retained state root");
    temporary_directory = native_open(directories[directory_count - 1], L"sqlite-tmp",
        FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
        FILE_SHARE_READ | FILE_SHARE_WRITE, 3, 1);
    return temporary_directory != INVALID_HANDLE_VALUE;
}

static HANDLE WINAPI guarded_create(LPCWSTR path, DWORD access, DWORD share,
    LPSECURITY_ATTRIBUTES security, DWORD disposition, DWORD flags, HANDLE template) {
    const wchar_t *name = leaf(path);
    const wchar_t *temp_name = temporary_relative(path);
    DWORD temp_flags = FILE_ATTRIBUTE_TEMPORARY | FILE_ATTRIBUTE_HIDDEN |
        FILE_FLAG_DELETE_ON_CLOSE;
    if (temp_name) {
        if (security || template || flags != temp_flags || disposition != CREATE_NEW) {
            SetLastError(ERROR_ACCESS_DENIED);
            return INVALID_HANDLE_VALUE;
        }
        HANDLE file = native_open_options(temporary_directory, temp_name, access | DELETE,
            share, 2, 0, FILE_ATTRIBUTE_TEMPORARY | FILE_ATTRIBUTE_HIDDEN, 0x1000);
        if (file != INVALID_HANDLE_VALUE) temp_native_opens++;
        return file;
    }
    if (!name || security || template || flags != FILE_ATTRIBUTE_NORMAL ||
        (disposition != CREATE_NEW && disposition != OPEN_ALWAYS &&
         disposition != OPEN_EXISTING)) {
        namespace_native_refusals++;
        SetLastError(ERROR_ACCESS_DENIED);
        return INVALID_HANDLE_VALUE;
    }
    HANDLE file = native_open(directories[directory_count - 1], name, access,
        share, disposition == CREATE_NEW ? 2 : disposition == OPEN_ALWAYS ? 3 : 1, 0);
    if (file != INVALID_HANDLE_VALUE) {
        if (!wcscmp(name, L"state.sqlite-wal")) wal_opens++;
        if (!wcscmp(name, L"state.sqlite-shm")) shm_opens++;
        if (!wcscmp(name, L"state.sqlite-journal")) journal_opens++;
    }
    return file;
}

static BOOL WINAPI guarded_close(HANDLE file) {
    BOOL result = CloseHandle(file);
    if (!result) native_close_errors++;
    return result;
}

#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
HANDLE WINAPI sqlite3_domus_native_create_v1(LPCWSTR path, DWORD access,
    DWORD share, LPSECURITY_ATTRIBUTES security, DWORD disposition,
    DWORD flags, HANDLE template) {
    PROBE_LOCK();
#ifdef DOMUS_SQLITE_CONCURRENT
    if (native_close_errors) {
        PROBE_UNLOCK();
        SetLastError(ERROR_ACCESS_DENIED);
        return INVALID_HANDLE_VALUE;
    }
#endif
    native_bridge_calls++;
    HANDLE result = guarded_create(path, access, share, security, disposition, flags, template);
    DWORD error = GetLastError();
    PROBE_UNLOCK();
    SetLastError(error);
    return result;
}
BOOL WINAPI sqlite3_domus_native_close_v1(HANDLE file) {
    PROBE_LOCK();
    BOOL result = guarded_close(file);
    DWORD error = GetLastError();
    PROBE_UNLOCK();
    SetLastError(error);
    return result;
}
#endif

static int full_path(sqlite3_vfs *vfs, const char *name, int capacity, char *out) {
    (void)vfs;
    if (strcmp(name, "state.sqlite") || !directory_count) return SQLITE_CANTOPEN;
    return strcpy_s(out, (size_t)capacity, database_path) ? SQLITE_CANTOPEN : SQLITE_OK;
}

static int open_file(sqlite3_vfs *vfs, sqlite3_filename path, sqlite3_file *file,
    int flags, int *actual) {
    (void)vfs;
    if (!entry_filename(path, flags)) return SQLITE_CANTOPEN;
    if (!path) {
        if (temporary_directory == INVALID_HANDLE_VALUE) return SQLITE_CANTOPEN;
        return temporary_open(base, temporary_anchor, '\\', file, flags, actual);
    }
    return base->xOpen(base, path, file, flags, actual);
}

static int access_file(sqlite3_vfs *vfs, const char *path, int flags, int *out) {
    (void)vfs;
    wchar_t wide[4096];
    *out = 0;
    if (!MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, wide, 4096))
        return SQLITE_IOERR_ACCESS;
    const wchar_t *name = leaf(wide);
    if (!name) return SQLITE_IOERR_ACCESS;
    HANDLE file = native_open(directories[directory_count - 1], name,
        flags == SQLITE_ACCESS_READWRITE ? GENERIC_READ | GENERIC_WRITE : FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 1, 0);
    if (file == INVALID_HANDLE_VALUE)
        return GetLastError() == ERROR_FILE_NOT_FOUND ? SQLITE_OK : SQLITE_IOERR_ACCESS;
    CloseHandle(file);
    *out = 1;
    return SQLITE_OK;
}

static int delete_file(sqlite3_vfs *vfs, const char *path, int sync) {
    (void)vfs;
    (void)sync;
    wchar_t wide[4096];
    if (!MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, wide, 4096))
        return SQLITE_IOERR_DELETE;
    const wchar_t *name = leaf(wide);
    if (!name) return SQLITE_IOERR_DELETE;
    HANDLE file = native_open(directories[directory_count - 1], name, DELETE | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 1, 0);
    if (file == INVALID_HANDLE_VALUE)
        return GetLastError() == ERROR_FILE_NOT_FOUND ? SQLITE_OK : SQLITE_IOERR_DELETE;
    FILE_DISPOSITION_INFO disposition = { TRUE };
    int result = SetFileInformationByHandle(file, FileDispositionInfo,
        &disposition, sizeof(disposition)) ? SQLITE_OK : SQLITE_IOERR_DELETE;
    CloseHandle(file);
    return result;
}

static void install(const wchar_t *path) {
    retain_root(path);
    wchar_t file[4096];
    require(swprintf_s(file, 4096, L"%s\\state.sqlite", path) > 0 &&
        WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, file, -1,
            database_path, sizeof(database_path), NULL, NULL) > 0, "bounded database token");
    base = sqlite3_vfs_find("win32");
#ifdef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base && base->iVersion >= 2, "upstream Windows VFS with WAL methods");
#else
    require(base && base->iVersion >= 3 && base->xSetSystemCall, "Windows test hooks");
#endif
    scoped = *base;
    scoped.zName = "domus_windows_root_probe";
    scoped.pNext = NULL;
    scoped.xFullPathname = full_path;
    scoped.xOpen = open_file;
    scoped.xAccess = access_file;
    scoped.xDelete = delete_file;
    if (scoped.szOsFile < (int)sizeof(TemporaryFile))
        scoped.szOsFile = sizeof(TemporaryFile);
#ifndef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base->xSetSystemCall(base, "CreateFileW",
        (sqlite3_syscall_ptr)guarded_create) == SQLITE_OK, "native create hook");
    require(base->xSetSystemCall(base, "CloseHandle",
        (sqlite3_syscall_ptr)guarded_close) == SQLITE_OK, "native close observation");
#endif
    require(sqlite3_vfs_register(&scoped, 0) == SQLITE_OK, "Windows test VFS");
}

static sqlite3 *open_db(void) {
    sqlite3 *db = NULL;
    require(entry_open("state.sqlite", &db, SQLITE_OPEN_READWRITE |
        SQLITE_OPEN_CREATE | SQLITE_OPEN_NOFOLLOW, scoped.zName) == SQLITE_OK, "SQLite open");
    require(sqlite3_busy_timeout(db, 5000) == SQLITE_OK, "SQLite busy timeout");
    return db;
}

static void execute(sqlite3 *db, const char *sql) {
    int result = sqlite3_exec(db, sql, NULL, NULL, NULL);
    if (result != SQLITE_OK) {
        fprintf(stderr, "SQLite %d: %s; %s\n", result, sqlite3_errmsg(db), sql);
        exit(1);
    }
}

static int count_rows(sqlite3 *db) {
    sqlite3_stmt *query = NULL;
    require(sqlite3_prepare_v2(db, "SELECT count(*) FROM values_table", -1,
        &query, NULL) == SQLITE_OK && sqlite3_step(query) == SQLITE_ROW, "committed row query");
    int result = sqlite3_column_int(query, 0);
    require(sqlite3_finalize(query) == SQLITE_OK, "query finalize");
    return result;
}

static void release_root(void) {
    if (temporary_directory != INVALID_HANDLE_VALUE) {
        CloseHandle(temporary_directory);
        temporary_directory = INVALID_HANDLE_VALUE;
    }
    while (directory_count) CloseHandle(directories[--directory_count]);
}

static void private_temporary(sqlite3 *db) {
    require(enable_temporary(), "native temporary child under retained state");
    temporary_seed(db);
    sqlite3_int64 before = temp_other_bytes;
    require(temporary_sort(db) == SQLITE_OK && temp_other_bytes > before,
        "file-backed sort and exact sorted values");
    temporary_auxiliary(db);
    namespace_refusals(db, scoped.zName);
    temporary_busy_close(db);
    wchar_t moved[4096];
    require(swprintf_s(moved, 4096, L"%s-held", temporary_path) > 0,
        "bounded temporary replacement target");
    SetLastError(0);
    int renamed = MoveFileExW(temporary_path, moved, 0);
    DWORD error = GetLastError();
    require(!renamed && (error == ERROR_ACCESS_DENIED || error == ERROR_SHARING_VIOLATION),
        "retained temporary root denies replacement");
    wchar_t collision[4096];
    require(swprintf_s(collision, 4096, L"%s\\etilqs_00000000000000000000000000000000",
        temporary_path) > 0 && CreateDirectoryW(collision, NULL), "temporary collision fixture");
    forced_temp_leaf = "etilqs_00000000000000000000000000000000";
    require(temporary_sort(db) != SQLITE_OK, "actual sort refuses preexisting temporary directory");
    forced_temp_leaf = NULL;
    require(GetFileAttributesW(collision) != INVALID_FILE_ATTRIBUTES &&
        (GetFileAttributesW(collision) & FILE_ATTRIBUTE_DIRECTORY) &&
        RemoveDirectoryW(collision), "refused temporary directory preserved then removed explicitly");
    require(temporary_sort(db) == SQLITE_OK, "sort succeeds after temporary-file refusal");
}

int wmain(int argc, wchar_t **argv) {
    require((argc == 2 || argc == 4) && sqlite3_libversion_number() == 3053002 &&
        SQLITE_VERSION_NUMBER == 3053002, "exact pinned SQLite");
    if (argc == 2) require(CreateDirectoryW(argv[1], NULL), "fresh generated database root");
    install(argv[1]);
    sqlite3 *db = open_db();
    if (argc == 4 && !wcscmp(argv[2], L"reject-temp")) {
        require(!enable_temporary(), "temporary root junction refused through actual native handle");
        require(sqlite3_exec(db, temporary_seed_sql, NULL, NULL, NULL) != SQLITE_OK,
            "actual TEMP work refuses missing native authority");
        require(count_rows(db) == 103, "temporary root refusal preserves committed main rows");
    } else if (argc == 4 && !wcscmp(argv[2], L"interrupt")) {
        execute(db, "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;");
        require(enable_temporary(), "interrupted writer native temporary authority");
        temporary_seed(db);
        sqlite3_int64 before = temp_other_bytes;
        require(temporary_sort(db) == SQLITE_OK && temp_other_bytes > before,
            "interrupted writer actual file-backed sort");
        execute(db, "INSERT INTO values_table VALUES(888);");
        execute(db, "BEGIN IMMEDIATE; INSERT INTO values_table VALUES(999);");
        require(sqlite3_db_cacheflush(db) == SQLITE_OK, "uncommitted WAL flush");
        ExitProcess(23);
    } else if (argc == 4 && !wcscmp(argv[2], L"recover")) {
        require(count_rows(db) == 103, "recovery retains committed rows only");
        sqlite3_stmt *query = NULL;
        require(sqlite3_prepare_v2(db, "SELECT count(*) FROM values_table WHERE value=999",
            -1, &query, NULL) == SQLITE_OK && sqlite3_step(query) == SQLITE_ROW &&
            sqlite3_column_int(query, 0) == 0, "uncommitted value absent");
        require(sqlite3_finalize(query) == SQLITE_OK, "recovery query finalize");
    } else if (argc == 4 && !wcscmp(argv[2], L"reject-wal")) {
        require(sqlite3_exec(db, "SELECT count(*) FROM values_table",
            NULL, NULL, NULL) != SQLITE_OK, "actual WAL reparse target refused");
    } else if (argc == 4) {
        require(!wcscmp(argv[2], L"worker"), "known fixture operation");
        int identity = _wtoi(argv[3]);
        require(identity == 1 || identity == 2, "bounded worker identity");
        for (int i = 0; i < 50; i++) {
            sqlite3_stmt *statement = NULL;
            require(sqlite3_prepare_v2(db, "INSERT INTO values_table VALUES(?1)",
                -1, &statement, NULL) == SQLITE_OK, "worker prepare");
            require(sqlite3_bind_int(statement, 1, identity * 1000 + i) == SQLITE_OK &&
                sqlite3_step(statement) == SQLITE_DONE, "worker committed row");
            require(sqlite3_finalize(statement) == SQLITE_OK, "worker finalize");
        }
    } else {
        execute(db, "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;");
        execute(db, "CREATE TABLE values_table(value INTEGER PRIMARY KEY);");
        execute(db, "INSERT INTO values_table VALUES(1);");
        wchar_t moved[4096];
        require(swprintf_s(moved, 4096, L"%s-moved", argv[1]) > 0, "bounded rename target");
        SetLastError(0);
        int renamed = MoveFileExW(argv[1], moved, 0);
        DWORD rename_error = GetLastError();
        require(!renamed && (rename_error == ERROR_SHARING_VIOLATION ||
            rename_error == ERROR_ACCESS_DENIED), "retained root refuses namespace replacement");
        require(wal_opens > 0 && shm_opens > 0, "actual native WAL and SHM opens");
        require(sqlite3_close(db) == SQLITE_OK, "initial close");
        db = NULL;
        wchar_t executable[4096];
        require(GetModuleFileNameW(NULL, executable, 4096) > 0, "current test executable");
        PROCESS_INFORMATION children[2];
        for (int i = 0; i < 2; i++) {
            wchar_t command[8192];
            require(swprintf_s(command, 8192, L"\"%s\" \"%s\" worker %d",
                executable, argv[1], i + 1) > 0, "bounded child command");
            STARTUPINFOW startup;
            memset(&startup, 0, sizeof(startup));
            startup.cb = sizeof(startup);
            require(CreateProcessW(executable, command, NULL, NULL, FALSE, 0,
                NULL, NULL, &startup, &children[i]), "fresh SQLite worker");
            CloseHandle(children[i].hThread);
        }
        for (int i = 0; i < 2; i++) {
            DWORD result;
            require(WaitForSingleObject(children[i].hProcess, 15000) == WAIT_OBJECT_0 &&
                GetExitCodeProcess(children[i].hProcess, &result) && result == 0,
                "concurrent worker finished");
            CloseHandle(children[i].hProcess);
        }
        db = open_db();
        require(count_rows(db) == 101, "100 concurrent committed rows and seed");
        sqlite3_stmt *query = NULL;
        require(sqlite3_prepare_v2(db, "SELECT count(*) FROM values_table WHERE "
            "value=1 OR value BETWEEN 1000 AND 1049 OR value BETWEEN 2000 AND 2049",
            -1, &query, NULL) == SQLITE_OK && sqlite3_step(query) == SQLITE_ROW &&
            sqlite3_column_int(query, 0) == 101, "every expected concurrent primary key is present");
        require(sqlite3_finalize(query) == SQLITE_OK, "exact concurrent values finalize");
        sqlite3 *reader = readonly_reader(scoped.zName);
        wal_snapshot(reader, db);
        require(sqlite3_close(reader) == SQLITE_OK, "same-process read-only reader closes");
        require(count_rows(db) == 101, "reader survives same-process writer close");
        execute(db, "PRAGMA journal_mode=DELETE;");
        execute(db, "INSERT INTO values_table VALUES(2);");
        require(journal_opens > 0 && count_rows(db) == 102, "native rollback journal and exact rows");
        private_temporary(db);
    }
    if (argc == 2) {
        temporary_deferred_close(db);
        db = NULL;
    } else {
        require(sqlite3_close(db) == SQLITE_OK, "final database close");
    }
    require(temp_native_opens == temp_closes && temp_close_errors == 0,
        "all temporary files close successfully through SQLite");
    require(sqlite3_vfs_unregister(&scoped) == SQLITE_OK, "test VFS unregistered");
#ifndef DOMUS_SQLITE_SOURCE_ADAPTER
    require(base->xSetSystemCall(base, "CreateFileW", NULL) == SQLITE_OK,
        "original native create hook restored");
#else
    require(native_bridge_calls > 0, "compile-time native bridge actually used");
#endif
    FILE_ID_INFO original;
    require(GetFileInformationByHandleEx(directories[directory_count - 1],
        FileIdInfo, &original, sizeof(original)), "full retained root identity");
    release_root();
    if (argc == 2) {
        wchar_t temporary_moved[4096], pattern[4096];
        require(swprintf_s(pattern, 4096, L"%s\\*", temporary_path) > 0,
            "bounded temporary cleanup observation");
        WIN32_FIND_DATAW entry;
        HANDLE listing = FindFirstFileW(pattern, &entry);
        require(listing != INVALID_HANDLE_VALUE, "temporary listing after all pins close");
        int remaining = 0;
        do {
            if (wcscmp(entry.cFileName, L".") && wcscmp(entry.cFileName, L"..")) remaining++;
        } while (FindNextFileW(listing, &entry));
        require(GetLastError() == ERROR_NO_MORE_FILES && FindClose(listing) && !remaining,
            "all temporary files removed after close");
        require(swprintf_s(temporary_moved, 4096, L"%s-held", temporary_path) > 0 &&
            MoveFileExW(temporary_path, temporary_moved, 0) &&
            MoveFileExW(temporary_moved, temporary_path, 0),
            "temporary rename succeeds after handles close");
        wchar_t moved[4096];
        require(swprintf_s(moved, 4096, L"%s-moved", argv[1]) > 0 &&
            MoveFileExW(argv[1], moved, 0), "rename succeeds after all pins close");
        require(CreateDirectoryW(argv[1], NULL), "fresh replacement namespace");
        retain_root(argv[1]);
        FILE_ID_INFO replacement;
        require(GetFileInformationByHandleEx(directories[directory_count - 1],
            FileIdInfo, &replacement, sizeof(replacement)) &&
            memcmp(&original, &replacement, sizeof(original)),
            "replacement differs from saved full native binding");
        release_root();
        require(RemoveDirectoryW(argv[1]) && MoveFileExW(moved, argv[1], 0),
            "restore exact generated original");
        require(native_close_errors == 0, "normal workflows have no actual native close errors");
        printf("{\"result\":\"windows_sqlite_root_prototype_passed\","
            "\"sqlite\":\"3.53.2\",\"parent_assertions\":%d,"
            "\"concurrent_committed_rows\":100,\"native_bridge_calls\":%d,"
            "\"temp_database_written_bytes\":%lld,\"temp_spill_written_bytes\":%lld,"
            "\"temp_native_opens\":%d,\"temp_closed_files\":%d,"
            "\"temp_close_errors\":%d,"
            "\"native_close_errors\":%d,"
            "\"wal_snapshot_checks\":%d,\"busy_close_checks\":%d,\"deferred_close_checks\":%d,"
            "\"readonly_wal_snapshot_checks\":%d,"
            "\"auxiliary_checks\":%d,\"blob_lifetime_checks\":%d,\"backup_lifetime_checks\":%d,"
            "\"namespace_refusal_checks\":%d,"
            "\"entry_policy_checks\":%d,\"entry_uri_refusals\":%d,"
            "\"subjournal_written_bytes\":%lld,\"transient_written_bytes\":%lld,"
            "\"scope\":\"generated NTFS; isolated native I/O candidate\"}\n",
            assertions, native_bridge_calls, (long long)temp_database_bytes,
            (long long)temp_other_bytes, temp_native_opens, temp_closes, temp_close_errors,
            native_close_errors,
            wal_snapshot_checks, busy_close_checks, deferred_close_checks, wal_snapshot_checks,
            auxiliary_checks, blob_lifetime_checks, backup_lifetime_checks, namespace_refusal_checks,
            entry_policy_checks, entry_uri_refusals,
            (long long)subjournal_bytes, (long long)transient_bytes);
    }
    return 0;
}
