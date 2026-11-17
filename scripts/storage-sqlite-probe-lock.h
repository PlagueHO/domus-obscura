/* Only protects fixture metadata. Never hold this lock across upstream SQLite calls. */
#ifndef DOMUS_SQLITE_PROBE_LOCK
#define DOMUS_SQLITE_PROBE_LOCK
#define PROBE_LOCK() sqlite3_mutex_enter(sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP1))
#define PROBE_UNLOCK() sqlite3_mutex_leave(sqlite3_mutex_alloc(SQLITE_MUTEX_STATIC_APP1))
#ifdef _WIN32
#define PROBE_ASSERT_INCREMENT(value) InterlockedIncrement((volatile LONG *)&(value))
#else
#define PROBE_ASSERT_INCREMENT(value) __atomic_add_fetch(&(value), 1, __ATOMIC_RELAXED)
#endif
#endif
