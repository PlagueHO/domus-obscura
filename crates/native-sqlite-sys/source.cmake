cmake_minimum_required(VERSION 3.24)

set(VERSION "0.38.2")
set(SHA256 "f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8")
if(NOT DEFINED SOURCE_CACHE)
    message(FATAL_ERROR "SOURCE_CACHE is required")
endif()
get_filename_component(SOURCE_CACHE "${SOURCE_CACHE}" ABSOLUTE)
file(MAKE_DIRECTORY "${SOURCE_CACHE}")
set(ARCHIVE "${SOURCE_CACHE}/libsqlite3-sys-${VERSION}.crate")
if(NOT EXISTS "${ARCHIVE}")
    if(OFFLINE)
        message(FATAL_ERROR "Pinned SQLite source is not cached; an online build is required first")
    endif()
    message(STATUS "Downloading pinned libsqlite3-sys ${VERSION}")
    file(DOWNLOAD
        "https://static.crates.io/crates/libsqlite3-sys/libsqlite3-sys-${VERSION}.crate"
        "${ARCHIVE}.part" TLS_VERIFY ON EXPECTED_HASH "SHA256=${SHA256}"
        TIMEOUT 120 INACTIVITY_TIMEOUT 30 STATUS DOWNLOAD_STATUS)
    list(GET DOWNLOAD_STATUS 0 DOWNLOAD_CODE)
    if(NOT DOWNLOAD_CODE EQUAL 0)
        message(FATAL_ERROR "Pinned SQLite source download failed: ${DOWNLOAD_STATUS}")
    endif()
    file(RENAME "${ARCHIVE}.part" "${ARCHIVE}")
endif()
file(SHA256 "${ARCHIVE}" ACTUAL_SHA256)
if(NOT ACTUAL_SHA256 STREQUAL SHA256)
    message(FATAL_ERROR "Pinned SQLite source checksum failed; the archive is unchanged")
endif()
file(ARCHIVE_EXTRACT INPUT "${ARCHIVE}" DESTINATION "${SOURCE_CACHE}" PATTERNS
    "libsqlite3-sys-${VERSION}/sqlite3/sqlite3.c"
    "libsqlite3-sys-${VERSION}/sqlite3/sqlite3.h"
    "libsqlite3-sys-${VERSION}/sqlite3/bindgen_bundled_version.rs"
    "libsqlite3-sys-${VERSION}/src/lib.rs"
    "libsqlite3-sys-${VERSION}/src/error.rs"
    "libsqlite3-sys-${VERSION}/LICENSE")
