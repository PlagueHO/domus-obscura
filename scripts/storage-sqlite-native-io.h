/* Compile-time native I/O boundary for the isolated pinned-source proof. */
#ifndef DOMUS_SQLITE_NATIVE_IO_V1
#define DOMUS_SQLITE_NATIVE_IO_V1

#ifdef _WIN32
HANDLE WINAPI sqlite3_domus_native_create_v1(LPCWSTR path, DWORD access,
    DWORD share, LPSECURITY_ATTRIBUTES security, DWORD disposition,
    DWORD flags, HANDLE template);
BOOL WINAPI sqlite3_domus_native_close_v1(HANDLE file);
#else
int sqlite3_domus_native_open_v1(const char *path, int flags, int mode);
int sqlite3_domus_native_unlink_v1(const char *path);
int sqlite3_domus_native_close_v1(int file);
#endif

#endif
