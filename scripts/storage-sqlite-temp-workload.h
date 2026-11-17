/* Public SQLite file-method forwarding for the isolated temporary-file proof. */
#ifndef DOMUS_SQLITE_TEMP_WORKLOAD
#define DOMUS_SQLITE_TEMP_WORKLOAD

typedef struct {
    sqlite3_file file;
    sqlite3_file *inner;
    char name[4096];
    int flags;
    sqlite3_io_methods methods;
} TemporaryFile;

static sqlite3_int64 temp_database_bytes, temp_other_bytes;
static int temp_native_opens, temp_closes, temp_close_errors;
static const char *forced_temp_leaf;
static int wal_snapshot_checks, busy_close_checks, deferred_close_checks;
static sqlite3_int64 subjournal_bytes, transient_bytes;
static int subjournal_opens, transient_opens;
static int force_temp_open_error;
static int auxiliary_checks, blob_lifetime_checks, backup_lifetime_checks;
static int namespace_refusal_checks;
static int namespace_native_refusals;
static int entry_policy_checks, entry_uri_refusals, entry_authorizer_refusals;

static int entry_authorize(void *context, int action, const char *first,
    const char *second, const char *schema, const char *trigger) {
    (void)context; (void)second; (void)schema; (void)trigger;
    if ((action == SQLITE_ATTACH && (!first || *first)) ||
        (action == SQLITE_PRAGMA && first &&
            (!sqlite3_stricmp(first, "temp_store_directory") ||
             !sqlite3_stricmp(first, "data_store_directory")))) {
        PROBE_LOCK();
        entry_authorizer_refusals++;
        PROBE_UNLOCK();
        return SQLITE_DENY;
    }
    return SQLITE_OK;
}

static int entry_open(const char *name, sqlite3 **db, int flags, const char *vfs) {
    *db = NULL;
    int read = SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW;
    int write = SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW;
    if (!name || strcmp(name, "state.sqlite") ||
        (flags != read && flags != write && flags != (write | SQLITE_OPEN_CREATE))) {
        return SQLITE_CANTOPEN;
    }
    int result = sqlite3_open_v2(name, db, flags, vfs);
    if (result != SQLITE_OK) return result;
    result = sqlite3_set_authorizer(*db, entry_authorize, NULL);
    if (result != SQLITE_OK) return result;
    sqlite3_limit(*db, SQLITE_LIMIT_ATTACHED, 1);
    return sqlite3_db_config(*db, SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, 0, NULL);
}

static int entry_filename(sqlite3_filename path, int flags) {
    if (path && (flags & SQLITE_OPEN_MAIN_DB) && sqlite3_uri_key(path, 0)) {
        PROBE_LOCK();
        entry_uri_refusals++;
        PROBE_UNLOCK();
        return 0;
    }
    return 1;
}

static int entry_control(sqlite3 *db, int operation, void *out) {
    if (operation != SQLITE_FCNTL_DATA_VERSION) return SQLITE_NOTFOUND;
    return sqlite3_file_control(db, "main", operation, out);
}

static int temporary_leaf(const char *name) {
    if (!name || strncmp(name, "etilqs_", 7) || strlen(name) != 39) return 0;
    return strspn(name + 7, "0123456789abcdef") == 32;
}

static int temp_close(sqlite3_file *file) {
    TemporaryFile *t = (TemporaryFile *)file;
    int result = t->inner->pMethods->xClose(t->inner);
    sqlite3_free(t->inner);
    t->inner = NULL;
    t->file.pMethods = NULL;
    PROBE_LOCK();
    temp_closes++;
    if (result != SQLITE_OK) temp_close_errors++;
    PROBE_UNLOCK();
    return result;
}

static int temp_read(sqlite3_file *f, void *b, int n, sqlite3_int64 o) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xRead(t->inner, b, n, o);
}
static int temp_write(sqlite3_file *f, const void *b, int n, sqlite3_int64 o) {
    TemporaryFile *t = (TemporaryFile *)f;
    int result = t->inner->pMethods->xWrite(t->inner, b, n, o);
    if (result == SQLITE_OK) {
        PROBE_LOCK();
        if (t->flags & SQLITE_OPEN_TEMP_DB) temp_database_bytes += n;
        else temp_other_bytes += n;
        if (t->flags & SQLITE_OPEN_SUBJOURNAL) subjournal_bytes += n;
        if (t->flags & SQLITE_OPEN_TRANSIENT_DB) transient_bytes += n;
        PROBE_UNLOCK();
    }
    return result;
}
static int temp_truncate(sqlite3_file *f, sqlite3_int64 n) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xTruncate(t->inner, n);
}
static int temp_sync(sqlite3_file *f, int flags) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xSync(t->inner, flags);
}
static int temp_size(sqlite3_file *f, sqlite3_int64 *n) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xFileSize(t->inner, n);
}
static int temp_lock(sqlite3_file *f, int lock) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xLock(t->inner, lock);
}
static int temp_unlock(sqlite3_file *f, int lock) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xUnlock(t->inner, lock);
}
static int temp_reserved(sqlite3_file *f, int *out) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xCheckReservedLock(t->inner, out);
}
static int temp_control(sqlite3_file *f, int op, void *out) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xFileControl(t->inner, op, out);
}
static int temp_sector(sqlite3_file *f) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xSectorSize(t->inner);
}
static int temp_device(sqlite3_file *f) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xDeviceCharacteristics(t->inner);
}
static int temp_map(sqlite3_file *f, int page, int size, int extend, void volatile **out) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xShmMap(t->inner, page, size, extend, out);
}
static int temp_shm_lock(sqlite3_file *f, int offset, int count, int flags) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xShmLock(t->inner, offset, count, flags);
}
static void temp_barrier(sqlite3_file *f) {
    TemporaryFile *t = (TemporaryFile *)f;
    t->inner->pMethods->xShmBarrier(t->inner);
}
static int temp_unmap(sqlite3_file *f, int remove) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xShmUnmap(t->inner, remove);
}
static int temp_fetch(sqlite3_file *f, sqlite3_int64 offset, int n, void **out) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xFetch(t->inner, offset, n, out);
}
static int temp_unfetch(sqlite3_file *f, sqlite3_int64 offset, void *out) {
    TemporaryFile *t = (TemporaryFile *)f;
    return t->inner->pMethods->xUnfetch(t->inner, offset, out);
}

static int temporary_open(sqlite3_vfs *upstream, const char *anchor, char separator,
    sqlite3_file *file, int flags, int *actual) {
    int types = SQLITE_OPEN_TEMP_DB | SQLITE_OPEN_TEMP_JOURNAL |
        SQLITE_OPEN_SUBJOURNAL | SQLITE_OPEN_TRANSIENT_DB;
    if (!(flags & types) || !(flags & SQLITE_OPEN_CREATE) ||
        !(flags & SQLITE_OPEN_DELETEONCLOSE)) return SQLITE_CANTOPEN;
    TemporaryFile *t = (TemporaryFile *)file;
    memset(t, 0, sizeof(*t));
    unsigned char random[16];
    char leaf_name[40] = "etilqs_";
    sqlite3_randomness(sizeof(random), random);
    for (int i = 0; i < 16; i++)
        sqlite3_snprintf(3, leaf_name + 7 + i * 2, "%02x", random[i]);
    if (forced_temp_leaf) {
        if (!temporary_leaf(forced_temp_leaf)) return SQLITE_CANTOPEN;
        memcpy(leaf_name, forced_temp_leaf, sizeof(leaf_name));
    }
    int length = sqlite3_snprintf(sizeof(t->name) - 1, t->name, "%s%c%s",
        anchor, separator, leaf_name) ? (int)strlen(t->name) : 0;
    if (!length || length >= (int)sizeof(t->name) - 2) return SQLITE_CANTOPEN;
    t->inner = sqlite3_malloc64((sqlite3_uint64)upstream->szOsFile);
    if (!t->inner) return SQLITE_NOMEM;
    memset(t->inner, 0, (size_t)upstream->szOsFile);
    int result = upstream->xOpen(upstream, t->name, t->inner,
        flags | SQLITE_OPEN_EXCLUSIVE | SQLITE_OPEN_NOFOLLOW, actual);
    if (result != SQLITE_OK) {
        if (t->inner->pMethods) t->inner->pMethods->xClose(t->inner);
        sqlite3_free(t->inner);
        t->inner = NULL;
        return result;
    }
    int version = t->inner->pMethods->iVersion;
    if (version < 1 || version > 3) {
        t->inner->pMethods->xClose(t->inner);
        sqlite3_free(t->inner);
        t->inner = NULL;
        return SQLITE_IOERR;
    }
    sqlite3_io_methods *m = &t->methods;
    *m = *t->inner->pMethods;
    m->xClose = temp_close;
    m->xRead = temp_read;
    m->xWrite = temp_write;
    m->xTruncate = temp_truncate;
    m->xSync = temp_sync;
    m->xFileSize = temp_size;
    m->xLock = temp_lock;
    m->xUnlock = temp_unlock;
    m->xCheckReservedLock = temp_reserved;
    m->xFileControl = temp_control;
    m->xSectorSize = temp_sector;
    m->xDeviceCharacteristics = temp_device;
    if (m->xShmMap) m->xShmMap = temp_map;
    if (m->xShmLock) m->xShmLock = temp_shm_lock;
    if (m->xShmBarrier) m->xShmBarrier = temp_barrier;
    if (m->xShmUnmap) m->xShmUnmap = temp_unmap;
    if (version >= 3) {
        if (m->xFetch) m->xFetch = temp_fetch;
        if (m->xUnfetch) m->xUnfetch = temp_unfetch;
    }
    t->flags = flags;
    t->file.pMethods = m;
    PROBE_LOCK();
    if (flags & SQLITE_OPEN_SUBJOURNAL) subjournal_opens++;
    if (flags & SQLITE_OPEN_TRANSIENT_DB) transient_opens++;
    PROBE_UNLOCK();
    if (force_temp_open_error) {
        int close_result = temp_close(&t->file);
        return close_result == SQLITE_OK ? SQLITE_IOERR : close_result;
    }
    return SQLITE_OK;
}

static const char temporary_seed_sql[] =
    "PRAGMA temp_store=FILE; PRAGMA temp.cache_size=8;"
        "CREATE TEMP TABLE spill(id INTEGER PRIMARY KEY,payload TEXT);"
        "WITH RECURSIVE source(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM source WHERE x<8192)"
        "INSERT INTO spill SELECT x,printf('%08d',8193-x)||hex(zeroblob(512)) FROM source;";

static void temporary_seed(sqlite3 *db) {
    execute(db, temporary_seed_sql);
    PROBE_LOCK();
    int written = temp_database_bytes > 0;
    PROBE_UNLOCK();
    require(written, "actual TEMP database disk writes");
}

static void temporary_payload(char *expected, int prefix) {
    sqlite3_snprintf(9, expected, "%08d", prefix);
    memset(expected + 8, '0', 1024);
    expected[1032] = 0;
}

static int temporary_rows(sqlite3 *db, const char *sql, int reversed) {
    sqlite3_stmt *query = NULL;
    int result = sqlite3_prepare_v2(db, sql, -1, &query, NULL);
    if (result != SQLITE_OK) return result;
    int count = 0, correct = 1;
    while ((result = sqlite3_step(query)) == SQLITE_ROW) {
        char expected[1033];
        temporary_payload(expected, reversed ? count + 1 : 8192 - count);
        const char *value = (const char *)sqlite3_column_text(query, 1);
        if (sqlite3_column_int(query, 0) != (reversed ? 8192 - count : count + 1) || !value ||
            sqlite3_column_bytes(query, 1) != 1032 || strcmp(value, expected)) correct = 0;
        count++;
    }
    int finish = sqlite3_finalize(query);
    if (result != SQLITE_DONE) return result;
    if (finish != SQLITE_OK) return finish;
    require(count == 8192 && correct, "every sorted key and exact payload is correct");
    return SQLITE_OK;
}

static int temporary_sort(sqlite3 *db) {
    return temporary_rows(db, "SELECT id,payload FROM temp.spill ORDER BY payload", 1);
}

static sqlite3_int64 lifetime_scalar(sqlite3 *db, const char *sql) {
    sqlite3_stmt *query = NULL;
    require(sqlite3_prepare_v2(db, sql, -1, &query, NULL) == SQLITE_OK,
        "lifetime scalar prepare");
    require(sqlite3_step(query) == SQLITE_ROW, "lifetime scalar result");
    sqlite3_int64 value = sqlite3_column_int64(query, 0);
    require(sqlite3_finalize(query) == SQLITE_OK, "lifetime scalar finalize");
    return value;
}

static sqlite3 *readonly_reader(const char *vfs) {
    sqlite3 *reader = NULL;
    require(entry_open("state.sqlite", &reader,
        SQLITE_OPEN_READONLY | SQLITE_OPEN_NOFOLLOW, vfs) == SQLITE_OK,
        "native read-only WAL connection opens");
    require(sqlite3_busy_timeout(reader, 5000) == SQLITE_OK, "read-only busy timeout");
    int result = sqlite3_exec(reader, "INSERT INTO values_table VALUES(777778)",
        NULL, NULL, NULL);
    require((result & 0xff) == SQLITE_READONLY, "read-only WAL connection refuses SQL mutation");
    return reader;
}

static void wal_snapshot(sqlite3 *reader, sqlite3 *writer) {
    sqlite3_int64 count = lifetime_scalar(reader, "SELECT count(*) FROM values_table");
    sqlite3_int64 sum = lifetime_scalar(reader, "SELECT sum(value) FROM values_table");
    require(lifetime_scalar(reader, "SELECT count(*) FROM values_table WHERE value=777777") == 0,
        "fresh contention marker");
    execute(reader, "BEGIN;");
    require(lifetime_scalar(reader, "SELECT sum(value) FROM values_table") == sum,
        "reader starts exact WAL snapshot");
    execute(writer, "INSERT INTO values_table VALUES(777777);");
    require(lifetime_scalar(writer, "SELECT count(*) FROM values_table") == count + 1 &&
        lifetime_scalar(writer, "SELECT sum(value) FROM values_table") == sum + 777777,
        "same-process writer commits exact new WAL value");
    require(lifetime_scalar(reader, "SELECT count(*) FROM values_table") == count &&
        lifetime_scalar(reader, "SELECT sum(value) FROM values_table") == sum,
        "active reader does not silently change snapshots");
    require(sqlite3_busy_timeout(writer, 0) == SQLITE_OK, "bounded checkpoint contention");
    int log = -1, checkpointed = -1;
    require(sqlite3_wal_checkpoint_v2(writer, "main", SQLITE_CHECKPOINT_TRUNCATE,
        &log, &checkpointed) == SQLITE_BUSY && log > checkpointed && checkpointed >= 0,
        "active reader prevents complete WAL truncation");
    execute(reader, "COMMIT;");
    require(sqlite3_wal_checkpoint_v2(writer, "main", SQLITE_CHECKPOINT_TRUNCATE,
        &log, &checkpointed) == SQLITE_OK && log == 0 && checkpointed == 0,
        "WAL truncation completes after reader release");
    require(lifetime_scalar(reader, "SELECT count(*) FROM values_table") == count + 1 &&
        lifetime_scalar(reader, "SELECT sum(value) FROM values_table") == sum + 777777,
        "new read observes the exact committed WAL value");
    execute(writer, "DELETE FROM values_table WHERE value=777777;");
    require(lifetime_scalar(reader, "SELECT count(*) FROM values_table") == count &&
        lifetime_scalar(reader, "SELECT sum(value) FROM values_table") == sum,
        "contention fixture restores exact original values");
    require(sqlite3_busy_timeout(writer, 5000) == SQLITE_OK, "restore fixture busy timeout");
    wal_snapshot_checks++;
}

static sqlite3 *temporary_memory(sqlite3 *source) {
    sqlite3_vfs *vfs = NULL;
    sqlite3 *memory = NULL;
    require(sqlite3_file_control(source, "main", SQLITE_FCNTL_VFS_POINTER, &vfs) == SQLITE_OK &&
        vfs && sqlite3_open_v2(":memory:", &memory,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, vfs->zName) == SQLITE_OK,
        "diagnostic memory destination uses selected VFS");
    require(lifetime_scalar(source, "PRAGMA temp.page_size") == 4096,
        "pinned TEMP backup page size");
    execute(memory, "PRAGMA page_size=4096;");
    return memory;
}

static sqlite3_backup *temporary_backup(sqlite3 *source, sqlite3 *destination) {
    sqlite3_backup *backup = sqlite3_backup_init(destination, "main", source, "temp");
    require(backup && sqlite3_backup_step(backup, 1) == SQLITE_OK &&
        sqlite3_backup_remaining(backup) > 0, "active backup retains native TEMP source");
    return backup;
}

static void temporary_backup_verify(sqlite3 *destination) {
    require(temporary_rows(destination, "SELECT id,payload FROM main.spill ORDER BY id", 0)
        == SQLITE_OK, "backup preserves every key and full payload");
    require(sqlite3_close(destination) == SQLITE_OK, "completed memory backup closes");
}

static sqlite3_blob *temporary_blob(sqlite3 *db) {
    sqlite3_blob *blob = NULL;
    require(sqlite3_blob_open(db, "temp", "spill", "payload", 1, 0, &blob) == SQLITE_OK,
        "incremental BLOB owns native TEMP pages");
    return blob;
}

static void temporary_blob_verify(sqlite3_blob *blob) {
    char actual[1033], expected[1033];
    temporary_payload(expected, 8192);
    require(sqlite3_blob_bytes(blob) == 1032 &&
        sqlite3_blob_read(blob, actual, 1032, 0) == SQLITE_OK &&
        !memcmp(actual, expected, 1032), "incremental BLOB reads full exact payload");
}

static void temporary_auxiliary(sqlite3 *db) {
    sqlite3_file *file = NULL;
    sqlite3_int64 size = -1;
    unsigned char prefix[2], bytes[8];
    require(sqlite3_file_control(db, "temp", SQLITE_FCNTL_FILE_POINTER, &file) == SQLITE_OK &&
        file && file->pMethods && file->pMethods->xFileSize(file, &size) == SQLITE_OK && size >= 2,
        "borrow existing SQLite-managed TEMP file without another descriptor");
    require(file->pMethods->xRead(file, prefix, 2, size - 2) == SQLITE_OK,
        "read exact existing end-of-file bytes");
    memset(bytes, 0xff, sizeof(bytes));
    require(file->pMethods->xRead(file, bytes, sizeof(bytes), size - 2) == SQLITE_IOERR_SHORT_READ &&
        !memcmp(bytes, prefix, 2) && !memcmp(bytes + 2, "\0\0\0\0\0\0", 6),
        "native short read preserves available bytes and zero-fills missing bytes");
    require(file->pMethods->xFileControl(file, 1000, NULL) == SQLITE_NOTFOUND,
        "unknown file control is not a claimed capability");

    int opened = temp_native_opens, closed = temp_closes;
    force_temp_open_error = 1;
    int result = temporary_sort(db);
    force_temp_open_error = 0;
    if ((result & 0xff) != SQLITE_IOERR || temp_native_opens <= opened ||
        temp_native_opens - opened != temp_closes - closed || temp_close_errors) {
        fprintf(stderr, "failed-open: sqlite=%d; native=%d; closed=%d; close_errors=%d\n",
            result, temp_native_opens - opened, temp_closes - closed, temp_close_errors);
    }
    require((result & 0xff) == SQLITE_IOERR && temp_native_opens > opened &&
        temp_native_opens - opened == temp_closes - closed && temp_close_errors == 0,
        "failed VFS open closes native allocation before returning an error");
    require(temporary_sort(db) == SQLITE_OK, "exact sort recovers after failed open cleanup");

    sqlite3_int64 count = lifetime_scalar(db, "SELECT count(*) FROM values_table");
    sqlite3_int64 sum = lifetime_scalar(db, "SELECT sum(value) FROM values_table");
    execute(db, "PRAGMA cache_size=8;"
        "CREATE TABLE maintenance(id INTEGER PRIMARY KEY,payload TEXT);"
        "INSERT INTO maintenance SELECT id,payload FROM temp.spill;");
    sqlite3_int64 before = subjournal_bytes;
    execute(db, "BEGIN; UPDATE maintenance SET payload=hex(zeroblob(516));"
        "SAVEPOINT auxiliary; UPDATE maintenance SET payload=zeroblob(1032);"
        "ROLLBACK TO auxiliary; RELEASE auxiliary; ROLLBACK;");
    if (subjournal_bytes <= before) {
        fprintf(stderr, "subjournal: opens=%d bytes=%lld; temp_store=%lld; spill=%lld\n",
            subjournal_opens, (long long)subjournal_bytes,
            (long long)lifetime_scalar(db, "PRAGMA temp_store"),
            (long long)lifetime_scalar(db, "PRAGMA cache_spill"));
    }
    require(subjournal_bytes > before && subjournal_opens > 0,
        "savepoint rollback uses actual native disk subjournal");
    require(temporary_rows(db, "SELECT id,payload FROM main.maintenance ORDER BY id", 0)
        == SQLITE_OK, "rollback restores every original main fixture payload");

    before = transient_bytes;
    require(lifetime_scalar(db,
        "SELECT sum(length(payload)) FROM (SELECT DISTINCT payload FROM temp.spill)") == 8454144 &&
        transient_bytes > before && transient_opens > 0,
        "DISTINCT materialization writes actual native transient database");

    opened = temp_native_opens;
    before = temp_database_bytes + temp_other_bytes;
    execute(db, "VACUUM;");
    require(temp_native_opens > opened && temp_database_bytes + temp_other_bytes > before,
        "VACUUM writes actual native private maintenance scratch");
    require(temporary_rows(db, "SELECT id,payload FROM main.maintenance ORDER BY id", 0)
        == SQLITE_OK, "VACUUM preserves exact fixture payloads");
    execute(db, "DROP TABLE maintenance;");
    require(lifetime_scalar(db, "SELECT count(*) FROM values_table") == count &&
        lifetime_scalar(db, "SELECT sum(value) FROM values_table") == sum,
        "maintenance preserves original committed values");
    auxiliary_checks++;
}

static int namespace_keys(sqlite3 *db, sqlite3_int64 *keys, int expected) {
    sqlite3_stmt *query = NULL;
    require(sqlite3_prepare_v2(db, "SELECT value FROM values_table ORDER BY value",
        -1, &query, NULL) == SQLITE_OK, "namespace committed-key query");
    int count = 0, result, matches = 1;
    while ((result = sqlite3_step(query)) == SQLITE_ROW) {
        require(count < 128, "bounded namespace fixture keys");
        sqlite3_int64 value = sqlite3_column_int64(query, 0);
        if (expected < 0) keys[count] = value;
        else if (count >= expected || keys[count] != value) matches = 0;
        count++;
    }
    require(result == SQLITE_DONE && matches && (expected < 0 || count == expected),
        "namespace refusal preserves every exact committed key");
    require(sqlite3_finalize(query) == SQLITE_OK, "namespace committed-key query closes");
    return count;
}

static void entry_policy(sqlite3 *db, const char *vfs) {
    const char *names[] = {
        ":memory:", "file:state.sqlite?mode=memory",
        "file:state.sqlite?immutable=1", "file:state.sqlite?nolock=1",
        "file:state.sqlite?modeof=domus-unselected"
    };
    sqlite3_int64 keys[128];
    int count = namespace_keys(db, keys, -1);
    for (unsigned int i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
        sqlite3 *other = NULL;
        require(entry_open(names[i], &other, SQLITE_OPEN_READWRITE |
            SQLITE_OPEN_CREATE | SQLITE_OPEN_NOFOLLOW, vfs) == SQLITE_CANTOPEN && !other,
            "state entry refuses memory and URI authority before SQLite parsing");
        entry_policy_checks++;
    }
    const char *uris[] = {
        "file:state.sqlite?immutable=1", "file:state.sqlite?nolock=1",
        "file:state.sqlite?modeof=domus-unselected"
    };
    for (unsigned int i = 0; i < sizeof(uris) / sizeof(uris[0]); i++) {
        sqlite3 *other = NULL;
        int before = entry_uri_refusals;
        int result = sqlite3_open_v2(uris[i], &other,
            SQLITE_OPEN_READONLY | SQLITE_OPEN_URI | SQLITE_OPEN_NOFOLLOW, vfs);
        require((result & 0xff) == SQLITE_CANTOPEN && entry_uri_refusals > before,
            "raw valid-name URI reaches and fails the scoped VFS parameter guard");
        require(!other || sqlite3_close(other) == SQLITE_OK, "URI refusal connection closes");
        entry_policy_checks++;
    }
    const char *sql[] = {
        "ATTACH DATABASE ':memory:' AS forbidden",
        "ATTACH DATABASE 'file:state.sqlite?mode=memory' AS forbidden",
        "ATTACH DATABASE 'file:state.sqlite?nolock=1' AS forbidden",
        "PRAGMA temp_store_directory='domus-unselected'",
        "PRAGMA data_store_directory='domus-unselected'"
    };
    for (unsigned int i = 0; i < sizeof(sql) / sizeof(sql[0]); i++) {
        int before = entry_authorizer_refusals;
        require(sqlite3_exec(db, sql[i], NULL, NULL, NULL) == SQLITE_AUTH &&
            entry_authorizer_refusals > before,
            "entry authorizer refuses new roots and process-directory changes before effects");
        entry_policy_checks++;
    }
    const int controls[] = {
        SQLITE_FCNTL_NULL_IO, SQLITE_FCNTL_WIN32_SET_HANDLE,
        SQLITE_FCNTL_FILE_POINTER, SQLITE_FCNTL_VFS_POINTER, SQLITE_FCNTL_WIN32_GET_HANDLE
    };
    for (unsigned int i = 0; i < sizeof(controls) / sizeof(controls[0]); i++) {
        void *unchanged = db;
        require(entry_control(db, controls[i], &unchanged) == SQLITE_NOTFOUND && unchanged == db,
            "caller control cannot replace disable or expose a native handle");
        entry_policy_checks++;
    }
    unsigned int version = 0;
    require(entry_control(db, SQLITE_FCNTL_DATA_VERSION, &version) == SQLITE_OK && version > 0,
        "explicit read-only data-version control remains usable");
    namespace_keys(db, keys, count);
    require(lifetime_scalar(db, "SELECT count(*) FROM pragma_database_list") == 2,
        "entry refusals leave only main and required TEMP databases");
}

static void namespace_refusals(sqlite3 *db, const char *vfs) {
    const char *names[] = {
        "domus-rejected.sqlite",
        "../domus-rejected.sqlite",
        "file:domus-rejected.sqlite?mode=rwc"
    };
    sqlite3_int64 keys[128];
    int count = namespace_keys(db, keys, -1);
    for (unsigned int i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
        sqlite3 *other = NULL;
        int result = sqlite3_open_v2(names[i], &other,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_URI |
            SQLITE_OPEN_NOFOLLOW, vfs);
        require((result & 0xff) == SQLITE_CANTOPEN,
            "unexpected filename or URI cannot select another state database");
        require(!other || sqlite3_close(other) == SQLITE_OK, "refused connection closes");
        other = NULL;
        require(sqlite3_open_v2("state.sqlite", &other,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_NOFOLLOW, vfs) == SQLITE_OK,
            "invalid filename does not invalidate retained authority");
        namespace_keys(other, keys, count);
        require(sqlite3_close(other) == SQLITE_OK, "valid refusal-check connection closes");
        namespace_refusal_checks++;
    }

    char *uri = sqlite3_mprintf("file:domus-rejected.sqlite?vfs=%s", base->zName);
    require(uri != NULL, "alternate-VFS refusal URI allocation");
    sqlite3 *other = NULL;
    int refused = namespace_native_refusals;
    int result = sqlite3_open_v2(uri, &other,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_URI |
        SQLITE_OPEN_NOFOLLOW, vfs);
    sqlite3_free(uri);
    require((result & 0xff) == SQLITE_CANTOPEN,
        "URI alternate VFS still cannot escape the active native root");
    require(namespace_native_refusals > refused,
        "alternate-VFS refusal reaches the actual native guard, not a permission fallback");
    require(!other || sqlite3_close(other) == SQLITE_OK, "alternate-VFS refusal closes");
    namespace_refusal_checks++;

    require(sqlite3_exec(db, "ATTACH DATABASE 'domus-rejected.sqlite' AS forbidden",
        NULL, NULL, NULL) != SQLITE_OK, "external ATTACH destination is not admitted");
    require(sqlite3_exec(db, "VACUUM INTO 'domus-rejected.sqlite'",
        NULL, NULL, NULL) != SQLITE_OK, "external VACUUM INTO destination is not admitted");
    require(lifetime_scalar(db, "SELECT count(*) FROM pragma_database_list") == 2,
        "refused destinations do not leave an attached database");
    namespace_keys(db, keys, count);
    namespace_refusal_checks += 2;

    entry_policy(db, vfs);
}

static void temporary_busy_close(sqlite3 *db) {
    sqlite3 *destination = temporary_memory(db);
    sqlite3_backup *backup = temporary_backup(db, destination);
    int closed = temp_closes;
    require(sqlite3_close(db) == SQLITE_BUSY && temp_closes == closed,
        "unfinished backup alone prevents source close");
    require(sqlite3_backup_step(backup, -1) == SQLITE_DONE &&
        sqlite3_backup_finish(backup) == SQLITE_OK, "busy-close backup finishes normally");
    temporary_backup_verify(destination);

    sqlite3_stmt *query = NULL;
    require(sqlite3_prepare_v2(db, "SELECT sum(id) FROM temp.spill", -1,
        &query, NULL) == SQLITE_OK, "busy-close live TEMP statement");
    closed = temp_closes;
    require(temp_native_opens > closed, "busy-close uses actual open TEMP file");
    require(sqlite3_close(db) == SQLITE_BUSY && temp_closes == closed,
        "busy close does not release TEMP file callbacks");
    require(sqlite3_step(query) == SQLITE_ROW &&
        sqlite3_column_int64(query, 0) == 33558528, "statement remains valid after busy close");
    require(sqlite3_finalize(query) == SQLITE_OK, "busy-close statement finalize");
    sqlite3_blob *blob = temporary_blob(db);
    require(sqlite3_close(db) == SQLITE_BUSY && temp_closes == closed,
        "live BLOB alone prevents source close");
    temporary_blob_verify(blob);
    require(sqlite3_blob_close(blob) == SQLITE_OK, "busy-close BLOB releases normally");
    busy_close_checks++;
}

static void temporary_deferred_close(sqlite3 *db) {
    sqlite3_stmt *query = NULL;
    require(sqlite3_prepare_v2(db, "SELECT sum(id) FROM temp.spill", -1,
        &query, NULL) == SQLITE_OK, "deferred-close live TEMP statement");
    int opened = temp_native_opens, closed = temp_closes;
    sqlite3_blob *blob = temporary_blob(db);
    sqlite3 *destination = temporary_memory(db);
    sqlite3_backup *backup = temporary_backup(db, destination);
    require(opened > closed, "deferred close has actual file ownership");
    require(sqlite3_close_v2(db) == SQLITE_OK && temp_closes == closed,
        "close_v2 success does not imply file callbacks have closed");
    require(sqlite3_step(query) == SQLITE_ROW &&
        sqlite3_column_int64(query, 0) == 33558528, "retained statement reads exact TEMP contents");
    require(sqlite3_finalize(query) == SQLITE_OK && temp_closes == closed,
        "statement finalize cannot release files owned by BLOB and backup");
    temporary_blob_verify(blob);
    require(sqlite3_blob_close(blob) == SQLITE_OK && temp_closes == closed,
        "BLOB close cannot release files still owned by backup");
    require(sqlite3_backup_step(backup, -1) == SQLITE_DONE &&
        sqlite3_backup_finish(backup) == SQLITE_OK && temp_closes == opened &&
        temp_close_errors == 0,
        "last backup finish closes every remaining TEMP file");
    temporary_backup_verify(destination);
    deferred_close_checks++;
    blob_lifetime_checks++;
    backup_lifetime_checks++;
}
#endif
