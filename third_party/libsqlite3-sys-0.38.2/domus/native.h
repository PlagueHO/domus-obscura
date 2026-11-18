/* Native ABI for the pinned SQLite 3.53.2 source boundary. */
#ifndef DOMUS_NATIVE_SQLITE_H
#define DOMUS_NATIVE_SQLITE_H
#ifdef _WIN32
HANDLE WINAPI domus_native_create(LPCWSTR, DWORD, DWORD,
    LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE);
BOOL WINAPI domus_native_close(HANDLE);
BOOL WINAPI domus_native_delete(LPCWSTR);
DWORD WINAPI domus_native_attributes(LPCWSTR);
BOOL WINAPI domus_native_attributes_ex(LPCWSTR, GET_FILEEX_INFO_LEVELS, LPVOID);
HANDLE WINAPI domus_native_mapping_w(HANDLE, LPSECURITY_ATTRIBUTES, DWORD, DWORD, DWORD, LPCWSTR);
HANDLE WINAPI domus_native_mapping_a(HANDLE, LPSECURITY_ATTRIBUTES, DWORD, DWORD, DWORD, LPCSTR);
#else
int domus_native_open(const char *, int, int);
int domus_native_close(int);
int domus_native_unlink(const char *);
int domus_native_stat(const char *, struct stat *);
int domus_native_lstat(const char *, struct stat *);
int domus_native_access(const char *, int);
#endif
#endif
