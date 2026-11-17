/* One shared fixture context, selected by an actual Rust-owned directory pin. */
#include <stdint.h>

static int independent_pin_matches(intptr_t pin) {
#ifdef _WIN32
    FILE_ID_INFO actual, expected;
    return GetFileInformationByHandleEx((HANDLE)pin, FileIdInfo, &actual, sizeof(actual)) &&
        GetFileInformationByHandleEx(directories[directory_count - 1],
            FileIdInfo, &expected, sizeof(expected)) &&
        actual.VolumeSerialNumber == expected.VolumeSerialNumber &&
        !memcmp(&actual.FileId, &expected.FileId, sizeof(actual.FileId));
#else
    struct stat identity;
    return !fstat((int)pin, &identity) && same(&identity, &ownership_root->identity);
#endif
}

intptr_t domus_independent_pin(void) {
#ifdef _WIN32
    HANDLE pin = INVALID_HANDLE_VALUE;
    require(DuplicateHandle(GetCurrentProcess(), directories[directory_count - 1],
        GetCurrentProcess(), &pin, 0, FALSE, DUPLICATE_SAME_ACCESS),
        "duplicate actual directory authority into Rust ownership");
    return (intptr_t)pin;
#else
    int pin = fcntl(ownership_root->directory, F_DUPFD_CLOEXEC, 3);
    require(pin >= 0, "duplicate actual directory authority into Rust ownership");
    return (intptr_t)pin;
#endif
}

int domus_independent_live(intptr_t pin) {
    return independent_pin_matches(pin) && domus_probe_root_live(ownership_root);
}

int domus_independent_pin_closed(intptr_t pin) {
#ifdef _WIN32
    DWORD flags;
    return !GetHandleInformation((HANDLE)pin, &flags) && GetLastError() == ERROR_INVALID_HANDLE;
#else
    errno = 0;
    return fcntl((int)pin, F_GETFD) == -1 && errno == EBADF;
#endif
}

sqlite3 *domus_independent_connection(intptr_t pin) {
    require(domus_independent_live(pin), "connection borrows exact shared directory pin");
    return domus_probe_connection(ownership_root);
}

int domus_independent_drop(intptr_t pin) {
    require(independent_pin_matches(pin), "final owner retains the exact native directory");
    return domus_probe_root_drop(ownership_root);
}

int domus_independent_calls(void) {
    PROBE_LOCK();
    int calls = native_bridge_calls;
    PROBE_UNLOCK();
    return calls;
}

int domus_independent_quarantine(void) {
    PROBE_LOCK();
    require(native_close_errors > 0, "an actual native close failed before quarantine");
    ownership_quarantined = 1;
    PROBE_UNLOCK();
    return domus_probe_verify_quarantine();
}
