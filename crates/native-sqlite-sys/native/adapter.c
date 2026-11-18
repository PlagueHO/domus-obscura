/*
 * Scoped native I/O for bundled SQLite 3.53.2.
 *
 * Registry locks protect references and admission flags only. No upstream
 * SQLite call or filesystem operation runs under that lock. Each native
 * descriptor owns a context reference, including Unix deferred-close fds.
 * A failed native close is never retried. Its record and authority are
 * quarantined until process exit, even if upstream xClose reports success.
 */
#define _GNU_SOURCE
#define _WIN32_WINNT 0x0a00
#include "sqlite3.h"
#if SQLITE_VERSION_NUMBER != 3053002
#error The native boundary requires the pinned SQLite 3.53.2 header
#endif
extern int domus_sqlite_source_boundary_v1(void);
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
#include <winternl.h>
typedef HANDLE Native;
typedef FILE_ID_INFO Identity;
#define BAD INVALID_HANDLE_VALUE
static SRWLOCK registry_lock = SRWLOCK_INIT;
static CONDITION_VARIABLE registry_changed = CONDITION_VARIABLE_INIT;
#define LOCK() AcquireSRWLockExclusive(&registry_lock)
#define UNLOCK() ReleaseSRWLockExclusive(&registry_lock)
#define WAIT() ((void)SleepConditionVariableSRW(&registry_changed, &registry_lock, INFINITE, 0))
#define NOTIFY() WakeAllConditionVariable(&registry_changed)
#else
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
typedef int Native;
typedef struct stat Identity;
#define BAD (-1)
static pthread_mutex_t registry_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t registry_changed = PTHREAD_COND_INITIALIZER;
#define LOCK() ((void)pthread_mutex_lock(&registry_lock))
#define UNLOCK() ((void)pthread_mutex_unlock(&registry_lock))
#define WAIT() ((void)pthread_cond_wait(&registry_changed, &registry_lock))
#define NOTIFY() ((void)pthread_cond_broadcast(&registry_changed))
#endif
#include "native.h"

#define MAX_CHAIN 256
#define MAX_NAME 255
#define PATH_SIZE 768
#define SELECTION_FILE ".domus-storage-selection.lock"
#define MAX_SELECTION 256
typedef struct Context Context;
typedef struct Descriptor Descriptor;
struct Context {
    Context *next;
    unsigned long long id;
    size_t refs;
    int invalid, close_error, closing, ready, listed;
    int count;
    Native directories[MAX_CHAIN], temporary;
    Identity identities[MAX_CHAIN], temporary_identity;
    Native selection;
    Identity selection_identity;
    int selection_size;
    unsigned char selection_record[MAX_SELECTION];
#ifdef _WIN32
    SRWLOCK selection_read_lock;
#endif
    char components[MAX_CHAIN][MAX_NAME + 1];
    char leaf[MAX_NAME + 1], anchor[96], vfs_name[64];
    sqlite3_vfs vfs;
    sqlite3_vfs *base;
    unsigned long long temporary_opens, temporary_bytes, native_live;
};
struct Descriptor {
    Descriptor *next;
    Native native;
    Context *context;
    int failed, closing;
};
static Context *contexts;
static Descriptor *descriptors;
static unsigned long long next_id;
static int track(Context *, Native, Descriptor *);

static int good_component(const char *s) {
    size_t n = s ? strlen(s) : 0;
    if (!n || n > MAX_NAME || !strcmp(s, ".") || !strcmp(s, "..")) return 0;
    for (size_t i = 0; i < n; i++)
        if ((unsigned char)s[i] < 32 || strchr("/\\:*?\"<>|", s[i])) return 0;
    return s[n-1] != '.' && s[n-1] != ' ';
}

static int native_identity(Native file, Identity *out, int directory) {
#ifdef _WIN32
    FILE_ATTRIBUTE_TAG_INFO tag;
    return GetFileInformationByHandleEx(file, FileAttributeTagInfo, &tag, sizeof(tag)) &&
        !(tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) &&
        (!!(tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) == !!directory) &&
        GetFileInformationByHandleEx(file, FileIdInfo, out, sizeof(*out));
#else
    return !fstat(file, out) &&
        (directory ? S_ISDIR(out->st_mode) : S_ISREG(out->st_mode));
#endif
}
static int same_identity(const Identity *a, const Identity *b) {
#ifdef _WIN32
    return a->VolumeSerialNumber == b->VolumeSerialNumber &&
        !memcmp(a->FileId.Identifier, b->FileId.Identifier, 16);
#else
    return a->st_dev == b->st_dev && a->st_ino == b->st_ino;
#endif
}
static int plain_close(Native file) {
#ifdef _WIN32
    return CloseHandle(file) ? 0 : -1;
#else
    return close(file);
#endif
}
static Native duplicate(Native file) {
#ifdef _WIN32
    HANDLE result;
    return DuplicateHandle(GetCurrentProcess(), file, GetCurrentProcess(), &result,
        0, FALSE, DUPLICATE_SAME_ACCESS) ? result : BAD;
#else
    return fcntl(file, F_DUPFD_CLOEXEC, 3);
#endif
}
static void retain(Context *c) { LOCK(); c->refs++; UNLOCK(); }
static void quarantine(Context *c) {
    LOCK();
    c->close_error = 1; c->invalid = 1;
    if (!c->listed) {
        c->closing = 1; c->ready = -1;
        c->next = contexts; contexts = c; c->listed = 1;
    }
    NOTIFY();
    UNLOCK();
}

/* Terminal release only: native refs and the connection destructor have ended. */
static int release(Context *c) {
    int retire = 0, failed;
    LOCK();
    c->refs--;
    failed = c->close_error;
    if (!c->refs && !failed && !c->closing) { c->closing = 1; retire = 1; }
    UNLOCK();
    if (!retire) return failed ? SQLITE_IOERR_CLOSE : SQLITE_OK;
    sqlite3_vfs_unregister(&c->vfs);
    if (c->temporary != BAD) {
        if (plain_close(c->temporary)) { quarantine(c); return SQLITE_IOERR_CLOSE; }
        c->temporary = BAD;
    }
    for (int i = c->count - 1; i >= 0; i--) {
        if (c->directories[i] != BAD) {
            if (plain_close(c->directories[i])) { quarantine(c); return SQLITE_IOERR_CLOSE; }
            c->directories[i] = BAD;
        }
        if (c->selection != BAD) {
            if (plain_close(c->selection)) { quarantine(c); return SQLITE_IOERR_CLOSE; }
            c->selection = BAD;
        }
    }
    LOCK();
    Context **entry = &contexts;
    while (*entry && *entry != c) entry = &(*entry)->next;
    if (*entry) { *entry = c->next; c->listed = 0; }
    UNLOCK();
    free(c);
    return SQLITE_OK;
}

#ifdef _WIN32
static Native relative_open(Native parent, const char *name, DWORD access,
    DWORD share, ULONG disposition, int directory, DWORD attributes, ULONG extra) {
    wchar_t wide[MAX_NAME + 1];
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, name, -1,
        wide, MAX_NAME + 1);
    if (!good_component(name) || n < 2) {
        SetLastError(ERROR_ACCESS_DENIED); return BAD;
    }
    UNICODE_STRING text = { (USHORT)((n-1)*2), (USHORT)((n-1)*2), wide };
    OBJECT_ATTRIBUTES object;
    IO_STATUS_BLOCK status;
    HANDLE file = BAD;
    memset(&object, 0, sizeof(object));
    object.Length = sizeof(object);
    object.RootDirectory = parent;
    object.ObjectName = &text;
    object.Attributes = 0x1040; /* OBJ_DONT_REPARSE | OBJ_CASE_INSENSITIVE. */
    NTSTATUS result = NtCreateFile(&file, access | SYNCHRONIZE, &object, &status,
        NULL, attributes, share, disposition,
        0x200020 | (directory ? 1 : 0x40) | extra, NULL, 0);
    if (result < 0) { SetLastError(RtlNtStatusToDosError(result)); return BAD; }
    Identity actual;
    if (!native_identity(file, &actual, directory)) {
        /* Returned handles are tracked by the caller before validation. */
        SetLastError(ERROR_ACCESS_DENIED);
    }
    return file;
}
#endif

static int check_selection(Context *c) {
    if (c->selection == BAD) return 1;
    Identity actual;
    Native parent = c->directories[c->count-1];
    if (!native_identity(c->selection, &actual, 0) ||
        !same_identity(&actual, &c->selection_identity)) return 0;
    unsigned char bytes[MAX_SELECTION];
#ifdef _WIN32
    Descriptor *record = malloc(sizeof(*record));
    if (!record) return 0;
    Native reopened = relative_open(parent, SELECTION_FILE, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 1, 0,
        FILE_ATTRIBUTE_NORMAL, 0);
    if (reopened == BAD) { free(record); return 0; }
    if (!track(c, reopened, record)) return 0;
    int valid = native_identity(reopened, &actual, 0) &&
        same_identity(&actual, &c->selection_identity);
    if (!domus_native_close(reopened) || !valid) return 0;
    LARGE_INTEGER size, zero;
    zero.QuadPart = 0;
    DWORD read = 0;
    AcquireSRWLockExclusive(&c->selection_read_lock);
    valid = GetFileSizeEx(c->selection, &size) && size.QuadPart == c->selection_size &&
        SetFilePointerEx(c->selection, zero, NULL, FILE_BEGIN) &&
        ReadFile(c->selection, bytes, (DWORD)c->selection_size, &read, NULL) &&
        read == (DWORD)c->selection_size;
    ReleaseSRWLockExclusive(&c->selection_read_lock);
    return valid && !memcmp(bytes, c->selection_record, (size_t)c->selection_size);
#else
    if (actual.st_uid != geteuid() || (actual.st_mode & 0077) ||
        actual.st_size != c->selection_size ||
        fstatat(parent, SELECTION_FILE, &actual, AT_SYMLINK_NOFOLLOW) ||
        !S_ISREG(actual.st_mode) || !same_identity(&actual, &c->selection_identity)) return 0;
    ssize_t read;
    do { read = pread(c->selection, bytes, (size_t)c->selection_size, 0); }
    while (read < 0 && errno == EINTR);
    return read == c->selection_size &&
        !memcmp(bytes, c->selection_record, (size_t)c->selection_size);
#endif
}

static int check(Context *c) {
    int bad;
    LOCK(); bad = c->invalid || c->closing; UNLOCK();
    if (bad) return 0;
    for (int i = 0; i <= c->count; i++) {
        Identity current;
        Native parent, pin;
        const char *name;
        if (i == 0) {
            if (!native_identity(c->directories[0], &current, 1) ||
                !same_identity(&current, &c->identities[0])) goto stale;
            continue;
        }
        if (i == c->count) {
            parent = c->directories[c->count-1];
            pin = c->temporary;
            name = "sqlite-tmp";
        } else {
            parent = c->directories[i-1];
            pin = c->directories[i];
            name = c->components[i];
        }
#ifdef _WIN32
        Descriptor *record = malloc(sizeof(*record));
        if (!record) return 0;
        Native reopened = relative_open(parent, name, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, 1, 1,
            FILE_ATTRIBUTE_NORMAL, 0);
        if (reopened == BAD) { free(record); goto stale; }
        if (!track(c, reopened, record)) return 0;
        int valid = native_identity(reopened, &current, 1) &&
            same_identity(&current, i == c->count ? &c->temporary_identity : &c->identities[i]);
        if (!domus_native_close(reopened)) return 0;
        if (!valid) goto stale;
#else
        if (fstatat(parent, name, &current, AT_SYMLINK_NOFOLLOW) ||
            !S_ISDIR(current.st_mode) ||
            !same_identity(&current, i == c->count ? &c->temporary_identity : &c->identities[i]))
            goto stale;
#endif
        if (!native_identity(pin, &current, 1) ||
            !same_identity(&current, i == c->count ? &c->temporary_identity : &c->identities[i]))
            goto stale;
#ifndef _WIN32
        if (i == c->count && (current.st_uid != geteuid() ||
            (current.st_mode & 0777) != 0700)) goto stale;
#endif
    }
    if (!check_selection(c)) goto stale;
    return 1;
stale:
    LOCK(); c->invalid = 1; UNLOCK();
    return 0;
}

static int temp_leaf(const char *name) {
    return strlen(name) == 39 && !strncmp(name, "etilqs_", 7) &&
        strspn(name+7, "0123456789abcdef") == 32;
}
/* Labels are opaque routing tokens, never paths handed to the OS. */
static int route(Context *c, const char *path, Native *parent, const char **name) {
    size_t n = strlen(c->anchor);
    if (strncmp(path, c->anchor, n)) return 0;
    if (!path[n]) { *parent = c->directories[c->count-1]; *name = NULL; return 1; }
    if (path[n] != '/') return 0;
    const char *leaf = path+n+1;
    if (!strncmp(leaf, "sqlite-tmp/", 11) && temp_leaf(leaf+11)) {
        *parent = c->temporary; *name = leaf+11; return 2;
    }
    size_t length = strlen(c->leaf);
    if (strncmp(leaf, c->leaf, length)) return 0;
    const char *suffix = leaf+length;
    if (*suffix && strcmp(suffix, "-wal") && strcmp(suffix, "-shm") &&
        strcmp(suffix, "-journal")) return 0;
    *parent = c->directories[c->count-1]; *name = leaf; return 1;
}
static int scoped_name(const char *path) {
    return path && (!strncmp(path, "/domus-sqlite-", 14) ||
        !strncmp(path, "C:/domus-sqlite-", 16));
}
static Context *lookup(const char *path) {
    Context *found = NULL;
    LOCK();
    for (Context *c = contexts; c; c = c->next) {
        size_t n = strlen(c->anchor);
        if (!c->closing && !strncmp(path, c->anchor, n) &&
            (!path[n] || path[n] == '/')) { c->refs++; found = c; break; }
    }
    UNLOCK();
    return found;
}
static int track(Context *c, Native file, Descriptor *d) {
    d->native = file; d->context = c; d->failed = 0; d->closing = 0;
    LOCK();
retry:
    for (Descriptor *old = descriptors; old; old = old->next) {
        if (old->native == file) {
            /* The OS may reuse a successfully closed value before its close
               callback removes our record. Do not confuse this with an
               uncertain failed close. Waiting releases the registry lock. */
            if (old->closing) { WAIT(); goto retry; }
            if (old->context) old->context->invalid = old->context->close_error = 1;
            if (c) c->invalid = c->close_error = 1;
            d->failed = 1;
            if (c) { c->refs++; c->native_live++; }
            d->next = descriptors; descriptors = d;
            UNLOCK(); return 0;
        }
    }
    if (c) { c->refs++; c->native_live++; }
    d->next = descriptors; descriptors = d;
    UNLOCK();
    return 1;
}

#ifdef _WIN32
BOOL WINAPI domus_native_close(HANDLE file) {
#else
int domus_native_close(int file) {
#endif
    Descriptor *found = NULL;
    LOCK();
    for (Descriptor *d = descriptors; d; d = d->next) {
        if (d->native == file) {
            if (d->failed || d->closing) {
                if (d->context) d->context->invalid = d->context->close_error = 1;
                UNLOCK();
#ifdef _WIN32
                SetLastError(ERROR_ACCESS_DENIED); return FALSE;
#else
                errno = EIO; return -1;
#endif
            }
            /* Mark in flight so no caller retries an uncertain close. */
            d->closing = 1; found = d; break;
        }
    }
    UNLOCK();
    int result = plain_close(file);
#ifdef _WIN32
    DWORD error = GetLastError();
#else
    int error = errno;
#endif
    if (found) {
        Context *c = found->context;
        if (result) {
            LOCK();
            found->failed = 1; found->closing = 0;
            if (c) c->invalid = c->close_error = 1;
            NOTIFY();
            UNLOCK();
            if (c) quarantine(c);
        }
        else {
            LOCK();
            Descriptor **p = &descriptors;
            while (*p && *p != found) p = &(*p)->next;
            if (*p) *p = found->next;
            if (c) c->native_live--;
            NOTIFY();
            UNLOCK();
            free(found); if (c) release(c);
        }
    }
#ifdef _WIN32
    SetLastError(error); return result ? FALSE : TRUE;
#else
    errno = error; return result;
#endif
}

#ifndef _WIN32
int domus_native_open(const char *path, int flags, int mode) {
    if (!scoped_name(path)) {
        Descriptor *record=malloc(sizeof(*record));
        if (!record) { errno=ENOMEM; return -1; }
        int file=open(path,flags,mode), error=errno;
        if (file<0) free(record);
        else if (!track(NULL,file,record)) { errno=EIO; return -1; }
        errno=error; return file;
    }
    Context *c = lookup(path);
    if (!c) { errno = EACCES; return -1; }
    Native parent;
    const char *name;
    int kind = route(c, path, &parent, &name), file = -1;
    if (!kind || !check(c)) { errno = EACCES; goto done; }
    Descriptor *record = malloc(sizeof(*record));
    if (!record) { errno = ENOMEM; goto done; }
    if (!name) file = duplicate(parent);
    else {
        if (kind == 2 && (flags & (O_CREAT|O_EXCL|O_RDWR)) != (O_CREAT|O_EXCL|O_RDWR)) {
            free(record); errno = EACCES; goto done;
        }
        file = openat(parent, name, flags | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC, mode);
    }
    if (file >= 0) {
        if (!track(c, file, record)) {
            file = -1; errno = EIO; goto done;
        }
        Identity actual;
        if (!native_identity(file, &actual, !name)) {
            domus_native_close(file); file = -1; errno = EACCES;
        } else if (kind == 2) { LOCK(); c->temporary_opens++; UNLOCK(); }
    } else free(record);
done: {
    int error = errno;
    release(c); errno = error; return file;
    }
}
int domus_native_stat(const char *path, struct stat *out) {
    if (!scoped_name(path)) return stat(path, out);
    Context *c = lookup(path);
    if (!c) { errno = EACCES; return -1; }
    Native parent;
    const char *name;
    int rc = -1;
    if (route(c, path, &parent, &name) && check(c)) {
        rc = name ? fstatat(parent, name, out, AT_SYMLINK_NOFOLLOW) : fstat(parent, out);
        if (!rc && (name ? !S_ISREG(out->st_mode) : !S_ISDIR(out->st_mode))) {
            errno = EACCES; rc = -1;
        }
    } else errno = EACCES;
    int error = errno; release(c); errno = error; return rc;
}
int domus_native_lstat(const char *path, struct stat *out) {
    return scoped_name(path) ? domus_native_stat(path,out) : lstat(path,out);
}
int domus_native_access(const char *path, int mode) {
    if (!scoped_name(path)) return access(path, mode);
    Context *c = lookup(path);
    if (!c) { errno = EACCES; return -1; }
    Native parent;
    const char *name;
    struct stat s;
    int rc = -1;
    if (route(c, path, &parent, &name) && name && check(c) &&
        !fstatat(parent, name, &s, AT_SYMLINK_NOFOLLOW) && S_ISREG(s.st_mode)) {
        rc = (int)syscall(SYS_faccessat2, parent, name, mode, AT_EACCESS|AT_SYMLINK_NOFOLLOW);
    } else errno = EACCES;
    int error = errno; release(c); errno = error; return rc;
}
int domus_native_unlink(const char *path) {
    if (!scoped_name(path)) return unlink(path);
    Context *c = lookup(path);
    if (!c) { errno = EACCES; return -1; }
    Native parent;
    const char *name;
    struct stat s;
    int rc = -1;
    if (route(c, path, &parent, &name) && name && check(c)) {
        if (!fstatat(parent, name, &s, AT_SYMLINK_NOFOLLOW)) {
            if (S_ISREG(s.st_mode)) rc = unlinkat(parent, name, 0);
            else errno = EACCES;
        }
    } else errno = EACCES;
    int error = errno; release(c); errno = error; return rc;
}
#else
static Context *descriptor_context(Native file) {
    Context *result=NULL;
    LOCK();
    for (Descriptor *d=descriptors;d;d=d->next)
        if (d->native==file) {
            result=d->context;
            if (result) result->refs++;
            break;
        }
    UNLOCK();
    return result;
}
HANDLE WINAPI domus_native_mapping_w(HANDLE file, LPSECURITY_ATTRIBUTES security,
    DWORD protect, DWORD high, DWORD low, LPCWSTR name) {
    Context *c=descriptor_context(file);
    HANDLE mapping=NULL;
    if (c && (!check(c) || security || name)) { SetLastError(ERROR_ACCESS_DENIED); goto done; }
    Descriptor *record=malloc(sizeof(*record));
    if (!record) { SetLastError(ERROR_NOT_ENOUGH_MEMORY); goto done; }
    mapping=CreateFileMappingW(file,security,protect,high,low,name);
    DWORD native_error=GetLastError();
    if (!mapping) free(record);
    else if (!track(c,mapping,record)) { mapping=NULL; SetLastError(ERROR_ACCESS_DENIED); }
    else SetLastError(native_error);
done: {
    DWORD error=GetLastError(); if (c) release(c); SetLastError(error); return mapping;
    }
}
HANDLE WINAPI domus_native_mapping_a(HANDLE file, LPSECURITY_ATTRIBUTES security,
    DWORD protect, DWORD high, DWORD low, LPCSTR name) {
    Context *c=descriptor_context(file);
    if (!c) {
        Descriptor *record=malloc(sizeof(*record));
        if (!record) { SetLastError(ERROR_NOT_ENOUGH_MEMORY); return NULL; }
        HANDLE mapping=CreateFileMappingA(file,security,protect,high,low,name);
        DWORD error=GetLastError();
        if (!mapping) free(record);
        else if (!track(NULL,mapping,record)) { SetLastError(ERROR_ACCESS_DENIED); return NULL; }
        SetLastError(error); return mapping;
    }
    release(c);
    if (name) { SetLastError(ERROR_ACCESS_DENIED); return NULL; }
    return domus_native_mapping_w(file,security,protect,high,low,NULL);
}
static int narrow_path(LPCWSTR path, char *out) {
    if (!WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, path, -1, out,
        PATH_SIZE, NULL, NULL)) return 0;
    for (char *p = out; *p; p++) if (*p == '\\') *p = '/';
    return 1;
}
HANDLE WINAPI domus_native_create(LPCWSTR path, DWORD access, DWORD share,
    LPSECURITY_ATTRIBUTES security, DWORD disposition, DWORD flags, HANDLE template) {
    char label[PATH_SIZE];
    if (!narrow_path(path, label)) return BAD;
    if (!scoped_name(label)) {
        Descriptor *record=malloc(sizeof(*record));
        if (!record) { SetLastError(ERROR_NOT_ENOUGH_MEMORY); return BAD; }
        HANDLE file=CreateFileW(path,access,share,security,disposition,flags,template);
        DWORD error=GetLastError();
        if (file==BAD) free(record);
        else if (!track(NULL,file,record)) { SetLastError(ERROR_ACCESS_DENIED); return BAD; }
        SetLastError(error); return file;
    }
    Context *c = lookup(label);
    if (!c) { SetLastError(ERROR_ACCESS_DENIED); return BAD; }
    Native parent, file = BAD;
    const char *name;
    int kind = route(c, label, &parent, &name);
    if (!kind || !name || security || template || !check(c)) goto denied;
    ULONG native_disposition;
    if (disposition == CREATE_NEW) native_disposition = 2;
    else if (disposition == OPEN_ALWAYS) native_disposition = 3;
    else if (disposition == OPEN_EXISTING) native_disposition = 1;
    else goto denied;
    ULONG extra = 0;
    DWORD attributes = FILE_ATTRIBUTE_NORMAL;
    if (kind == 2) {
        if (disposition != CREATE_NEW ||
            flags != (FILE_ATTRIBUTE_TEMPORARY|FILE_ATTRIBUTE_HIDDEN|FILE_FLAG_DELETE_ON_CLOSE))
            goto denied;
        extra = 0x1000; access |= DELETE;
        attributes = FILE_ATTRIBUTE_TEMPORARY|FILE_ATTRIBUTE_HIDDEN;
    } else if (flags != FILE_ATTRIBUTE_NORMAL) goto denied;
    Descriptor *record = malloc(sizeof(*record));
    if (!record) { SetLastError(ERROR_NOT_ENOUGH_MEMORY); goto done; }
    file = relative_open(parent, name, access, share, native_disposition, 0, attributes, extra);
    if (file != BAD) {
        if (!track(c, file, record)) {
            file = BAD; SetLastError(ERROR_ACCESS_DENIED);
        } else {
            Identity actual;
            if (!native_identity(file, &actual, 0)) {
                domus_native_close(file); file = BAD; SetLastError(ERROR_ACCESS_DENIED);
            } else if (kind == 2) { LOCK(); c->temporary_opens++; UNLOCK(); }
        }
    } else free(record);
    goto done;
denied:
    SetLastError(ERROR_ACCESS_DENIED);
done: {
    DWORD error = GetLastError(); release(c); SetLastError(error); return file;
    }
}
BOOL WINAPI domus_native_attributes_ex(LPCWSTR path, GET_FILEEX_INFO_LEVELS level, LPVOID output) {
    char label[PATH_SIZE];
    if (!narrow_path(path, label)) return FALSE;
    if (!scoped_name(label)) return GetFileAttributesExW(path, level, output);
    Context *c = lookup(label);
    if (!c) { SetLastError(ERROR_ACCESS_DENIED); return FALSE; }
    Native parent, file = BAD;
    const char *name;
    int success = 0;
    if (level != GetFileExInfoStandard || !route(c, label, &parent, &name) || !check(c))
        goto denied;
    Descriptor *record = malloc(sizeof(*record));
    if (!record) { SetLastError(ERROR_NOT_ENOUGH_MEMORY); goto done; }
    file = name ? relative_open(parent, name, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE, 1, 0, FILE_ATTRIBUTE_NORMAL, 0)
        : duplicate(parent);
    if (file == BAD) { free(record); goto done; }
    if (!track(c, file, record)) {
        SetLastError(ERROR_ACCESS_DENIED); goto done;
    }
    Identity identity;
    BY_HANDLE_FILE_INFORMATION info;
    if (native_identity(file, &identity, !name) && GetFileInformationByHandle(file, &info)) {
        WIN32_FILE_ATTRIBUTE_DATA *out = output;
        out->dwFileAttributes = info.dwFileAttributes;
        out->ftCreationTime = info.ftCreationTime;
        out->ftLastAccessTime = info.ftLastAccessTime;
        out->ftLastWriteTime = info.ftLastWriteTime;
        out->nFileSizeHigh = info.nFileSizeHigh; out->nFileSizeLow = info.nFileSizeLow;
        success = 1;
    }
    if (!domus_native_close(file)) success = 0;
    goto done;
denied: SetLastError(ERROR_ACCESS_DENIED);
done: {
    DWORD error = GetLastError(); release(c); SetLastError(error); return success;
    }
}
DWORD WINAPI domus_native_attributes(LPCWSTR path) {
    WIN32_FILE_ATTRIBUTE_DATA data;
    return domus_native_attributes_ex(path, GetFileExInfoStandard, &data)
        ? data.dwFileAttributes : INVALID_FILE_ATTRIBUTES;
}
BOOL WINAPI domus_native_delete(LPCWSTR path) {
    char label[PATH_SIZE];
    if (!narrow_path(path, label)) return FALSE;
    if (!scoped_name(label)) return DeleteFileW(path);
    Context *c = lookup(label);
    if (!c) { SetLastError(ERROR_ACCESS_DENIED); return FALSE; }
    Native parent, file = BAD;
    const char *name;
    int success = 0;
    if (!route(c, label, &parent, &name) || !name || !check(c)) {
        SetLastError(ERROR_ACCESS_DENIED); goto done;
    }
    Descriptor *record = malloc(sizeof(*record));
    if (!record) { SetLastError(ERROR_NOT_ENOUGH_MEMORY); goto done; }
    file = relative_open(parent, name, DELETE|FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE, 1, 0, FILE_ATTRIBUTE_NORMAL, 0);
    if (file == BAD) { free(record); goto done; }
    if (!track(c, file, record)) {
        SetLastError(ERROR_ACCESS_DENIED); goto done;
    }
    Identity actual;
    FILE_DISPOSITION_INFO disposition = { TRUE };
    if (native_identity(file, &actual, 0))
        success = SetFileInformationByHandle(file, FileDispositionInfo, &disposition, sizeof(disposition));
    if (!domus_native_close(file)) success = 0;
done: {
    DWORD error = GetLastError(); release(c); SetLastError(error); return success;
    }
}
#endif

typedef struct ScopedFile {
    sqlite3_file file;
    sqlite3_file *inner;
    Context *context;
    int flags;
    char name[PATH_SIZE];
    sqlite3_io_methods methods;
} ScopedFile;
static int allowed(ScopedFile *f) { return check(f->context); }
static int file_close(sqlite3_file *file) {
    ScopedFile *f = (ScopedFile *)file;
    Context *c = f->context;
    int rc = f->inner->pMethods->xClose(f->inner);
    if (rc != SQLITE_OK) quarantine(c);
    sqlite3_free(f->inner);
    f->inner = NULL; f->file.pMethods = NULL;
    LOCK(); int failed = c->close_error; UNLOCK();
    int retired = release(c);
    return failed || retired != SQLITE_OK ? SQLITE_IOERR_CLOSE : rc;
}
static int file_read(sqlite3_file *file, void *b, int n, sqlite3_int64 o) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xRead(f->inner,b,n,o) : SQLITE_IOERR_READ;
}
static int file_write(sqlite3_file *file, const void *b, int n, sqlite3_int64 o) {
    ScopedFile *f = (ScopedFile *)file;
    if (!allowed(f)) return SQLITE_IOERR_WRITE;
    int rc = f->inner->pMethods->xWrite(f->inner,b,n,o);
    if (!rc && (f->flags & (SQLITE_OPEN_TEMP_DB|SQLITE_OPEN_TEMP_JOURNAL|
        SQLITE_OPEN_SUBJOURNAL|SQLITE_OPEN_TRANSIENT_DB))) {
        LOCK(); f->context->temporary_bytes += n; UNLOCK();
    }
    return rc;
}
static int file_truncate(sqlite3_file *file, sqlite3_int64 n) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xTruncate(f->inner,n) : SQLITE_IOERR_TRUNCATE;
}
static int file_sync(sqlite3_file *file, int flags) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xSync(f->inner,flags) : SQLITE_IOERR_FSYNC;
}
static int file_size(sqlite3_file *file, sqlite3_int64 *n) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xFileSize(f->inner,n) : SQLITE_IOERR_FSTAT;
}
static int file_lock(sqlite3_file *file, int lock) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xLock(f->inner,lock) : SQLITE_IOERR_LOCK;
}
static int file_unlock(sqlite3_file *file, int lock) {
    ScopedFile *f = (ScopedFile *)file;
    return f->inner->pMethods->xUnlock(f->inner,lock);
}
static int file_reserved(sqlite3_file *file, int *out) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xCheckReservedLock(f->inner,out) : SQLITE_IOERR_CHECKRESERVEDLOCK;
}
static int file_control(sqlite3_file *file, int op, void *out) {
    ScopedFile *f = (ScopedFile *)file;
    if (!allowed(f)) return SQLITE_IOERR;
    /* No upstream request may manufacture an OS temporary pathname. */
    if (op == SQLITE_FCNTL_TEMPFILENAME) return SQLITE_NOTFOUND;
    return f->inner->pMethods->xFileControl(f->inner,op,out);
}
static int file_sector(sqlite3_file *file) {
    ScopedFile *f = (ScopedFile *)file;
    return f->inner->pMethods->xSectorSize(f->inner);
}
static int file_device(sqlite3_file *file) {
    ScopedFile *f = (ScopedFile *)file;
    return f->inner->pMethods->xDeviceCharacteristics(f->inner);
}
static int file_map(sqlite3_file *file, int page, int n, int extend, void volatile **out) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xShmMap(f->inner,page,n,extend,out) : SQLITE_IOERR_SHMMAP;
}
static int file_shm_lock(sqlite3_file *file, int offset, int n, int flags) {
    ScopedFile *f = (ScopedFile *)file;
    if (!(flags & SQLITE_SHM_UNLOCK) && !allowed(f)) return SQLITE_IOERR_SHMLOCK;
    return f->inner->pMethods->xShmLock(f->inner,offset,n,flags);
}
static void file_barrier(sqlite3_file *file) {
    ScopedFile *f = (ScopedFile *)file;
    f->inner->pMethods->xShmBarrier(f->inner);
}
static int file_unmap(sqlite3_file *file, int remove) {
    ScopedFile *f = (ScopedFile *)file;
    return f->inner->pMethods->xShmUnmap(f->inner,remove && allowed(f));
}
static int file_fetch(sqlite3_file *file, sqlite3_int64 offset, int n, void **out) {
    ScopedFile *f = (ScopedFile *)file;
    return allowed(f) ? f->inner->pMethods->xFetch(f->inner,offset,n,out) : SQLITE_IOERR_MMAP;
}
static int file_unfetch(sqlite3_file *file, sqlite3_int64 offset, void *out) {
    ScopedFile *f = (ScopedFile *)file;
    return f->inner->pMethods->xUnfetch(f->inner,offset,out);
}

static int vfs_path(sqlite3_vfs *vfs, const char *name, int n, char *out) {
    Context *c = vfs->pAppData;
    if (strcmp(name, c->leaf) || !check(c)) return SQLITE_CANTOPEN;
    int size = snprintf(out, (size_t)n, "%s/%s", c->anchor, c->leaf);
    return size > 0 && size < n ? SQLITE_OK : SQLITE_CANTOPEN;
}
static int vfs_open(sqlite3_vfs *vfs, sqlite3_filename path, sqlite3_file *file,
    int flags, int *actual) {
    Context *c = vfs->pAppData;
    ScopedFile *f = (ScopedFile *)file;
    memset(f, 0, sizeof(*f));
    if (!check(c)) {
        LOCK(); int failed = c->close_error; UNLOCK();
        return failed ? SQLITE_IOERR_CLOSE : SQLITE_CANTOPEN;
    }
    if (!path) {
        int types = SQLITE_OPEN_TEMP_DB|SQLITE_OPEN_TEMP_JOURNAL|
            SQLITE_OPEN_SUBJOURNAL|SQLITE_OPEN_TRANSIENT_DB;
        if (!(flags & types) || !(flags & SQLITE_OPEN_DELETEONCLOSE) ||
            !(flags & SQLITE_OPEN_CREATE)) return SQLITE_CANTOPEN;
        unsigned char random[16];
        sqlite3_randomness(16, random);
        int n = snprintf(f->name, sizeof(f->name), "%s/sqlite-tmp/etilqs_", c->anchor);
        for (int i = 0; i < 16; i++) snprintf(f->name+n+i*2, 3, "%02x", random[i]);
        flags |= SQLITE_OPEN_EXCLUSIVE;
    } else {
        Native parent;
        const char *name;
        if (route(c, path, &parent, &name) != 1 || !name ||
            ((flags & SQLITE_OPEN_MAIN_DB) &&
             (strcmp(name, c->leaf) || sqlite3_uri_key(path, 0)))) return SQLITE_CANTOPEN;
        if (strlen(path) + 2 >= sizeof(f->name)) return SQLITE_CANTOPEN;
        memcpy(f->name, path, strlen(path)+1);
    }
    f->inner = sqlite3_malloc64((sqlite3_uint64)c->base->szOsFile);
    if (!f->inner) return SQLITE_NOMEM;
    memset(f->inner, 0, (size_t)c->base->szOsFile);
    f->context = c; f->flags = flags;
    retain(c);
    int rc = c->base->xOpen(c->base, f->name, f->inner, flags|SQLITE_OPEN_NOFOLLOW, actual);
    if (rc != SQLITE_OK) {
        if (f->inner->pMethods) f->inner->pMethods->xClose(f->inner);
        sqlite3_free(f->inner); f->inner = NULL; release(c); return rc;
    }
    sqlite3_io_methods *m = &f->methods;
    *m = *f->inner->pMethods;
    m->xClose=file_close; m->xRead=file_read; m->xWrite=file_write;
    m->xTruncate=file_truncate; m->xSync=file_sync; m->xFileSize=file_size;
    m->xLock=file_lock; m->xUnlock=file_unlock; m->xCheckReservedLock=file_reserved;
    m->xFileControl=file_control; m->xSectorSize=file_sector;
    m->xDeviceCharacteristics=file_device;
    if (m->iVersion >= 2) {
        if (m->xShmMap) m->xShmMap=file_map;
        if (m->xShmLock) m->xShmLock=file_shm_lock;
        if (m->xShmBarrier) m->xShmBarrier=file_barrier;
        if (m->xShmUnmap) m->xShmUnmap=file_unmap;
    }
    if (m->iVersion >= 3) {
        if (m->xFetch) m->xFetch=file_fetch;
        if (m->xUnfetch) m->xUnfetch=file_unfetch;
    }
    f->file.pMethods=m;
    return SQLITE_OK;
}
static int vfs_access(sqlite3_vfs *vfs, const char *path, int flags, int *out) {
    Context *c = vfs->pAppData;
    Native parent;
    const char *name;
    *out = 0;
    if (route(c,path,&parent,&name) != 1 || !name || !check(c)) return SQLITE_IOERR_ACCESS;
#ifdef _WIN32
    wchar_t wide[PATH_SIZE];
    if (!MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,path,-1,wide,PATH_SIZE)) return SQLITE_IOERR_ACCESS;
    WIN32_FILE_ATTRIBUTE_DATA data;
    if (!domus_native_attributes_ex(wide,GetFileExInfoStandard,&data)) {
        DWORD error=GetLastError();
        return error==ERROR_FILE_NOT_FOUND || error==ERROR_PATH_NOT_FOUND ? SQLITE_OK : SQLITE_IOERR_ACCESS;
    }
    *out = flags != SQLITE_ACCESS_READWRITE || !(data.dwFileAttributes & FILE_ATTRIBUTE_READONLY);
#else
    struct stat data;
    if (fstatat(parent,name,&data,AT_SYMLINK_NOFOLLOW))
        return errno==ENOENT ? SQLITE_OK : SQLITE_IOERR_ACCESS;
    if (!S_ISREG(data.st_mode)) return SQLITE_IOERR_ACCESS;
    if (flags == SQLITE_ACCESS_EXISTS) *out = data.st_size > 0;
    else {
        int mode = flags == SQLITE_ACCESS_READWRITE ? R_OK|W_OK : R_OK;
        if (!syscall(SYS_faccessat2,parent,name,mode,AT_EACCESS|AT_SYMLINK_NOFOLLOW)) *out=1;
        else if (errno!=EACCES && errno!=EPERM && errno!=EROFS && errno!=ENOENT) return SQLITE_IOERR_ACCESS;
    }
#endif
    return SQLITE_OK;
}
static int vfs_delete(sqlite3_vfs *vfs, const char *path, int sync) {
    Context *c=vfs->pAppData;
    Native parent;
    const char *name;
    if (route(c,path,&parent,&name)!=1 || !name || !check(c)) return SQLITE_IOERR_DELETE;
#ifdef _WIN32
    (void)sync;
    wchar_t wide[PATH_SIZE];
    if (!MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,path,-1,wide,PATH_SIZE)) return SQLITE_IOERR_DELETE;
    if (domus_native_delete(wide)) return SQLITE_OK;
    DWORD error=GetLastError();
    return error==ERROR_FILE_NOT_FOUND || error==ERROR_PATH_NOT_FOUND ? SQLITE_OK : SQLITE_IOERR_DELETE;
#else
    if (domus_native_unlink(path) && errno!=ENOENT) return SQLITE_IOERR_DELETE;
    return sync && fsync(parent) ? SQLITE_IOERR_DIR_FSYNC : SQLITE_OK;
#endif
}
static void *no_load(sqlite3_vfs *vfs, const char *name) { (void)vfs; (void)name; return NULL; }
static void no_load_error(sqlite3_vfs *vfs, int n, char *out) {
    (void)vfs; sqlite3_snprintf(n,out,"extensions disabled for scoped SQLite");
}

/*
 * Public C ABI. Input handles and UTF-8 component pointers are borrowed only
 * during construction. Directory pins are duplicated. The original locked
 * selection handle is transferred when selection_adopted becomes 1, including
 * error paths. No Rust pointer is retained. Context owns one release() reference.
 */
int domus_sqlite_context(const intptr_t *handles, const char *const *names,
    int count, intptr_t temporary, const char *leaf, intptr_t selection,
    const unsigned char *selection_record, int selection_size,
    int *selection_adopted, Context **output) {
    *output=NULL;
    *selection_adopted=0;
    if (domus_sqlite_source_boundary_v1()!=3053002 ||
        sqlite3_libversion_number()!=3053002) return SQLITE_MISUSE;
    if (count < 1 || count > MAX_CHAIN || !good_component(leaf)) return SQLITE_MISUSE;
    if (((Native)selection==BAD && selection_size) ||
        ((Native)selection!=BAD && (!selection_record || selection_size<1 ||
         selection_size>MAX_SELECTION))) return SQLITE_MISUSE;
    for (int i=1;i<count;i++) if (!good_component(names[i])) return SQLITE_MISUSE;
    if (sqlite3_initialize()!=SQLITE_OK) return SQLITE_ERROR;
#ifdef _WIN32
    sqlite3_vfs *base=sqlite3_vfs_find("win32");
#else
    sqlite3_vfs *base=sqlite3_vfs_find("unix");
#endif
    if (!base || base->iVersion<2) return SQLITE_CANTOPEN;
    Context *c=calloc(1,sizeof(*c));
    if (!c) return SQLITE_NOMEM;
    c->count=count; c->refs=1; c->base=base; c->temporary=BAD; c->selection=BAD;
#ifdef _WIN32
    InitializeSRWLock(&c->selection_read_lock);
#endif
    for (int i=0;i<MAX_CHAIN;i++) c->directories[i]=BAD;
    if ((Native)selection!=BAD) {
        c->selection=(Native)selection;
        *selection_adopted=1;
        c->selection_size=selection_size;
        memcpy(c->selection_record,selection_record,(size_t)selection_size);
        if (!native_identity(c->selection,&c->selection_identity,0)) goto fail;
    }
    memcpy(c->leaf,leaf,strlen(leaf)+1);
    for (int i=0;i<count;i++) {
        c->directories[i]=duplicate((Native)handles[i]);
        if (c->directories[i]==BAD ||
            !native_identity(c->directories[i],&c->identities[i],1)) goto fail;
        if (i) memcpy(c->components[i],names[i],strlen(names[i])+1);
    }
    c->temporary=duplicate((Native)temporary);
    if (c->temporary==BAD || !native_identity(c->temporary,&c->temporary_identity,1) || !check(c)) goto fail;
    LOCK(); c->id=++next_id; UNLOCK();
#ifdef _WIN32
    snprintf(c->anchor,sizeof(c->anchor),"C:/domus-sqlite-%llu",c->id);
#else
    snprintf(c->anchor,sizeof(c->anchor),"/domus-sqlite-%llu",c->id);
#endif
    snprintf(c->vfs_name,sizeof(c->vfs_name),"domus_native_%llu",c->id);
    c->vfs=*base; c->vfs.zName=c->vfs_name; c->vfs.pNext=NULL;
    c->vfs.pAppData=c; c->vfs.szOsFile=sizeof(ScopedFile); c->vfs.mxPathname=PATH_SIZE-2;
    c->vfs.xFullPathname=vfs_path; c->vfs.xOpen=vfs_open;
    c->vfs.xAccess=vfs_access; c->vfs.xDelete=vfs_delete;
    c->vfs.xDlOpen=no_load; c->vfs.xDlError=no_load_error;
    /* Runtime syscall substitution is not part of this boundary. */
    c->vfs.xSetSystemCall=NULL; c->vfs.xGetSystemCall=NULL; c->vfs.xNextSystemCall=NULL;
    LOCK();
    for (Context *old=contexts;old;old=old->next) {
        if ((!old->closing || old->close_error) && old->count==count && !strcmp(old->leaf,c->leaf) &&
            same_identity(&old->identities[count-1],&c->identities[count-1]) &&
            same_identity(&old->temporary_identity,&c->temporary_identity)) {
            int equal=1;
            if ((old->selection==BAD)!=(c->selection==BAD) ||
                old->selection_size!=c->selection_size ||
                (c->selection!=BAD && (!same_identity(&old->selection_identity,&c->selection_identity) ||
                 memcmp(old->selection_record,c->selection_record,(size_t)c->selection_size))))
                equal=0;
            for (int i=0;i<count;i++)
                if (!same_identity(&old->identities[i],&c->identities[i]) ||
                    strcmp(old->components[i],c->components[i])) equal=0;
            if (equal) {
                old->refs++;
                while (!old->ready) WAIT();
                int ready=old->ready;
                UNLOCK();
                int released=release(c);
                if (released!=SQLITE_OK) {
                    quarantine(old); release(old); return released;
                }
                if (ready<0 || !check(old)) { release(old); return SQLITE_CANTOPEN; }
                *output=old; return SQLITE_OK;
            }
        }
    }
    c->next=contexts; contexts=c; c->listed=1;
    UNLOCK();
    int rc=sqlite3_vfs_register(&c->vfs,0);
    LOCK(); c->ready=rc==SQLITE_OK ? 1 : -1; NOTIFY(); UNLOCK();
    if (rc!=SQLITE_OK) { release(c); return rc; }
    *output=c; return SQLITE_OK;
fail:
    release(c); return SQLITE_CANTOPEN;
}
int domus_sqlite_check(Context *c) {
    if (check(c)) return SQLITE_OK;
    LOCK(); int failed=c->close_error; UNLOCK();
    return failed ? SQLITE_IOERR_CLOSE : SQLITE_CANTOPEN;
}
int domus_sqlite_release(Context *c) { return release(c); }
void domus_sqlite_retain(Context *c) { retain(c); }
const char *domus_sqlite_vfs(Context *c) { return c->vfs_name; }
const char *domus_sqlite_leaf(Context *c) { return c->leaf; }
void domus_sqlite_stats(Context *c, unsigned long long *out) {
    LOCK();
    out[0]=c->temporary_opens; out[1]=c->temporary_bytes;
    out[2]=c->native_live; out[3]=(unsigned long long)c->close_error;
    UNLOCK();
}
static int authorize(void *context, int action, const char *first,
    const char *second, const char *schema, const char *trigger) {
    (void)context; (void)second; (void)schema; (void)trigger;
    if ((action==SQLITE_ATTACH && (!first || *first)) ||
        (action==SQLITE_PRAGMA && first &&
         (!sqlite3_stricmp(first,"temp_store_directory") ||
          !sqlite3_stricmp(first,"data_store_directory")))) return SQLITE_DENY;
    return SQLITE_OK;
}
static void connection_destroy(void *context) { release(context); }
static void authority_function(sqlite3_context *context, int argc, sqlite3_value **argv) {
    (void)argc; (void)argv; sqlite3_result_null(context);
}
int domus_sqlite_bind(Context *c, sqlite3 *db) {
    retain(c);
    int rc=sqlite3_create_function_v2(db,"domus_native_authority",0,SQLITE_UTF8,
        c,authority_function,NULL,NULL,connection_destroy);
    if (rc!=SQLITE_OK) return rc; /* SQLite calls the destructor on failure. */
    return sqlite3_set_authorizer(db,authorize,c);
}
